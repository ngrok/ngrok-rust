use std::{
    future::Future,
    io,
};

use async_trait::async_trait;
use futures::{
    channel::{
        mpsc,
        oneshot,
    },
    prelude::*,
    select,
    stream::StreamExt,
    SinkExt,
};
use tokio::io::{
    AsyncRead,
    AsyncWrite,
};
use tokio_util::codec::Framed;
use tracing::{
    debug,
    instrument,
    trace,
};

use crate::{
    codec::FrameCodec,
    errors::ErrorType,
    frame::{
        Body,
        Frame,
        Header,
        HeaderType,
        StreamID,
    },
    stream::Stream,
    stream_manager::{
        OpenReq,
        SharedStreamManager,
        StreamManager,
    },
};

const DEFAULT_WINDOW: usize = 0x40000; // 256KB
const DEFAULT_ACCEPT: usize = 64;
const DEFAULT_STREAMS: usize = 512;

pub trait SessionTask: Future<Output = Result<(), ErrorType>> + Send + 'static {}
impl<F> SessionTask for F where F: Future<Output = Result<(), ErrorType>> + Send + 'static {}

pub struct SessionBuilder<S> {
    io_stream: Option<S>,
    window: usize,
    accept_queue_size: usize,
    stream_limit: usize,
    client: bool,
}

impl<S> Default for SessionBuilder<S> {
    fn default() -> Self {
        SessionBuilder {
            io_stream: None,
            window: DEFAULT_WINDOW,
            accept_queue_size: DEFAULT_ACCEPT,
            stream_limit: DEFAULT_STREAMS,
            client: true,
        }
    }
}

impl<S> SessionBuilder<S>
where
    S: AsyncRead + AsyncWrite + Send + 'static,
{
    pub fn new(io_stream: S) -> Self {
        SessionBuilder {
            io_stream: io_stream.into(),
            ..Default::default()
        }
    }

    pub fn with_stream(mut self, io_stream: S) -> Self {
        self.io_stream = Some(io_stream);
        self
    }

    pub fn with_window_size(mut self, size: usize) -> Self {
        self.window = size;
        self
    }

    pub fn with_accept_queue_size(mut self, size: usize) -> Self {
        self.accept_queue_size = size;
        self
    }

    pub fn with_stream_limit(mut self, count: usize) -> Self {
        self.stream_limit = count;
        self
    }

    pub fn client(mut self) -> Self {
        self.client = true;
        self
    }

    pub fn server(mut self) -> Self {
        self.client = false;
        self
    }

    pub fn build(self) -> (impl ReadTask, impl WriteTask, Muxado) {
        let SessionBuilder {
            io_stream,
            window,
            accept_queue_size,
            stream_limit,
            client,
        } = self;
        let (accept_tx, accept_rx) = mpsc::channel(accept_queue_size);
        let (open_tx, open_rx) = mpsc::channel(512);
        let manager = StreamManager::new(stream_limit, client);
        let raw_output = manager.sys_sender();
        let (m1, m2) = manager.split();
        let (io_tx, io_rx) = Framed::new(
            io_stream.expect("no io stream provided"),
            FrameCodec::default(),
        )
        .split();

        let read_task = Reader {
            io: io_rx,
            accept_tx,
            window,
            manager: m1,
            last_stream_processed: StreamID::clamp(0),
            raw_output,
        };

        let write_task = Writer {
            io: io_tx,
            manager: m2,
            open_reqs: open_rx,
        };

        (
            read_task.run(),
            write_task.run(),
            Muxado {
                window,
                incoming: accept_rx,
                outgoing: open_tx,
            },
        )
    }

    #[cfg(feature = "tokio_rt")]
    pub fn start(self) -> Muxado {
        let (read, write, sess) = self.build();
        tokio::spawn(async move {
            let res = read.await;
            trace!(res = debug(res), "read task complete");
        });
        tokio::spawn(async move {
            let res = write.await;
            trace!(res = debug(res), "write task complete");
        });
        sess
    }
}

pub trait ReadTask: SessionTask {}
impl<T> ReadTask for T where T: SessionTask {}

struct Reader<R> {
    io: R,
    raw_output: mpsc::Sender<Frame>,
    accept_tx: mpsc::Sender<Stream>,
    window: usize,
    manager: SharedStreamManager,
    last_stream_processed: StreamID,
}
impl<R> Reader<R>
where
    R: futures::stream::Stream<Item = Result<Frame, io::Error>> + Unpin,
{
    #[instrument(level = "trace", skip(self))]
    async fn handle_frame(&mut self, frame: Frame) -> Result<(), ErrorType> {
        // If the remote sent a syn, create a new stream and add it to the accept channel.
        if frame.is_syn() {
            let (req, stream) = OpenReq::create(self.window, false);
            self.manager
                .lock()
                .await
                .create_stream(frame.header.stream_id.into(), req)?;
            self.accept_tx
                .send(stream)
                .map_err(|_| ErrorType::SessionClosed)
                .await?;
        }

        let needs_close = frame.is_fin();

        let Frame {
            header:
                Header {
                    length: _,
                    flags: _,
                    stream_id,
                    typ,
                },
            ..
        } = frame;

        match typ {
            HeaderType::Data | HeaderType::Rst | HeaderType::WndInc => {
                if let Err(error) = self.manager.send_to_stream(frame).await {
                    debug!(
                        stream_id = display(stream_id),
                        error = display(error),
                        "error sending to stream, generating rst"
                    );
                    self.raw_output
                        .send(Frame::rst(stream_id, error))
                        .map_err(|_| ErrorType::SessionClosed)
                        .await?;
                } else {
                    self.last_stream_processed = stream_id;
                    if needs_close {
                        if let Ok(handle) = self.manager.lock().await.get_stream(stream_id) {
                            handle.data_write_closed = true;
                        }
                    }
                }
            }
            HeaderType::Invalid(_) => {
                self.raw_output
                    .send(Frame::goaway(
                        self.last_stream_processed,
                        ErrorType::ProtocolError,
                        "invalid frame".into(),
                    ))
                    .map_err(|_| ErrorType::StreamClosed)
                    .await?
            }
            HeaderType::GoAway => {
                if let Body::GoAway { error, .. } = frame.body {
                    self.manager.go_away(error).await;
                    return Err(ErrorType::RemoteGoneAway);
                }

                unreachable!()
            }
        }
        Ok(())
    }

    #[instrument(level = "trace", skip(self))]
    // read task - runs until there are no more frames coming from the remote
    async fn run(mut self) -> Result<(), ErrorType> {
        let _e: Result<(), _> = async {
            loop {
                match self.io.try_next().await {
                    Ok(Some(frame)) => self.handle_frame(frame).await?,
                    Ok(None) | Err(_) => {
                        return Err(ErrorType::SessionClosed);
                    }
                }
            }
        }
        .await;

        self.manager.close_senders().await;

        return Err(ErrorType::SessionClosed);
    }
}

pub trait WriteTask: SessionTask {}
impl<T> WriteTask for T where T: SessionTask {}

struct Writer<W> {
    manager: SharedStreamManager,
    open_reqs: mpsc::Receiver<(usize, oneshot::Sender<Result<Stream, ErrorType>>)>,
    io: W,
}

impl<W> Writer<W>
where
    W: Sink<Frame, Error = io::Error> + Unpin + Send + 'static,
{
    #[instrument(level = "trace", skip(self))]
    async fn run(mut self) -> Result<(), ErrorType> {
        loop {
            select! {
                frame = self.manager.next() => {
                    if let Some(frame) = frame {
                        if let Err(_e) = self.io.send(frame).await {
                            return Err(ErrorType::SessionClosed);
                        }
                    }
                },
                req = self.open_reqs.next() => {
                    if let Some((window, resp)) = req {
                        let (req, stream) = OpenReq::create(window, true);

                        let mut manager = self.manager.lock().await;
                        let res = manager.create_stream(None, req);
                        let _ = resp.send(res.map(move |_| stream));
                    }
                },
                complete => {
                    return Ok(());
                }
            }
        }
    }
}

#[async_trait]
pub trait Session: Send {
    async fn accept(&mut self) -> Option<Stream>;

    async fn open(&mut self) -> Result<Stream, ErrorType>;
}

pub struct Muxado {
    window: usize,
    incoming: mpsc::Receiver<Stream>,
    outgoing: mpsc::Sender<(usize, oneshot::Sender<Result<Stream, ErrorType>>)>,
}

impl Muxado {
    pub async fn accept(&mut self) -> Option<Stream> {
        self.incoming.next().await
    }

    pub async fn open(&mut self) -> Result<Stream, ErrorType> {
        let (resp_tx, resp_rx) = oneshot::channel();

        self.outgoing
            .send((self.window, resp_tx))
            .await
            .map_err(|_| ErrorType::SessionClosed)?;

        resp_rx
            .await
            .map_err(|_| ErrorType::SessionClosed)
            .and_then(|r| r)
    }
}

#[async_trait]
impl Session for Muxado {
    async fn accept(&mut self) -> Option<Stream> {
        Muxado::accept(self).await
    }

    async fn open(&mut self) -> Result<Stream, ErrorType> {
        Muxado::open(self).await
    }
}
