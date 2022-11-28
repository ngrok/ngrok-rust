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

/// Builder for a muxado session.
/// Should probably leave this alone unless you're sure you know what you're
/// doing.
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
    /// Start building a new muxado session using the provided IO stream.
    pub fn new(io_stream: S) -> Self {
        SessionBuilder {
            io_stream,
            window: DEFAULT_WINDOW,
            accept_queue_size: DEFAULT_ACCEPT,
            stream_limit: DEFAULT_STREAMS,
            client: true,
        }
    }

    /// Set the stream window size.
    /// Defaults to 256kb.
    pub fn with_window_size(mut self, size: usize) -> Self {
        self.window = size;
        self
    }

    /// Set the accept queue size.
    /// This is the size of the channel that will hold "open stream" requests
    /// from the remote. If [`AcceptStream::accept`] isn't called and the
    /// channel fills up, the session will block.
    /// Defaults to 64.
    pub fn with_accept_queue_size(mut self, size: usize) -> Self {
        self.accept_queue_size = size;
        self
    }

    /// Set the maximum number of streams allowed at a given time.
    /// If this limit is reached, new streams will be refused.
    /// Defaults to 512.
    pub fn with_stream_limit(mut self, count: usize) -> Self {
        self.stream_limit = count;
        self
    }

    /// Set this session to act as a client.
    pub fn client(mut self) -> Self {
        self.client = true;
        self
    }

    /// Set this session to act as a server.
    pub fn server(mut self) -> Self {
        self.client = false;
        self
    }

    /// Start a muxado session with the current options.
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
        let sys_tx = manager.sys_sender();
        let (m1, m2) = manager.split();

        let (io_tx, io_rx) = Framed::new(io_stream, FrameCodec::default()).split();

        let read_task = Reader {
            io: io_rx,
            accept_tx,
            window,
            manager: m1,
            last_stream_processed: StreamID::clamp(0),
            sys_tx,
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

// read task - runs until there are no more frames coming from the remote
// Reads frames from the underlying stream and forwards them to the stream
// manager.
struct Reader<R> {
    io: R,
    sys_tx: mpsc::Sender<Frame>,
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
            // These frame types are stream-specific
            HeaderType::Data | HeaderType::Rst | HeaderType::WndInc => {
                if let Err(error) = self.manager.send_to_stream(frame).await {
                    debug!(
                        stream_id = display(stream_id),
                        error = display(error),
                        "error sending to stream, generating rst"
                    );
                    self.sys_tx
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

            // GoAway is a system-level frame, so send it along the special
            // system channel.
            HeaderType::GoAway => {
                if let Body::GoAway { error, .. } = frame.body {
                    self.manager.go_away(error).await;
                    return Err(ErrorType::RemoteGoneAway);
                }

                unreachable!()
            }
            HeaderType::Invalid(_) => {
                self.sys_tx
                    .send(Frame::goaway(
                        self.last_stream_processed,
                        ErrorType::ProtocolError,
                        "invalid frame".into(),
                    ))
                    .map_err(|_| ErrorType::StreamClosed)
                    .await?
            }
        }
        Ok(())
    }

    // The actual read/process loop
    #[instrument(level = "trace", skip(self))]
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

        Err(ErrorType::SessionClosed)
    }
}

// The writer task responsible for receiving frames from streams or open
// requests and writing them to the underlying stream.
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
                // If a request for a new stream originated locally, tell the
                // stream manager to create it. The first dataframe from it will
                // have the SYN flag set.
                req = self.open_reqs.next() => {
                    if let Some(resp_tx) = req {
                        let (req, stream) = OpenReq::create(self.window, true);

                        let mut manager = self.manager.lock().await;
                        let res = manager.create_stream(None, req);
                        let _ = resp_tx.send(res.map(move |_| stream));
                    }
                },
                // All senders have been dropped - exit.
                complete => {
                    return Ok(());
                }
            }
        }
    }
}

/// A muxado session.
/// Can be used directly to open and accept streams, or split into dedicated
/// open/accept parts.
pub trait Session: AcceptStream + OpenStream {
    /// The open half of the session.
    type Open: OpenStream;
    /// The accept half of the session.
    type Accept: AcceptStream;
    /// Split the session into dedicated open/accept components.
    fn split(self) -> (Self::Open, Self::Accept);
}

#[async_trait]
pub trait AcceptStream {
    /// Accept an incoming stream that was opened by the remote.
    async fn accept(&mut self) -> Option<Stream>;
}

#[async_trait]
pub trait OpenStream {
    /// Open a new stream.
    async fn open(&mut self) -> Result<Stream, ErrorType>;
}

/// The [`OpenStream`] half of a muxado session.
pub struct MuxadoOpen(mpsc::Sender<oneshot::Sender<Result<Stream, ErrorType>>>);
/// The [`AcceptStream`] half of a muxado session.
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
