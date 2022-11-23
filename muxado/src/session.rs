use std::io;

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

pub struct SessionBuilder<S> {
    io_stream: S,
    window: usize,
    accept_queue_size: usize,
    stream_limit: usize,
    client: bool,
}

impl<S> SessionBuilder<S>
where
    S: AsyncRead + AsyncWrite + Send + 'static,
{
    pub fn new(io_stream: S) -> Self {
        SessionBuilder {
            io_stream: io_stream.into(),
            window: DEFAULT_WINDOW,
            accept_queue_size: DEFAULT_ACCEPT,
            stream_limit: DEFAULT_STREAMS,
            client: true,
        }
    }

    pub fn with_stream(mut self, io_stream: S) -> Self {
        self.io_stream = io_stream;
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

    pub fn start(self) -> Muxado {
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
        let raw_tx = manager.sys_sender();
        let (m1, m2) = manager.split();
        let (io_tx, io_rx) = Framed::new(io_stream, FrameCodec::default()).split();

        let read_task = Reader {
            io: io_rx,
            accept_tx,
            window,
            manager: m1,
            last_stream_processed: StreamID::clamp(0),
            raw_tx,
        };

        let write_task = Writer {
            window,
            io: io_tx,
            manager: m2,
            open_reqs: open_rx,
        };

        tokio::spawn(read_task.run());
        tokio::spawn(write_task.run());

        Muxado {
            incoming: MuxadoAccept(accept_rx),
            outgoing: MuxadoOpen(open_tx),
        }
    }
}

struct Reader<R> {
    io: R,
    raw_tx: mpsc::Sender<Frame>,
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
                    self.raw_tx
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
                self.raw_tx
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

struct Writer<W> {
    manager: SharedStreamManager,
    window: usize,
    open_reqs: mpsc::Receiver<oneshot::Sender<Result<Stream, ErrorType>>>,
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
                    if let Some(resp_tx) = req {
                        let (req, stream) = OpenReq::create(self.window, true);

                        let mut manager = self.manager.lock().await;
                        let res = manager.create_stream(None, req);
                        let _ = resp_tx.send(res.map(move |_| stream));
                    }
                },
                complete => {
                    return Ok(());
                }
            }
        }
    }
}

pub trait Session: AcceptStream + OpenStream {
    type Open: OpenStream;
    type Accept: AcceptStream;
    fn split(self) -> (Self::Open, Self::Accept);
}

#[async_trait]
pub trait AcceptStream {
    async fn accept(&mut self) -> Option<Stream>;
}

#[async_trait]
pub trait OpenStream {
    async fn open(&mut self) -> Result<Stream, ErrorType>;
}

pub struct MuxadoOpen(mpsc::Sender<oneshot::Sender<Result<Stream, ErrorType>>>);
pub struct MuxadoAccept(mpsc::Receiver<Stream>);

#[async_trait]
impl AcceptStream for MuxadoAccept {
    async fn accept(&mut self) -> Option<Stream> {
        self.0.next().await
    }
}

#[async_trait]
impl OpenStream for MuxadoOpen {
    async fn open(&mut self) -> Result<Stream, ErrorType> {
        let (resp_tx, resp_rx) = oneshot::channel();

        self.0
            .send(resp_tx)
            .await
            .map_err(|_| ErrorType::SessionClosed)?;

        resp_rx
            .await
            .map_err(|_| ErrorType::SessionClosed)
            .and_then(|r| r)
    }
}

pub struct Muxado {
    incoming: MuxadoAccept,
    outgoing: MuxadoOpen,
}

#[async_trait]
impl AcceptStream for Muxado {
    async fn accept(&mut self) -> Option<Stream> {
        self.incoming.accept().await
    }
}

#[async_trait]
impl OpenStream for Muxado {
    async fn open(&mut self) -> Result<Stream, ErrorType> {
        self.outgoing.open().await
    }
}

impl Session for Muxado {
    type Accept = MuxadoAccept;
    type Open = MuxadoOpen;
    fn split(self) -> (Self::Open, Self::Accept) {
        (self.outgoing, self.incoming)
    }
}

#[cfg(test)]
mod test {
    use tokio::io::{
        self,
        AsyncReadExt,
        AsyncWriteExt,
    };

    use super::*;
    #[tokio::test]
    async fn test_session() {
        let (left, right) = io::duplex(512);
        let mut server = SessionBuilder::new(left).server().start();
        let mut client = SessionBuilder::new(right).client().start();

        tokio::spawn(async move {
            let mut stream = server.accept().await.expect("accept stream");
            let mut buf = Vec::new();
            stream.read_to_end(&mut buf).await.expect("read stream");
            drop(stream);
            let mut stream = server.open().await.expect("open stream");
            stream.write_all(&buf).await.expect("write to stream");
        });

        let mut stream = client.open().await.expect("open stream");
        stream
            .write_all(b"Hello, world!")
            .await
            .expect("write to stream");
        drop(stream);

        let mut stream = client.accept().await.expect("accept stream");
        let mut buf = Vec::new();
        stream
            .read_to_end(&mut buf)
            .await
            .expect("read from stream");

        assert_eq!(b"Hello, world!", &*buf,);
    }
}
