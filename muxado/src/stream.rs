use std::{
    cmp,
    fmt,
    io,
    pin::Pin,
    task::{
        Context,
        Poll,
        Waker,
    },
};

use bytes::BytesMut;
use futures::{
    channel::mpsc,
    ready,
    sink::Sink,
    stream::Stream as StreamT,
};
use pin_project::pin_project;
use tokio::io::{
    AsyncRead,
    AsyncWrite,
    ReadBuf,
};
use tracing::instrument;

use crate::{
    errors::Error,
    frame::{
        Body,
        Frame,
        HeaderType,
        Length,
        WndInc,
    },
    stream_output::StreamSender,
    window::Window,
};

/// A muxado stream.
///
/// This is an [AsyncRead]/[AsyncWrite] struct that's backed by a muxado
/// session.
#[pin_project(project = StreamProj, PinnedDrop)]
pub struct Stream {
    pub(crate) dropref: Option<awaitdrop::Ref>,

    window: Window,

    read_buf: BytesMut,

    // These are the two channels that are used to shuttle data back and forth
    // between the stream and the stream manager, which is responsible for
    // routing frames to their proper stream.
    #[pin]
    fin: mpsc::Receiver<Frame>,
    #[pin]
    fout: StreamSender,

    read_waker: Option<Waker>,
    write_waker: Option<Waker>,

    write_closed: Option<Error>,

    data_read_closed: bool,

    needs_syn: bool,
}

impl fmt::Debug for Stream {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Stream")
            .field("window", &self.window)
            .field("read_buf", &self.read_buf)
            .field("read_waker", &self.read_waker)
            .field("write_waker", &self.write_waker)
            .field("reset", &self.write_closed)
            .field("read_closed", &self.data_read_closed)
            .finish()
    }
}

impl Stream {
    pub(crate) fn new(
        fout: StreamSender,
        fin: mpsc::Receiver<Frame>,
        window_size: usize,
        needs_syn: bool,
    ) -> Self {
        Self {
            dropref: None,
            window: Window::new(window_size),
            fin,
            fout,
            read_buf: Default::default(),
            read_waker: Default::default(),
            write_waker: Default::default(),
            write_closed: Default::default(),
            data_read_closed: false,
            needs_syn,
        }
    }

    #[instrument(level = "trace", skip_all)]
    fn poll_recv_frame(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Frame>> {
        let mut this = self.project();
        let fin = this.fin.as_mut();
        fin.poll_next(cx)
    }

    // Receive data and fill the read buffer.
    // Handle frames of any other type along the way.
    // Returns `Poll::Ready` once there are new bytes to read, or EOF/RST has
    // been reached.
    #[instrument(level = "trace", skip_all)]
    fn poll_recv_data(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.poll_recv_frame_type(cx, HeaderType::Data)
    }

    #[instrument(level = "trace", skip_all)]
    fn poll_recv_wndinc(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.poll_recv_frame_type(cx, HeaderType::WndInc)
    }

    #[instrument(level = "trace", skip(self, cx))]
    fn poll_recv_frame_type(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        target_typ: HeaderType,
    ) -> Poll<io::Result<()>> {
        loop {
            let frame = if let Some(frame) = ready!(self.as_mut().poll_recv_frame(cx)) {
                frame
            } else {
                self.data_read_closed = true;
                return Ok(()).into();
            };

            let typ = self.handle_frame(frame, Some(cx));

            if typ == target_typ {
                return Poll::Ready(Ok(()));
            }
        }
    }

    #[instrument(level = "trace", skip(self, cx))]
    fn poll_send_wndinc(self: Pin<&mut Self>, cx: &mut Context<'_>, by: WndInc) -> Poll<()> {
        let mut this = self.project();
        // Treat a closed send channel as "success"
        if ready!(this.fout.as_mut().poll_ready(cx)).is_err() {
            return Poll::Ready(());
        }

        // Same as above
        let _ = this.fout.as_mut().start_send(Body::WndInc(by).into());

        Poll::Ready(())
    }

    #[instrument(level = "trace", skip(self, cx))]
    fn handle_frame(&mut self, frame: Frame, cx: Option<&Context<'_>>) -> HeaderType {
        if frame.is_fin() {
            self.data_read_closed = true;
        }
        match frame.body {
            Body::Data(bs) => {
                self.read_buf.extend_from_slice(&bs);
                self.maybe_wake_read(cx);
            }
            Body::WndInc(by) => {
                self.window.inc(*by as usize);
                self.maybe_wake_write(cx);
            }
            _ => unreachable!("stream should never receive GoAway, Rst or Invalid frames"),
        }
        frame.header.typ
    }

    #[instrument(level = "trace", skip_all)]
    fn maybe_wake_read(&mut self, cx: Option<&Context>) {
        maybe_wake(cx, self.read_waker.take())
    }
    #[instrument(level = "trace", skip_all)]
    fn maybe_wake_write(&mut self, cx: Option<&Context>) {
        maybe_wake(cx, self.write_waker.take())
    }
}

impl<'a> StreamProj<'a> {
    fn closed_err(&mut self, code: Error) -> io::Error {
        *self.write_closed = Some(code);
        io::Error::new(io::ErrorKind::ConnectionReset, code)
    }
}

fn maybe_wake(me: Option<&Context>, other: Option<Waker>) {
    match (me.map(Context::waker), other) {
        (Some(me), Some(other)) if !other.will_wake(me) => other.wake(),
        (None, Some(other)) => other.wake(),
        _ => {}
    }
}

impl AsyncRead for Stream {
    #[instrument(level = "trace", skip_all)]
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        loop {
            // If we have data, return it
            if !self.read_buf.is_empty() {
                let max = cmp::min(self.read_buf.len(), buf.remaining());
                let clamped = WndInc::clamp(max as u32);
                let n = *clamped as usize;

                if n > 0 {
                    // Wait till there's window capacity to receive an increment.
                    // If this fails, continue anyway.
                    ready!(self.as_mut().poll_send_wndinc(cx, clamped));

                    buf.put_slice(self.read_buf.split_to(n).as_ref());
                }

                return Poll::Ready(Ok(()));
            }

            // EOF's should return Ok without modifying the output buffer.
            if self.data_read_closed {
                return Poll::Ready(Ok(()));
            }

            // Data frames may be ingested by the writer as well, so make sure
            // we don't get forgotten.
            self.read_waker = Some(cx.waker().clone());

            // Otherwise, try to get more.
            ready!(self.as_mut().poll_recv_data(cx))?;
        }
    }
}

impl AsyncWrite for Stream {
    #[instrument(level = "trace", skip(self, cx))]
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, io::Error>> {
        if let Some(code) = self.write_closed {
            return Poll::Ready(Err(io::Error::new(io::ErrorKind::BrokenPipe, code)));
        }

        if self.window.capacity() == 0 {
            self.write_waker = Some(cx.waker().clone());

            ready!(self.as_mut().poll_recv_wndinc(cx))?;
        }

        let mut this = self.project();

        ready!(this.fout.as_mut().poll_ready(cx)).map_err(|e| this.closed_err(e))?;

        let wincap = this.window.capacity();

        let max_len = Length::clamp(buf.len() as u32);

        let send_len = cmp::min(wincap, *max_len as usize);

        let bs = BytesMut::from(&buf[..send_len]);

        let mut frame: Frame = Body::Data(bs.freeze()).into();
        if *this.needs_syn {
            *this.needs_syn = false;
            frame = frame.syn();
        }

        this.fout
            .as_mut()
            .start_send(frame)
            .map_err(|e| this.closed_err(e))?;

        let _dec = this.window.dec(send_len);
        debug_assert!(_dec == send_len);

        Poll::Ready(Ok(send_len))
    }

    #[instrument(level = "trace", skip_all)]
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        let mut this = self.project();
        this.fout
            .as_mut()
            .poll_flush(cx)
            .map_err(|e| this.closed_err(e))
    }

    #[instrument(level = "trace", skip_all)]
    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), io::Error>> {
        // Rather than close the output channel, send a fin frame.
        // This lets us use the actual channel closure as the "stream is gone
        // for good" signal.
        let mut this = self.as_mut().project();
        if this.write_closed.is_none() {
            ready!(this.fout.as_mut().poll_ready(cx))
                .and_then(|_| {
                    this.fout
                        .as_mut()
                        .start_send(Frame::from(Body::Data([][..].into())).fin())
                })
                .map_err(|e| this.closed_err(e))?;
            *this.write_closed = Some(Error::StreamClosed);
        }

        this.fout
            .as_mut()
            .poll_flush(cx)
            .map_err(|e| this.closed_err(e))
    }
}

#[cfg(test)]
pub mod test {
    use std::time::Duration;

    use futures::channel::mpsc;
    use tokio::{
        io::{
            AsyncReadExt,
            AsyncWriteExt,
        },
        time,
    };
    use tracing_test::traced_test;

    use super::*;

    #[traced_test]
    #[tokio::test]
    async fn test_stream() {
        let (mut tx, stream_rx) = mpsc::channel(512);
        let (stream_tx, mut rx) = mpsc::channel(512);
        let stream_tx = StreamSender::wrap(stream_tx);

        let mut stream = Stream::new(stream_tx, stream_rx, 5, true);

        const MSG: &str = "Hello, world!";
        const MSG2: &str = "Hello to you too!";

        // First try a short write, the window won't permit more
        let n = time::timeout(Duration::from_secs(1), stream.write(MSG2.as_bytes()))
            .await
            .unwrap()
            .unwrap();

        assert_eq!(n, 5);
        let resp = rx.try_next().unwrap().unwrap();
        assert_eq!(resp, Frame::from(Body::Data(MSG2[0..5].into())).syn());

        // Next, send the stream an inc and a data frame
        tx.try_send(Body::WndInc(WndInc::clamp(5)).into()).unwrap();
        tx.try_send(Body::Data(MSG.as_bytes().into()).into())
            .unwrap();
        drop(tx);

        // Read the data. The wndinc should get processed as well.
        let mut buf = String::new();
        time::timeout(Duration::from_secs(1), stream.read_to_string(&mut buf))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(buf, "Hello, world!");
        // Reading the data should generate a wndinc
        let resp = rx.try_next().unwrap().unwrap();
        assert_eq!(resp, Body::WndInc(WndInc::clamp(MSG.len() as u32)).into());

        // Finally, try writing again. If the previous read handled the wndinc, we'll have capacity for 5 more bytes
        let n = time::timeout(Duration::from_secs(1), stream.write(MSG2[5..].as_bytes()))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(n, 5);
        let resp = rx.try_next().unwrap().unwrap();
        assert_eq!(resp, Body::Data(MSG2[5..10].into()).into());

        stream.shutdown().await.unwrap();

        assert!(rx.try_next().unwrap().unwrap().is_fin());
    }
}

#[pin_project::pinned_drop]
impl PinnedDrop for Stream {
    #[instrument(level = "trace", skip_all)]
    fn drop(self: Pin<&mut Self>) {}
}
