//! Heartbeating [TypedSession] wrapper.
//!
//! This can be used to wrap a [TypedSession] to provide heartbeating
//! functionality. The wrapper will start a background task to send heartbeats
//! to the remote via a dedicated heartbeat stream. It will also accept incoming
//! heartbeat streams and start a task to reply to them.

use std::{
    error::Error as StdError,
    future::Future,
    io,
    sync::{
        atomic::{
            AtomicU64,
            Ordering,
        },
        Arc,
    },
    time::Duration,
};

use async_trait::async_trait;
use futures::{
    future::select,
    prelude::*,
};
use tokio::{
    io::{
        AsyncReadExt,
        AsyncWriteExt,
    },
    select,
    sync::{
        mpsc,
        oneshot,
    },
};

use crate::{
    errors::Error,
    typed::{
        StreamType,
        TypedAccept,
        TypedOpenClose,
        TypedSession,
        TypedStream,
    },
};

const HEARTBEAT_TYPE: StreamType = StreamType::clamp(0xFFFFFFFF);

/// Wrapper for a muxado [TypedSession] that adds heartbeating over a dedicated
/// typed stream.
pub struct Heartbeat<S> {
    drop_waiter: awaitdrop::Waiter,
    typ: StreamType,
    inner: S,
}

/// Controller for the heartbeat task.
///
/// Allows owners to change the heartbeat timing at runtime and to explicitly
/// request heartbeats. When dropped, cancels the heartbeat tasks.
pub struct HeartbeatCtl {
    // Implicitly used to cancel the heartbeat tasks.
    #[allow(dead_code)]
    dropref: awaitdrop::Ref,
    durations: Arc<(AtomicU64, AtomicU64)>,
    on_demand: mpsc::Sender<oneshot::Sender<Duration>>,
}

/// A handler called on every heartbeat with the latency for that beat.
#[async_trait]
pub trait HeartbeatHandler: Send + Sync + 'static {
    /// Handle the heartbeat
    ///
    /// A `None` latency implies that the timeout was reached before the
    /// heartbeat reply was received.
    ///
    /// If this returns an error, the heartbeat task will exit.
    async fn handle_heartbeat(&self, latency: Option<Duration>) -> Result<(), Box<dyn StdError>>;
}

#[async_trait]
impl<T, F> HeartbeatHandler for T
where
    T: Fn(Option<Duration>) -> F + Send + Sync + 'static,
    F: Future<Output = Result<(), Box<dyn StdError>>> + Send,
{
    async fn handle_heartbeat(&self, latency: Option<Duration>) -> Result<(), Box<dyn StdError>> {
        self(latency).await
    }
}

/// The heartbeat task configuration.
pub struct HeartbeatConfig {
    /// The interval on which heartbeats will be sent.
    pub interval: Duration,
    /// The amount of time past a missed heartbeat that the other side will be
    /// considered dead.
    pub tolerance: Duration,
    /// An optional callback to run when a heartbeat is received.
    pub handler: Option<Arc<dyn HeartbeatHandler>>,
}

impl Default for HeartbeatConfig {
    fn default() -> Self {
        HeartbeatConfig {
            interval: Duration::from_secs(10),
            tolerance: Duration::from_secs(15),
            handler: None,
        }
    }
}

impl<S> Heartbeat<S>
where
    S: TypedSession + 'static,
{
    /// Wrap a typed session and start the heartbeat task.
    /// Returns an error if the stream can't be opened.
    pub async fn start(sess: S, cfg: HeartbeatConfig) -> Result<(Self, HeartbeatCtl), io::Error> {
        let (dropref, drop_waiter) = awaitdrop::awaitdrop();

        let mut hb = Heartbeat {
            drop_waiter: drop_waiter.clone(),
            typ: HEARTBEAT_TYPE,
            inner: sess,
        };

        let (dtx, drx) = mpsc::channel(1);
        let (mtx, mrx) = mpsc::channel(1);
        let mut ctl = HeartbeatCtl {
            dropref,
            durations: Arc::new((
                (cfg.interval.as_nanos() as u64).into(),
                (cfg.tolerance.as_nanos() as u64).into(),
            )),
            on_demand: dtx,
        };

        let stream = hb
            .inner
            .open_typed(hb.typ)
            .await
            .map_err(|_| io::ErrorKind::ConnectionReset)?;

        ctl.start_requester(stream, drx, mtx, drop_waiter.wait())
            .await?;
        ctl.start_check(mrx, cfg.handler, drop_waiter.wait())?;

        Ok((hb, ctl))
    }
}

impl HeartbeatCtl {
    /// Explicitly request a heartbeat and return the latency.
    pub async fn beat(&self) -> Result<Duration, io::Error> {
        let (tx, rx) = oneshot::channel();
        self.on_demand
            .send(tx)
            .await
            .map_err(|_| io::ErrorKind::NotConnected)?;
        rx.await.map_err(|_| io::ErrorKind::ConnectionReset.into())
    }

    /// Change the heartbeat interval.
    pub fn set_interval(&self, interval: Duration) {
        self.durations
            .0
            .store(interval.as_nanos() as u64, Ordering::Relaxed);
    }

    /// Change the heartbeat tolerance.
    pub fn set_tolerance(&self, tolerance: Duration) {
        self.durations
            .1
            .store(tolerance.as_nanos() as u64, Ordering::Relaxed);
    }

    fn start_check(
        &mut self,
        mut mark: mpsc::Receiver<Duration>,
        cb: Option<Arc<dyn HeartbeatHandler>>,
        dropped: awaitdrop::WaitFuture,
    ) -> Result<(), io::Error> {
        let (mut interval, mut tolerance) = self.get_durations();
        let durations = self.durations.clone();

        tokio::spawn(
            select(
                async move {
                    let mut deadline = tokio::time::Instant::now() + interval + tolerance;
                    loop {
                        match tokio::time::timeout_at(deadline, mark.recv()).await {
                            Err(_e) => {
                                if let Some(cb) = cb.as_ref() {
                                    cb.handle_heartbeat(None).await?;
                                }
                            }
                            Ok(Some(lat)) => {
                                if let Some(cb) = cb.as_ref() {
                                    cb.handle_heartbeat(lat.into()).await?;
                                }
                            }
                            Ok(None) => {
                                return Result::<(), Box<dyn StdError>>::Ok(());
                            }
                        };

                        // Slight divergence from Go implementation: this didn't
                        // previously happen in the "timeout" case, which did noting but
                        // the callback. Presumably, this usually killed the connection,
                        // causing the goroutine to exit *anyway*. If we didn't reset
                        // the deadline here, it would timeout immediately rather than
                        // blocking indefinitely as in Go.
                        (interval, tolerance) = get_durations(&durations);
                        deadline = tokio::time::Instant::now() + interval + tolerance;
                    }
                }
                .boxed(),
                dropped,
            )
            .then(|_| async move {
                tracing::debug!("check exited");
            }),
        );

        Ok(())
    }

    async fn start_requester(
        &mut self,
        mut stream: TypedStream,
        mut on_demand: mpsc::Receiver<oneshot::Sender<Duration>>,
        mark: mpsc::Sender<Duration>,
        drop_waiter: awaitdrop::WaitFuture,
    ) -> Result<(), io::Error> {
        let (interval, _) = self.get_durations();
        let mut ticker = tokio::time::interval(interval);

        tokio::spawn(
            select(
                async move {
                    loop {
                        let mut resp_chan: Option<oneshot::Sender<Duration>> = None;

                        select! {
                            // If on_demand is closed, this will return None
                            // immediately. In that case, wait on the next tick instead.
                            c = on_demand.recv() => if c.is_none() {
                                ticker.tick().await;
                            } else {
                                resp_chan = c;
                            },
                            _ = ticker.tick() => {},
                        }

                        tracing::debug!("sending heartbeat");

                        let start = std::time::Instant::now();
                        let id: i32 = rand::random();

                        if stream.write_all(&id.to_be_bytes()[..]).await.is_err() {
                            return;
                        }

                        let mut resp_bytes = [0u8; 4];

                        tracing::debug!("waiting for response");

                        if stream.read_exact(&mut resp_bytes[..]).await.is_err() {
                            tracing::debug!("error reading response");
                            return;
                        }

                        tracing::debug!("got response");

                        let resp_id = i32::from_be_bytes(resp_bytes);

                        if id != resp_id {
                            return;
                        }

                        let latency = std::time::Instant::now() - start;

                        if let Some(resp_chan) = resp_chan {
                            let _ = resp_chan.send(latency);
                        } else {
                            let _ = mark.send(latency).await;
                        }
                    }
                }
                .boxed(),
                drop_waiter,
            )
            .then(|_| async move {
                tracing::debug!("requester exited");
            }),
        );

        Ok(())
    }

    fn get_durations(&self) -> (Duration, Duration) {
        get_durations(&self.durations)
    }
}

fn start_responder(mut stream: TypedStream, drop_waiter: awaitdrop::WaitFuture) {
    tokio::spawn(select(
        async move {
            loop {
                let mut buf = [0u8; 4];
                if let Err(e) = stream.read(&mut buf[..]).await {
                    tracing::debug!(?e, "heartbeat responder exiting");
                    return;
                }
                if let Err(e) = stream.write_all(&buf[..]).await {
                    tracing::debug!(?e, "heartbeat responder exiting");
                    return;
                }
            }
        }
        .boxed(),
        drop_waiter,
    ));
}

#[async_trait]
impl<S> TypedAccept for Heartbeat<S>
where
    S: TypedAccept + Send,
{
    async fn accept_typed(&mut self) -> Result<TypedStream, Error> {
        loop {
            let stream = self.inner.accept_typed().await?;
            let typ = stream.typ();

            if typ == self.typ {
                start_responder(stream, self.drop_waiter.wait());
                continue;
            }

            return Ok(stream);
        }
    }
}

#[async_trait]
impl<S> TypedOpenClose for Heartbeat<S>
where
    S: TypedOpenClose + Send,
{
    async fn open_typed(&mut self, typ: StreamType) -> Result<TypedStream, Error> {
        // Don't open a heartbeat stream manually
        if typ == self.typ {
            return Err(Error::StreamRefused);
        }

        self.inner.open_typed(typ).await
    }

    async fn close(&mut self, error: Error, msg: String) -> Result<(), Error> {
        self.inner.close(error, msg).await
    }
}

impl<S> TypedSession for Heartbeat<S>
where
    S: TypedSession + Send,
    S::TypedAccept: Send,
    S::TypedOpen: Send,
{
    type TypedAccept = Heartbeat<S::TypedAccept>;
    type TypedOpen = Heartbeat<S::TypedOpen>;

    fn split_typed(self) -> (Self::TypedOpen, Self::TypedAccept) {
        let drop_waiter = self.drop_waiter;
        let typ = self.typ;
        let (open, accept) = self.inner.split_typed();
        (
            Heartbeat {
                drop_waiter: drop_waiter.clone(),
                typ,
                inner: open,
            },
            Heartbeat {
                drop_waiter,
                typ,
                inner: accept,
            },
        )
    }
}

fn get_durations(durations: &Arc<(AtomicU64, AtomicU64)>) -> (Duration, Duration) {
    (
        Duration::from_nanos(durations.0.load(Ordering::Relaxed)),
        Duration::from_nanos(durations.1.load(Ordering::Relaxed)),
    )
}
