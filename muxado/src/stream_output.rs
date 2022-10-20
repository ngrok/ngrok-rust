use std::{
    pin::Pin,
    sync::{
        atomic::{
            AtomicU32,
            Ordering,
        },
        Arc,
    },
    task::{
        Context,
        Poll,
    },
};

use futures::{
    channel::mpsc::{
        self,
        SendError,
    },
    ready,
    sink::Sink,
};
use pin_utils::unsafe_pinned;
use tracing::instrument;

use crate::{
    errors::ErrorType,
    frame::{
        ErrorCode,
        Frame,
    },
};

pub struct StreamSender {
    sink: mpsc::Sender<Frame>,
    closer: SinkCloser,
}

impl StreamSender {
    unsafe_pinned!(sink: mpsc::Sender<Frame>);

    pub fn wrap(sink: mpsc::Sender<Frame>) -> StreamSender {
        let code = Arc::new(AtomicU32::new(0));
        StreamSender {
            sink,
            closer: SinkCloser { code: code.clone() },
        }
    }

    pub fn closer(&self) -> SinkCloser {
        self.closer.clone()
    }
}

impl Sink<Frame> for StreamSender {
    type Error = ErrorType;

    #[instrument(level = "trace", skip_all)]
    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.closer.check_closed()?;
        Poll::Ready(match ready!(self.as_mut().sink().poll_ready(cx)) {
            Ok(()) => Ok(()),
            Err(_) => {
                // If there was an error here, it means the stream manager got
                // dropped.
                self.closer.close_with(ErrorType::SessionClosed);
                Err(ErrorType::SessionClosed)
            }
        })
    }
    #[instrument(level = "trace", skip(self))]
    fn start_send(mut self: Pin<&mut Self>, item: Frame) -> Result<(), Self::Error> {
        self.closer.check_closed()?;
        match self.as_mut().sink().start_send(item) {
            Ok(()) => Ok(()),
            Err(_) => {
                self.closer.close_with(ErrorType::SessionClosed);
                Err(ErrorType::SessionClosed)
            }
        }
    }
    #[instrument(level = "trace", skip_all)]
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.closer.check_closed()?;
        Poll::Ready(match ready!(self.as_mut().sink().poll_flush(cx)) {
            Ok(()) => Ok(()),
            Err(_) => {
                self.closer.close_with(ErrorType::SessionClosed);
                Err(ErrorType::SessionClosed)
            }
        })
    }

    // Note: This should never actually be called. The stream uses a sentinel
    //       invalid frame to indicate closure rather than closing the channel.
    #[instrument(level = "trace", skip_all)]
    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.closer.check_closed()?;
        // The receiving end of this expects that if the channel looks closed,
        // there are no buffered messages to read. Make sure we're flushed
        // before closing.
        (|| -> Poll<Result<(), SendError>> {
            ready!(self.as_mut().sink().poll_flush(cx))?;
            ready!(self.as_mut().sink().poll_close(cx))?;
            Ok(()).into()
        })()
        .map_ok(|_| self.closer.close_with(ErrorType::StreamClosed))
        .map_err(|_| {
            self.closer.close_with(ErrorType::SessionClosed);
            ErrorType::SessionClosed
        })
    }
}

#[derive(Clone, Debug)]
pub struct SinkCloser {
    code: Arc<AtomicU32>,
}

impl SinkCloser {
    #[instrument(level = "trace")]
    pub fn close_with(&self, ty: ErrorType) {
        // Only store an error if there wasn't already one.
        // Discard the result since we don't really care to return it.
        let _ = self.code.compare_exchange(
            0,
            *ErrorCode::from(ty),
            Ordering::AcqRel,
            Ordering::Relaxed,
        );
    }

    pub fn is_closed(&self) -> bool {
        self.check_closed().is_err()
    }

    pub fn check_closed(&self) -> Result<(), ErrorType> {
        let code = self.code.load(Ordering::Acquire);
        if code != 0 {
            Err(ErrorType::from(ErrorCode::mask(code)))
        } else {
            Ok(())
        }
    }
}
