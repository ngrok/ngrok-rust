use std::{
    io,
    sync::{
        Arc,
        atomic::{
            AtomicBool,
            Ordering,
        },
    },
};

use async_trait::async_trait;
use futures::{
    SinkExt,
    channel::{
        mpsc,
        oneshot,
    },
    prelude::*,
    select,
    select_biased,
    stream::{
        FusedStream,
        StreamExt,
    },
};
use tokio::io::{
    AsyncRead,
    AsyncWrite,
};
use tokio_util::{
    codec::Framed,
    sync::CancellationToken,
};
use tracing::{
    Instrument,
    debug,
    debug_span,
    instrument,
    trace,
};

use crate::{
    codec::FrameCodec,
    errors::Error,
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
///
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
    pub fn window_size(mut self, size: usize) -> Self {
        self.window = size;
        self
    }

    /// Set the accept queue size.
    /// This is the size of the channel that will hold "open stream" requests
    /// from the remote. If [Accept::accept] isn't called and the
    /// channel fills up, the session will block.
    /// Defaults to 64.
    pub fn accept_queue_size(mut self, size: usize) -> Self {
        self.accept_queue_size = size;
        self
    }

    /// Set the maximum number of streams allowed at a given time.
    /// If this limit is reached, new streams will be refused.
    /// Defaults to 512.
    pub fn stream_limit(mut self, count: usize) -> Self {
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
    pub fn start(self) -> MuxadoSession {
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

        let shutdown = CancellationToken::new();

        let read_task = Reader {
            shutdown: shutdown.clone(),
            io: io_rx,
            accept_tx,
            window,
            manager: m1,
            last_stream_processed: StreamID::clamp(0),
            sys_tx: sys_tx.clone(),
        };

        let write_task = Writer {
            shutdown: shutdown.clone(),
            window,
            io: io_tx,
            manager: m2,
            open_reqs: open_rx,
        };

        let (dropref, waiter) = awaitdrop::awaitdrop();

        tokio::spawn(
            futures::future::select(
                async move {
                    let result = read_task.run().await;
                    debug!(?result, "read_task exited");
                }
                .boxed(),
                waiter.wait(),
            )
            .instrument(debug_span!("read_task")),
        );
        tokio::spawn(
            futures::future::select(
                async move {
                    let result = write_task.run().await;
                    debug!(?result, "write_task exited");
                }
                .boxed(),
                waiter.wait(),
            )
            .instrument(debug_span!("write_task")),
        );

        MuxadoSession {
            incoming: MuxadoAccept(dropref.clone(), accept_rx),
            outgoing: MuxadoOpen {
                dropref,
                open_tx,
                sys_tx,
                closed: AtomicBool::from(false).into(),
                shutdown: ShutdownHandle(shutdown),
            },
        }
    }
}

// read task - runs until there are no more frames coming from the remote
// Reads frames from the underlying stream and forwards them to the stream
// manager.
struct Reader<R> {
    shutdown: CancellationToken,
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
    /// Handle an incoming frame from the remote
    #[instrument(level = "trace", skip(self))]
    async fn handle_frame(&mut self, frame: Frame) -> Result<(), Error> {
        // If the remote sent a syn, create a new stream and add it to the accept channel.
        if frame.is_syn() {
            let (req, stream) = OpenReq::create(self.window, false);
            self.manager
                .lock()
                .await
                .create_stream(frame.header.stream_id.into(), req)?;
            self.accept_tx
                .send(stream)
                .map_err(|_| Error::SessionClosed)
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
                    // If the stream manager couldn't send this frame to the
                    // stream for some reason, generate an RST to tell the other
                    // end to stop sending on this stream.
                    debug!(
                        stream_id = display(stream_id),
                        error = display(error),
                        "error sending to stream, generating rst"
                    );
                    self.sys_tx
                        .send(Frame::rst(stream_id, error))
                        .map_err(|_| Error::SessionClosed)
                        .await?;
                } else {
                    self.last_stream_processed = stream_id;
                    if needs_close
                        && let Ok(handle) = self.manager.lock().await.get_stream(stream_id)
                    {
                        handle.data_write_closed = true;
                    }
                }
            }

            // GoAway is a system-level frame, so send it along the special
            // system channel.
            HeaderType::GoAway => {
                if let Body::GoAway { error, .. } = frame.body {
                    self.manager.go_away(error).await;
                    return Err(Error::RemoteGoneAway);
                }

                unreachable!()
            }
            HeaderType::Invalid(_) => {
                self.sys_tx
                    .send(Frame::goaway(
                        self.last_stream_processed,
                        Error::Protocol,
                        "invalid frame".into(),
                    ))
                    .map_err(|_| Error::StreamClosed)
                    .await?
            }
        }
        Ok(())
    }

    // The actual read/process loop
    async fn run(mut self) -> Result<(), Error> {
        let shutdown = self.shutdown.clone();
        let res: Result<(), _> = select! {
            res = async {
                loop {
                    match self.io.try_next().await {
                        Ok(Some(frame)) => {
                            trace!(?frame, "received frame from remote");
                            self.handle_frame(frame).await?
                        }
                        Ok(None) | Err(_) => {
                            return Err(Error::SessionClosed);
                        }
                    }
                }
            }.fuse() => res,
            _ = shutdown.cancelled().fuse() => Err(Error::SessionClosed),
        };
        debug!(?res, "read loop exited");

        if shutdown.is_cancelled() {
            debug!("session shut down locally");
            // Fail every open stream so that readers and writers on them
            // don't hang on the dead transport.
            self.manager.go_away(Error::SessionClosed).await;
            return Err(Error::SessionClosed);
        }

        self.manager.close_senders().await;

        Err(Error::SessionClosed)
    }
}

// How long the writer may take to finish sending a frame it had already
// started on when the session was forcibly shut down.
const SHUTDOWN_GRACE: std::time::Duration = std::time::Duration::from_millis(250);

// The writer task responsible for receiving frames from streams or open
// requests and writing them to the underlying stream.
struct Writer<W> {
    shutdown: CancellationToken,
    manager: SharedStreamManager,
    window: usize,
    open_reqs: mpsc::Receiver<oneshot::Sender<Result<Stream, Error>>>,
    io: W,
}

impl<W> Writer<W>
where
    W: Sink<Frame, Error = io::Error> + Unpin + Send + 'static,
{
    async fn run(mut self) -> Result<(), Error> {
        let shutdown = self.shutdown.clone();
        loop {
            // After a forced shutdown, send nothing more and open no more
            // streams. Exiting drops the transport and fails queued opens.
            if shutdown.is_cancelled() {
                return Err(Error::SessionClosed);
            }
            // All senders have been dropped - exit.
            if self.manager.is_terminated() && self.open_reqs.is_terminated() {
                return Ok(());
            }
            // Biased so that a shutdown always wins over pending work: neither
            // a new frame nor a new stream is started once it's requested.
            // Opens come before frames: busy streams can have a frame ready
            // every time round, which would starve opens, while opens are
            // few and can't starve frames.
            select_biased! {
                _ = shutdown.cancelled().fuse() => {},
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
                // The stream manager produced a frame that needs to be sent to
                // the remote.
                frame = self.manager.next() => {
                    if let Some(frame) = frame {
                        let is_goaway = matches!(frame.header.typ, HeaderType::GoAway);
                        trace!(?frame, "sending frame to remote");
                        let send = self.io.send(frame).fuse();
                        futures::pin_mut!(send);
                        let sent = select! {
                            res = send => res.is_ok(),
                            // Give a frame that's already being sent, such as
                            // the GOAWAY from a graceful close, a moment to
                            // finish, as Go does. A dead transport can't hold
                            // up shutdown for longer than this.
                            _ = shutdown.cancelled().fuse() => matches!(
                                tokio::time::timeout(SHUTDOWN_GRACE, send).await,
                                Ok(Ok(()))
                            ),
                        };
                        if !sent {
                            return Err(Error::SessionClosed);
                        }
                        if is_goaway {
                            return Ok(())
                        }
                    }
                },
            }
        }
    }
}

/// A muxado session.
///
/// Can be used directly to open and accept streams, or split into dedicated
/// open/accept parts.
pub trait Session: Accept + OpenClose {
    /// The open half of the session.
    type OpenClose: OpenClose;
    /// The accept half of the session.
    type Accept: Accept;
    /// Split the session into dedicated open/accept components.
    fn split(self) -> (Self::OpenClose, Self::Accept);

    /// Get a handle that can forcibly shut down this session, if supported.
    ///
    /// Wrappers around a [Session] should forward this to the wrapped
    /// session, or heartbeat timeouts can't close the underlying transport.
    fn shutdown_handle(&self) -> Option<ShutdownHandle> {
        None
    }
}

/// A handle that forcibly shuts down a muxado session.
///
/// Unlike [OpenClose::close], which only queues a GOAWAY for the remote, this
/// stops the session's read and write tasks, fails all open streams and
/// pending opens, causes [Accept::accept] to return `None`, and drops the
/// underlying transport. This happens immediately, except that a frame that's
/// already being written gets up to 250 ms to finish. It's intended for when
/// the remote is known to be unresponsive, such as after a missed heartbeat.
#[derive(Clone, Debug)]
pub struct ShutdownHandle(CancellationToken);

impl ShutdownHandle {
    /// Shut down the session. Idempotent.
    pub fn shutdown(&self) {
        self.0.cancel();
    }

    /// Returns true if the session has been shut down via this handle.
    pub fn is_shutdown(&self) -> bool {
        self.0.is_cancelled()
    }
}

/// Trait for accepting incoming streams in a muxado [Session].
#[async_trait]
pub trait Accept {
    /// Accept an incoming stream that was opened by the remote.
    async fn accept(&mut self) -> Option<Stream>;
}

/// Trait for opening new streams in a muxado [Session].
#[async_trait]
pub trait OpenClose {
    /// Open a new stream.
    async fn open(&mut self) -> Result<Stream, Error>;
    /// Close the session by sending a GOAWAY
    async fn close(&mut self, error: Error, msg: String) -> Result<(), Error>;
}

/// The [Open] half of a muxado session.
#[derive(Clone)]
pub struct MuxadoOpen {
    dropref: awaitdrop::Ref,
    open_tx: mpsc::Sender<oneshot::Sender<Result<Stream, Error>>>,
    sys_tx: mpsc::Sender<Frame>,
    closed: Arc<AtomicBool>,
    shutdown: ShutdownHandle,
}

/// The [Accept] half of a muxado session.
pub struct MuxadoAccept(#[allow(dead_code)] awaitdrop::Ref, mpsc::Receiver<Stream>);

#[async_trait]
impl Accept for MuxadoAccept {
    async fn accept(&mut self) -> Option<Stream> {
        self.1.next().await
    }
}

#[async_trait]
impl OpenClose for MuxadoOpen {
    async fn open(&mut self) -> Result<Stream, Error> {
        if self.closed.load(Ordering::SeqCst) || self.shutdown.is_shutdown() {
            return Err(Error::SessionClosed);
        }
        let (resp_tx, resp_rx) = oneshot::channel();

        self.open_tx
            .send(resp_tx)
            .await
            .map_err(|_| Error::SessionClosed)?;

        let mut res = resp_rx
            .await
            .map_err(|_| Error::SessionClosed)
            .and_then(|r| r);

        if let Ok(stream) = &mut res {
            stream.dropref = self.dropref.clone().into();
        }

        res
    }

    async fn close(&mut self, error: Error, msg: String) -> Result<(), Error> {
        let res = self
            .sys_tx
            .send(Frame::goaway(
                StreamID::clamp(0),
                error,
                msg.into_bytes().into(),
            ))
            .await
            .map_err(|_| Error::SessionClosed);
        self.closed.store(true, Ordering::SeqCst);
        res
    }
}

/// The base muxado [Session] implementation.
///
/// See the [Session], [Accept], and [Open] trait implementations for
/// available methods.
pub struct MuxadoSession {
    incoming: MuxadoAccept,
    outgoing: MuxadoOpen,
}

#[async_trait]
impl Accept for MuxadoSession {
    async fn accept(&mut self) -> Option<Stream> {
        self.incoming.accept().await
    }
}

#[async_trait]
impl OpenClose for MuxadoSession {
    async fn open(&mut self) -> Result<Stream, Error> {
        self.outgoing.open().await
    }

    async fn close(&mut self, error: Error, msg: String) -> Result<(), Error> {
        self.outgoing.close(error, msg).await
    }
}

impl Session for MuxadoSession {
    type Accept = MuxadoAccept;
    type OpenClose = MuxadoOpen;
    fn split(self) -> (Self::OpenClose, Self::Accept) {
        (self.outgoing, self.incoming)
    }

    fn shutdown_handle(&self) -> Option<ShutdownHandle> {
        Some(self.outgoing.shutdown.clone())
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

    /// A forced shutdown of an idle session drops the transport at once,
    /// without waiting out the writer's grace period, and fails opens.
    #[tokio::test(start_paused = true)]
    async fn shutdown_drops_idle_transport_at_once() {
        let (left, right) = io::duplex(512);
        let mut server = SessionBuilder::new(left).server().start();
        let mut client = SessionBuilder::new(right).client().start();
        let handle = client.shutdown_handle().expect("muxado sessions shut down");

        let start = tokio::time::Instant::now();
        handle.shutdown();
        assert!(handle.is_shutdown());
        assert!(
            server.accept().await.is_none(),
            "the remote should see the transport close"
        );
        assert_eq!(start.elapsed(), std::time::Duration::ZERO);

        assert!(matches!(client.open().await, Err(Error::SessionClosed)));
        assert!(client.accept().await.is_none());
    }

    /// An open that races a forced shutdown fails rather than returning a
    /// stream on the dead session.
    #[tokio::test(start_paused = true)]
    async fn open_racing_shutdown_fails() {
        // Were the writer to pick among its ready branches at random, it
        // would sometimes take the open first. Repeat so that that's caught.
        for _ in 0..100 {
            let (left, right) = io::duplex(512);
            let _server = SessionBuilder::new(left).server().start();
            let client = SessionBuilder::new(right).client().start();
            let handle = client.shutdown_handle().expect("muxado sessions shut down");
            let (open, _accept) = client.split();
            // Let the writer start waiting for work.
            tokio::task::yield_now().await;

            // Queue an open request as an open that passed its shutdown
            // check just before the shutdown would.
            let (resp_tx, resp_rx) = oneshot::channel();
            open.open_tx.clone().send(resp_tx).await.unwrap();
            handle.shutdown();

            let res = tokio::time::timeout(std::time::Duration::from_secs(1), resp_rx)
                .await
                .expect("the open should be answered");
            assert!(
                !matches!(res, Ok(Ok(_))),
                "the open should fail on a session that was shut down"
            );
        }
    }

    /// A writer stuck sending a frame on a transport the remote has stopped
    /// reading gets the grace period to finish, then gives up and drops the
    /// transport. A stream write blocked on the remote's window fails.
    #[tokio::test(start_paused = true)]
    async fn shutdown_gives_stalled_writer_the_grace_period() {
        // The remote holds its end of the transport but never reads it.
        let (mut left, right) = io::duplex(64);
        let mut client = SessionBuilder::new(right)
            .client()
            .window_size(1024)
            .start();
        let handle = client.shutdown_handle().expect("muxado sessions shut down");

        // More than the window, so the write blocks once it's used up, and
        // more than the transport buffers, so the writer stalls mid-send.
        let mut stream = client.open().await.expect("open stream");
        let write = tokio::spawn(async move { stream.write_all(&[0u8; 4096]).await });
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        assert!(!write.is_finished(), "the write should be blocked");

        let start = tokio::time::Instant::now();
        handle.shutdown();

        let res = tokio::time::timeout(std::time::Duration::from_secs(1), write)
            .await
            .expect("the blocked write should fail on shutdown")
            .unwrap();
        assert!(res.is_err(), "the blocked write should fail: {res:?}");

        // Writing to the remote end fails once the session has dropped the
        // transport. Nothing reads what's written, so this blocks until then.
        let res = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            left.write_all(&[0u8; 4096]),
        )
        .await
        .expect("the stalled writer should give up");
        assert!(res.is_err(), "the transport should be dropped");
        assert_eq!(start.elapsed(), SHUTDOWN_GRACE);
    }

    /// Opens aren't starved by a steady stream of outgoing frames, as when
    /// the link, not the flow-control window, limits how fast streams send.
    ///
    /// This runs in real time on several threads: the writers must refill the
    /// frame queue while the session's writer is sending, which paused time
    /// on a single thread doesn't do.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn open_not_starved_by_busy_streams() {
        // Two transports joined by a pump that slows the client-to-server
        // direction, so the link is the bottleneck.
        let (server_io, pump_server) = io::duplex(4096);
        let (pump_client, client_io) = io::duplex(4096);
        let (mut from_server, mut to_server) = io::split(pump_server);
        let (mut from_client, mut to_client) = io::split(pump_client);
        tokio::spawn(async move {
            let mut buf = [0u8; 2048];
            loop {
                let n = match from_client.read(&mut buf).await {
                    Ok(0) | Err(_) => return,
                    Ok(n) => n,
                };
                if to_server.write_all(&buf[..n]).await.is_err() {
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_micros(200)).await;
            }
        });
        tokio::spawn(async move {
            let _ = io::copy(&mut from_server, &mut to_client).await;
        });

        // The server reads as fast as data arrives, so windows stay open.
        let mut server = SessionBuilder::new(server_io).server().start();
        tokio::spawn(async move {
            while let Some(mut stream) = server.accept().await {
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 65536];
                    while let Ok(n) = stream.read(&mut buf).await
                        && n > 0
                    {}
                });
            }
        });

        let mut client = SessionBuilder::new(client_io).client().start();
        for i in 0..5 {
            let mut stream = tokio::time::timeout(std::time::Duration::from_secs(5), client.open())
                .await
                .unwrap_or_else(|_| panic!("open {i} waited behind other streams' frames"))
                .expect("open stream");
            tokio::spawn(async move {
                let buf = vec![0u8; 16384];
                while stream.write_all(&buf).await.is_ok() {}
            });
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }
}
