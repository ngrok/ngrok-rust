use std::{
    collections::HashMap,
    pin::Pin,
    sync::{
        atomic::{
            AtomicBool,
            Ordering,
        },
        Arc,
    },
    task::{
        Context,
        Poll,
        Waker,
    },
};

use futures::{
    channel::mpsc,
    future::poll_fn,
    lock::{
        BiLock,
        BiLockGuard,
    },
    prelude::*,
    ready,
    stream::{
        FusedStream,
        FuturesUnordered,
        Stream as StreamT,
        StreamFuture,
    },
};
use tracing::{
    debug,
    error,
    instrument,
    trace,
};

use crate::{
    errors::Error,
    frame::{
        Body,
        Frame,
        HeaderType,
        StreamID,
    },
    stream::Stream,
    stream_output::*,
};

#[derive(Debug)]
pub struct SharedStreamManager(BiLock<StreamManager>, Arc<AtomicBool>);

#[derive(Clone, Debug)]
pub(crate) struct StreamHandle {
    // Channel to send frames from the remote to the stream.
    pub to_stream: mpsc::Sender<Frame>,

    // Handle to close the stream's frame sink with a code from an `rst` or
    // similar
    pub sink_closer: SinkCloser,

    pub needs_fin: bool,

    // Whether our writer is closed
    pub data_write_closed: bool,

    // Track the bytes in/wndinc out so we can send goaways in the event of a
    // misbehaving remote.
    pub window: usize,
}

type StreamTasks = FuturesUnordered<WithID<StreamFuture<mpsc::Receiver<Frame>>>>;

#[derive(Debug)]
pub struct StreamManager {
    stream_limit: usize,

    streams: HashMap<StreamID, StreamHandle>,
    sys_tx: mpsc::Sender<Frame>,
    sys_rx: mpsc::Receiver<Frame>,
    tasks: StreamTasks,

    last_local_id: StreamID,
    last_remote_id: StreamID,

    gone_away: bool,

    // If we run out of streams to poll, the task collection will be put to
    // sleep. We can't immediately poll it when we add a new stream since that
    // may lose a frame. Instead, the poll_next implementation will store its
    // waker here, and we'll wake it up in create_stream to get it polling
    // again.
    new_streams: Option<Waker>,
}

impl StreamManager {
    fn tasks(&mut self) -> Pin<&mut StreamTasks> {
        Pin::new(&mut self.tasks)
    }

    fn sys_rx(&mut self) -> Pin<&mut mpsc::Receiver<Frame>> {
        Pin::new(&mut self.sys_rx)
    }

    pub fn new(stream_limit: usize, client: bool) -> Self {
        let (sys_tx, sys_rx) = mpsc::channel(512);
        let mut last_local_id = 0;
        let mut last_remote_id = 0;
        if client {
            last_local_id += 1;
        } else {
            last_remote_id += 1;
        }
        StreamManager {
            streams: Default::default(),
            stream_limit,
            sys_tx,
            sys_rx,
            last_local_id: StreamID::clamp(last_local_id),
            last_remote_id: StreamID::clamp(last_remote_id),
            tasks: Default::default(),
            gone_away: false,
            new_streams: None,
        }
    }

    // Split the manager into two shared halves.
    pub fn split(self) -> (SharedStreamManager, SharedStreamManager) {
        let (l, r) = BiLock::new(self);
        let terminated = Arc::new(AtomicBool::new(false));
        (
            SharedStreamManager(l, terminated.clone()),
            SharedStreamManager(r, terminated),
        )
    }

    pub fn go_away(&mut self, error: Error) {
        self.gone_away = true;
        for (_id, handle) in self.streams.drain() {
            handle.sink_closer.close_with(error);
        }
    }

    pub fn sys_sender(&self) -> mpsc::Sender<Frame> {
        self.sys_tx.clone()
    }

    pub fn close_senders(&mut self) {
        for (_, stream) in self.streams.iter_mut() {
            stream.to_stream.close_channel()
        }
    }
}

impl FusedStream for StreamManager {
    fn is_terminated(&self) -> bool {
        self.gone_away
    }
}

impl FusedStream for SharedStreamManager {
    fn is_terminated(&self) -> bool {
        self.1.load(Ordering::SeqCst)
    }
}

// Stream implementation for StreamManager
// This is used as the "output" from all of the streams and will produce frames
// that need to be sent to the remote via the underlying IO stream.
impl StreamT for StreamManager {
    type Item = Frame;

    #[instrument(level = "trace", skip_all)]
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // There will only be no new frames if we've gone away.
        if self.gone_away {
            return Poll::Ready(None);
        }

        // Go ahead and store the latest waker for use by newly started streams.
        // In order to start receiving wakeups from them, we have to ensure that
        // our task collection gets polled here after one is added to it.
        self.new_streams = Some(cx.waker().clone());

        // Handle system frames first, but don't return if it's not ready, or
        // it's somehow closed (shouldn't happen).
        if let Poll::Ready(Some(mut frame)) = self.as_mut().sys_rx().poll_next(cx) {
            if let Body::GoAway {
                ref mut last_stream_id,
                error: _,
                message: _,
            } = &mut frame.body
            {
                *last_stream_id = self.last_remote_id;
                // We won't be sending any more frames from streams.
                self.as_mut().tasks().clear();
                self.as_mut().go_away(Error::SessionClosed);
            }
            return Some(frame).into();
        }

        // Otherwise, get the next frame from a stream.
        let (id, (item, rest)) = if let Some(i) = ready!(self.as_mut().tasks().poll_next(cx)) {
            i
        } else {
            return Poll::Pending;
        };

        let handle = if let Ok(handle) = self.get_stream(id) {
            handle
        } else {
            // We only remove streams when the read/write end is dropped and we
            // get None from it. We don't re-add it it to the future set then,
            // so we can't receive any more frames from it here.
            unreachable!();
        };

        // If the sink is closed and we don't need a fin, don't return a frame.
        // We should never really see a case where we have a closed sink while a
        // fin is needed, but make double sure.
        // The sink closer is only closed from this end if a reset is received
        // or issued. It's only closed from the other end if this end has gone
        // away.
        if handle.sink_closer.is_closed() && !handle.needs_fin {
            debug!(needs_fin = handle.needs_fin, "removing stream without fin");
            self.remove_stream(id);
            cx.waker().wake_by_ref();
            return Poll::Pending;
        }

        let frame = if let Some(frame) = item {
            if let Body::WndInc(inc) = frame.body {
                handle.window += *inc as usize;
            }

            if frame.is_fin() {
                debug!(stream_id = debug(id), "setting needs_fin to false");
                handle.needs_fin = false;
            }

            self.push_task(id, rest);

            frame
        } else {
            // If we got None from the stream, it means its channel is closed
            // because it got dropped on the other end. Maybe generate a fin and
            // remove it from our map.

            // Make sure we haven't already sent a fin for this stream. If we
            // don't even know about the stream, it must have been removed by a
            // remote reset. Don't generate a fin in that case.
            let needs_fin = handle.needs_fin;
            handle.needs_fin = false;
            self.remove_stream(id);
            debug!(needs_fin, "got none from stream, trying to send a fin");
            if needs_fin {
                debug!("removing stream and sending fin");
                Frame::from(Body::Data([][..].into())).fin()
            } else {
                debug!("removing stream that's already fin'd");
                // Could introduce a loop and `continue` here, or we could just
                // return `Pending` and wake ourselves back up.
                cx.waker().wake_by_ref();
                return Poll::Pending;
            }
        }
        .stream_id(id);

        Some(frame).into()
    }
}

impl StreamT for SharedStreamManager {
    type Item = <StreamManager as StreamT>::Item;
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        ready!(self.0.poll_lock(cx)).as_pin_mut().poll_next(cx)
    }
}

impl SharedStreamManager {
    #[instrument(level = "trace", skip(self))]
    pub async fn go_away(&mut self, error: Error) {
        self.1.store(true, Ordering::SeqCst);
        self.lock().await.go_away(error);
    }

    // Send a frame to a stream with the given ID.
    // Should only return an error if the stream is closed, and the caller needs to send a reset.
    #[instrument(level = "trace", skip(self))]
    pub async fn send_to_stream(&mut self, frame: Frame) -> Result<(), Error> {
        let id = frame.header.stream_id;
        let typ = frame.header.typ;
        // If we see data coming in, reduce the stream's window. If it goes
        // below 0, we'll reset the remote with a flow control error.
        let mut shrink_window = if let Body::Data(bs) = &frame.body {
            bs.len()
        } else {
            0
        };

        match frame.body {
            Body::GoAway { .. } | Body::Invalid { .. } => {
                error!(
                    body = ?frame,
                    id = %id,
                    "attempt to send invalid frame type to stream",
                );
                return Err(Error::Internal);
            }
            _ => {}
        }

        let is_fin = frame.is_fin() && frame.body.len() == 0;

        let mut frame = Some(frame);

        let mut handle_frame = |handle: &mut StreamHandle, cx: &mut Context| {
            if typ == HeaderType::Data && handle.data_write_closed {
                debug!("attempt to send data on closed stream");
                return Err(Error::StreamClosed).into();
            }

            // Don't send resets to the stream, just close its channel with the
            // error.
            if let Some(Frame {
                body: Body::Rst(err),
                ..
            }) = frame
            {
                debug!(
                    stream_id = display(id),
                    error = display(err),
                    "received rst from remote, closing stream"
                );
                // Close the writer on the other end, mark *our* writer as
                // closed, and disable fin generation.
                handle.sink_closer.close_with(err);
                handle.data_write_closed = true;
                handle.needs_fin = false;
                return Ok(()).into();
            }

            // Keep track of how much data has been sent to the stream. If it
            // goes over, send a reset.
            if shrink_window <= handle.window {
                handle.window -= shrink_window;
                // We're polling this function, so we need to avoid shrinking
                // more than once.
                shrink_window = 0;
            } else {
                debug!(
                    frame_size = shrink_window,
                    stream_window = handle.window,
                    "remote violated flow control"
                );
                return Err(Error::FlowControl).into();
            }

            let sink = &mut handle.to_stream;
            trace!("checking stream for readiness");
            ready!(sink.poll_ready(cx))
                .and_then(|_| sink.start_send(frame.take().unwrap()))
                .map_err(|_| Error::StreamClosed)
                .or_else(|res| if is_fin { Ok(()) } else { Err(res) })?;
            Ok(()).into()
        };

        // The rest of this is in a `poll_fn` so that we don't hold the lock for
        // any longer than necessary to check if the stream is ready. If we did
        // it await-style, we'd continue holding the lock even if the stream was
        // still pending.
        poll_fn(move |cx| -> Poll<Result<_, Error>> {
            // Lock self, look up the stream. If it doesn't exist, return the
            // error.
            let mut lock = ready!(self.0.poll_lock(cx));
            let mut handle = match lock.get_stream(id) {
                Ok(handle) => handle,
                Err(_e) if HeaderType::Data != typ || is_fin => {
                    return Ok(()).into();
                }
                Err(e) => return Err(e).into(),
            };

            let res = ready!(handle_frame(handle, cx));

            // Any errors from data frames should cause a reset to be sent by
            // the session.
            if HeaderType::Data == typ && !is_fin {
                // If we're sending a reset, close all the writers to prevent
                // any more frames from being sent.
                if let Err(e) = res {
                    debug!(error = display(e), "error handling frame");
                    handle.sink_closer.close_with(Error::StreamClosed);
                    handle.data_write_closed = true;
                    handle.needs_fin = false;
                }
                res.into()
            } else {
                Ok(()).into()
            }
        })
        .await
    }

    pub async fn close_senders(&mut self) {
        self.lock().await.close_senders()
    }

    pub async fn lock(&mut self) -> BiLockGuard<'_, StreamManager> {
        self.0.lock().await
    }
}

pub struct OpenReq {
    channel: (mpsc::Sender<Frame>, mpsc::Receiver<Frame>),
    closer: SinkCloser,
    window: usize,
}

impl OpenReq {
    pub fn create(window: usize, needs_syn: bool) -> (OpenReq, Stream) {
        let (to_stream, from_session) = mpsc::channel(window);
        let (to_session, from_stream) = mpsc::channel(window);
        let to_session = StreamSender::wrap(to_session);
        let req = OpenReq {
            channel: (to_stream, from_stream),
            closer: to_session.closer(),
            window,
        };
        let stream = Stream::new(to_session, from_session, window, needs_syn);
        (req, stream)
    }
}

impl StreamManager {
    #[instrument(level = "trace", skip(self))]
    pub(crate) fn get_stream(&mut self, id: StreamID) -> Result<&mut StreamHandle, Error> {
        if let Some(handle) = self.streams.get_mut(&id) {
            Ok(handle)
        } else {
            trace!("stream not found");
            Err(Error::StreamClosed)
        }
    }

    #[instrument(level = "trace", skip(self, req))]
    pub fn create_stream(&mut self, id: Option<StreamID>, req: OpenReq) -> Result<StreamID, Error> {
        // Only return an error if we're at the stream limit.
        if self.streams.len() == self.stream_limit {
            return Err(Error::StreamsExhausted);
        }

        let (to_stream, from_stream) = req.channel;
        let closer = req.closer;
        let window = req.window;
        let id = if let Some(remote_id) = id {
            self.last_remote_id = remote_id;
            remote_id
        } else {
            let new_id = StreamID::clamp(*self.last_local_id + 2);
            self.last_local_id = new_id;
            new_id
        };
        self.streams.insert(
            id,
            StreamHandle {
                window,
                to_stream,
                sink_closer: closer,
                needs_fin: true,
                data_write_closed: false,
            },
        );
        self.push_task(id, from_stream);
        // wake up the main stream if it put itself to sleep.
        if let Some(w) = self.new_streams.take() {
            w.wake()
        }
        Ok(id)
    }

    fn push_task(&mut self, id: StreamID, recv: mpsc::Receiver<Frame>) {
        self.tasks.push(recv.into_future().with_id(id));
    }

    #[instrument(level = "debug", skip(self))]
    fn remove_stream(&mut self, id: StreamID) -> Option<StreamHandle> {
        self.streams.remove(&id)
    }
}

struct WithID<F: ?Sized> {
    id: StreamID,
    fut: F,
}

impl<F: Unpin> WithID<F> {
    fn fut(&mut self) -> Pin<&mut F> {
        Pin::new(&mut self.fut)
    }
}

impl<F> Future for WithID<F>
where
    F: Future + Unpin,
{
    type Output = (StreamID, F::Output);

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let out = ready!(self.as_mut().fut().poll(cx));
        Poll::Ready((self.id, out))
    }
}

trait WithIDExt {
    fn with_id(self, id: StreamID) -> WithID<Self>;
}

impl<F> WithIDExt for F
where
    F: Future,
{
    fn with_id(self, id: StreamID) -> WithID<Self> {
        WithID { id, fut: self }
    }
}
