trait FramedConn: StreamT<Item = Result<Frame, io::Error>> + Sink<Frame, Error = io::Error> {}
impl<T> FramedConn for T where
    T: StreamT<Item = Result<Frame, io::Error>> + Sink<Frame, Error = io::Error>
{
}

trait NeedsWriteTx: Sink<Frame, Error = ()> + Clone {}
impl<T> NeedsWriteTx for T where T: Sink<Frame, Error = ()> + Clone {}

#[pin_project(project = SessionProj)]
pub struct Session<IO, R, S> {
    // The real IO stream
    #[pin]
    remote: IO,

    // Frames coming from streams in other tasks
    #[pin]
    needs_write: R,

    // Send side of the above channel.
    // Cloned and given to each stream
    needs_write_tx: S,

    // Map of all of the live streams
    // Also includes un-accepted streams.
    streams: HashMap<StreamID, StreamHandle>,

    // Queue of streams that need accepting before real use.
    accept_queue: VecDeque<Stream>,
    // If another task is trying to accept a stream, but there weren't any at
    // the time, its waker will be here.
    accept_waker: Option<Waker>,

    // A frame that's been read and needs to be processed
    current_frame: Option<Frame>,

    // The last stream that had either a wndinc or data frame sent to it.
    last_stream_id: StreamID,

    // We sent a goaway to the remote
    remote_gone: bool,
    // Remote sent a goaway to us
    self_gone: bool,

    // No more frames from the remote
    eof: bool,

    // Window size to use when creating streams
    window_size: usize,
}
impl<IO, R, S> Session<IO, R, S>
where
    IO: StreamT<Item = Result<Frame, io::Error>> + Sink<Frame, Error = io::Error> + 'static,
    R: StreamT<Item = Body> + 'static,
    S: Sink<Frame, Error = ()> + Clone + 'static,
{
    fn poll_recv_frame(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        debug_assert!(self.current_frame.is_none());

        let this = self.project();

        if *this.self_gone {
            Err(io::Error::from(io::ErrorKind::ConnectionReset))?;
        }
        if *this.remote_gone {
            Err(io::Error::from(io::ErrorKind::ConnectionAborted))?;
        }

        let frame = ready!(this.remote.poll_next(cx)).transpose()?;
        if frame.is_none() {
            *this.eof = true;
        }
        *this.current_frame = frame;

        Poll::Ready(Ok(()))
    }

    fn poll_handle_frame(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Result<(), io::Error>> {
        debug_assert!(self.current_frame.is_some());

        let mut this = self.project();

        // Check if a new stream is being openend.
        let new_stream = this.current_frame.as_mut().and_then(|f| {
            let syn = f.is_syn();
            f.header.flags.set(Flags::SYN, false); // 0 the flag so we only start a new stream once.
            Some(f.header.stream_id).filter(|_| syn && f.header.typ == HeaderType::Data)
        });

        if let Some(id) = new_stream {
            if let Some(stream) = this.create_stream(id) {
                // Since the remote opened the stream, add it to the queue.
                this.accept_queue.push_back(stream);
                // If someone was waiting for a new stream, wake 'em up.
                this.accept_waker.take().map(|w| {
                    w.wake();
                });
            }
        }

        let frame = this.current_frame.as_ref().unwrap();
        let stream_id = frame.header.stream_id;

        let mut needs_cleanup = false;
        let mut framing_error = false;
        let mut needs_send = false;
        let mut sent = false;

        match frame.body {
            Body::Invalid { error, .. } => {
                ready!(this.remote.as_mut().poll_ready(cx))?;
                // Close all of the current streams.
                for (_, stream) in this.streams.drain() {
                    stream.sink_closer.close_with(ErrorType::RemoteGoneAway);
                }
                *this.remote_gone = true;
                let goaway = Frame::goaway(
                    *this.last_stream_id,
                    error.into(),
                    format!("Invalid header: {}", error).into(),
                );
                this.remote.as_mut().start_send(goaway)?;
            }
            Body::Rst(code) => {
                if let Some(stream) = this.streams.remove(&stream_id) {
                    stream.sink_closer.close_with(code);
                }
            }
            Body::Data(_) | Body::WndInc(_) => {
                needs_send = true;
                let body = frame.body.clone();

                let consumed = if let Body::Data(ref bs) = frame.body {
                    bs.len()
                } else {
                    0
                };

                if let Some(stream) = this.streams.get_mut(&stream_id) {
                    if let Some(frames) = stream.frames.as_mut() {
                        if ready!(frames.as_mut().poll_ready(cx)).is_ok() {
                            if consumed > stream.window {
                                framing_error = true;
                            } else if sent {
                                stream.window -= consumed;
                            }

                            if !framing_error {
                                sent = frames.as_mut().start_send(body).is_ok();
                            } else {
                                // Lie a little and say that we actually sent
                                // something. This lets us skip some cleanup
                                // till later.
                                sent = true;
                            }
                        }
                    }

                    // If sending wasn't possible, it means the stream's rx
                    // channel is closed. Drop our side of it so that we don't
                    // try to send to it again.
                    if !sent {
                        stream.frames.take();
                        // If the sink closer contains an error or is gone, it
                        // means someone has already closed it.
                        // FIXME: This is racy. Add a closure channel as well to
                        //        resolve it.
                        needs_cleanup = stream.sink_closer.check_closed().is_err();
                    }
                }

                if sent && !framing_error {
                    *this.last_stream_id = stream_id;
                }
            }
            Body::GoAway { .. } => {
                for (_, stream) in this.streams.drain() {
                    stream.sink_closer.close_with(ErrorType::RemoteGoneAway);
                }
                *this.self_gone = true;
            }
        }

        // If there was a framing error (remote overflowed the stream's window),
        // send a reset.
        if framing_error {
            ready!(this.remote.as_mut().poll_ready(cx))?;
            this.remote
                .as_mut()
                .start_send(Frame::rst(stream_id, ErrorType::FlowControlError))?;

            // We deferred this cleanup earlier by lying about the sent status.
            needs_cleanup = this
                .streams
                .get_mut(&stream_id)
                .map(|s| {
                    s.frames.take();
                    s.sink_closer.check_closed().is_err()
                })
                .unwrap_or(false);
        } else
        // If we should've tried to send a frame to a stream, but couldn't, it
        // means the stream either doesn't exist or was closed.
        // Send a reset to the remote.
        if needs_send && !sent {
            ready!(this.remote.as_mut().poll_ready(cx))?;
            this.remote
                .as_mut()
                .start_send(Frame::rst(stream_id, ErrorType::StreamClosed))?;
        }

        if needs_cleanup {
            this.streams.remove(&stream_id);
        }

        // At this point, the frame has been handled. Drop it.
        this.current_frame.take();

        Poll::Ready(Ok(()))
    }

    // Read and handle frames until an error or EOF is encountered.
    pub fn poll_read_task(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), io::Error>> {
        loop {
            if self.eof {
                return Poll::Ready(Ok(()));
            }

            if self.current_frame.is_some() {
                ready!(self.as_mut().poll_handle_frame(cx))?;
            } else {
                ready!(self.as_mut().poll_recv_frame(cx))?;
            }
        }
    }
}
