//! Heartbeating [TypedSession] wrapper.
//!
//! This can be used to wrap a [TypedSession] to provide heartbeating
//! functionality. The wrapper will start a background task to send heartbeats
//! to the remote via a dedicated heartbeat stream. It will also accept incoming
//! heartbeat streams and start a task to reply to them.

use std::{
    error::Error as StdError,
    io,
    sync::{
        Arc,
        atomic::{
            AtomicU64,
            Ordering,
        },
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
    runtime::Handle,
    select,
    sync::{
        Notify,
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

// The shortest interval between scheduled heartbeats. Shorter intervals,
// including zero, are raised to this, both for sending heartbeats and for
// timing them out.
const MIN_INTERVAL: Duration = Duration::from_millis(1);

/// Wrapper for a muxado [TypedSession] that adds heartbeating over a dedicated
/// typed stream.
pub struct Heartbeat<S> {
    runtime: Handle,
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
    // Wakes the requester so it picks up a new interval right away.
    interval_changed: Arc<Notify>,
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
    /// An error returned from this is logged and otherwise ignored: it
    /// doesn't stop heartbeats or close the session.
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
            runtime: Handle::current(),
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
            interval_changed: Arc::new(Notify::new()),
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
    ///
    /// If the new interval would make a heartbeat due sooner, the next one is
    /// brought forward; otherwise it's sent as already scheduled and the new
    /// interval applies from then on. Heartbeats are sent at most once a
    /// millisecond.
    pub fn set_interval(&self, interval: Duration) {
        self.durations
            .0
            .store(interval.as_nanos() as u64, Ordering::Relaxed);
        self.interval_changed.notify_one();
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
                        let latency = match tokio::time::timeout_at(deadline, mark.recv()).await {
                            Err(_) => None,
                            Ok(Some(lat)) => Some(lat),
                            Ok(None) => return,
                        };
                        // A failing handler says nothing about whether the
                        // connection is alive, so don't stop checking it.
                        if let Some(cb) = cb.as_ref()
                            && let Err(error) = cb.handle_heartbeat(latency).await
                        {
                            tracing::warn!(%error, "heartbeat handler error");
                        }

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
        let mut interval = self.get_durations().0;
        // When the last scheduled heartbeat was sent, and when the next one
        // is due. The first is due right away.
        let mut last_tick = tokio::time::Instant::now();
        let mut next_tick = last_tick;
        let durations = self.durations.clone();
        let interval_changed = self.interval_changed.clone();

        tokio::spawn(
            select(
                async move {
                    loop {
                        let mut resp_chan: Option<oneshot::Sender<Duration>> = None;

                        // Pick up interval changes. The check task's current
                        // deadline is based on an earlier interval, so the
                        // heartbeat that's already due is never pushed back:
                        // a new interval can only bring it forward, and no
                        // earlier than now. Later ones follow the new
                        // interval.
                        let new_interval = get_durations(&durations).0;
                        if new_interval != interval {
                            interval = new_interval;
                            let rescheduled =
                                (last_tick + interval).max(tokio::time::Instant::now());
                            next_tick = next_tick.min(rescheduled);
                        }

                        let mut scheduled = false;
                        select! {
                            // Loop around to pick up the new interval.
                            _ = interval_changed.notified() => continue,
                            // on_demand only closes when the HeartbeatCtl is
                            // dropped, which tears the heartbeat down.
                            c = on_demand.recv() => match c {
                                Some(c) => resp_chan = Some(c),
                                None => return,
                            },
                            _ = tokio::time::sleep_until(next_tick) => scheduled = true,
                        }
                        if scheduled {
                            last_tick = tokio::time::Instant::now();
                            next_tick = last_tick + interval;
                        }

                        tracing::debug!("sending heartbeat");

                        let start = std::time::Instant::now();
                        let id: i32 = rand::random();

                        if stream.write_all(&id.to_be_bytes()[..]).await.is_err() {
                            tracing::debug!("error writing heartbeat");
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
                            tracing::debug!("heartbeat response id mismatch");
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

fn start_responder(rt: &Handle, mut stream: TypedStream, drop_waiter: awaitdrop::WaitFuture) {
    rt.spawn(select(
        async move {
            loop {
                let mut buf = [0u8; 4];
                if let Err(e) = stream.read_exact(&mut buf[..]).await {
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
                start_responder(&self.runtime, stream, self.drop_waiter.wait());
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
        let runtime = self.runtime;
        let (open, accept) = self.inner.split_typed();
        (
            Heartbeat {
                runtime: runtime.clone(),
                drop_waiter: drop_waiter.clone(),
                typ,
                inner: open,
            },
            Heartbeat {
                runtime,
                drop_waiter,
                typ,
                inner: accept,
            },
        )
    }
}

fn get_durations(durations: &Arc<(AtomicU64, AtomicU64)>) -> (Duration, Duration) {
    (
        Duration::from_nanos(durations.0.load(Ordering::Relaxed)).max(MIN_INTERVAL),
        Duration::from_nanos(durations.1.load(Ordering::Relaxed)),
    )
}

#[cfg(test)]
mod test {
    use std::sync::Mutex;

    use tokio::io::duplex;

    use super::*;
    use crate::{
        SessionBuilder,
        typed::Typed,
    };

    fn config(handler: Option<Arc<dyn HeartbeatHandler>>) -> HeartbeatConfig {
        HeartbeatConfig {
            interval: Duration::from_secs(1),
            tolerance: Duration::from_secs(1),
            handler,
        }
    }

    type Calls = Arc<Mutex<Vec<Option<Duration>>>>;

    fn recorder() -> (Arc<dyn HeartbeatHandler>, Calls) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let c = calls.clone();
        let handler = move |lat: Option<Duration>| {
            c.lock().unwrap().push(lat);
            async { Ok::<(), Box<dyn StdError>>(()) }
        };
        (Arc::new(handler), calls)
    }

    /// Changing the interval at runtime, up or down, doesn't make a healthy
    /// session time out.
    #[tokio::test(start_paused = true)]
    async fn set_interval_keeps_session() {
        for (from, to) in [(10, 1), (1, 10)] {
            let (left, right) = duplex(64 * 1024);
            let server = Typed::new(SessionBuilder::new(left).server().start());
            let (mut server, _server_ctl) = Heartbeat::start(server, config(None)).await.unwrap();
            tokio::spawn(async move { while server.accept_typed().await.is_ok() {} });

            let (handler, calls) = recorder();
            let client = Typed::new(SessionBuilder::new(right).client().start());
            let cfg = HeartbeatConfig {
                interval: Duration::from_secs(from),
                tolerance: Duration::from_secs(2),
                handler: Some(handler),
            };
            let (mut hb, ctl) = Heartbeat::start(client, cfg).await.unwrap();

            tokio::time::sleep(Duration::from_millis(500)).await;
            ctl.set_interval(Duration::from_secs(to));

            let res = tokio::time::timeout(Duration::from_secs(60), hb.accept_typed()).await;
            assert!(res.is_err(), "{from}s -> {to}s: session should stay open");
            let calls = calls.lock().unwrap();
            assert!(
                calls.iter().all(Option::is_some),
                "{from}s -> {to}s: {calls:?}"
            );
            let expected = 60 / to.max(1) as usize;
            assert!(
                calls.len() + 2 >= expected,
                "{from}s -> {to}s: expected about {expected} beats, got {}",
                calls.len()
            );
        }
    }

    /// A client with the given interval and tolerance, and a server that
    /// answers its heartbeats. The server sends none of its own, so the
    /// client doesn't have to keep accepting for the session to stay up.
    async fn healthy_pair(
        interval: Duration,
        tolerance: Duration,
    ) -> (Heartbeat<Typed<crate::MuxadoSession>>, HeartbeatCtl, Calls) {
        let (left, right) = duplex(64 * 1024);
        let mut server = Typed::new(SessionBuilder::new(left).server().start());
        tokio::spawn(async move {
            while let Ok(mut stream) = server.accept_typed().await {
                tokio::spawn(async move {
                    let mut buf = [0u8; 4];
                    while stream.read_exact(&mut buf).await.is_ok() {
                        if stream.write_all(&buf).await.is_err() {
                            return;
                        }
                    }
                });
            }
        });

        let (handler, calls) = recorder();
        let client = Typed::new(SessionBuilder::new(right).client().start());
        let cfg = HeartbeatConfig {
            interval,
            tolerance,
            handler: Some(handler),
        };
        let (hb, ctl) = Heartbeat::start(client, cfg).await.unwrap();
        (hb, ctl, calls)
    }

    /// Lowering the interval long after the last heartbeat sends one
    /// heartbeat right away, not one for every new interval that has passed.
    #[tokio::test(start_paused = true)]
    async fn lowering_interval_late_doesnt_burst() {
        let (mut hb, ctl, calls) =
            healthy_pair(Duration::from_secs(60), Duration::from_secs(2)).await;

        tokio::time::sleep(Duration::from_secs(50)).await;
        assert_eq!(calls.lock().unwrap().len(), 1);
        ctl.set_interval(Duration::from_secs(1));
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(
            calls.lock().unwrap().len(),
            2,
            "one heartbeat should follow the change at once"
        );

        let res = tokio::time::timeout(Duration::from_millis(5500), hb.accept_typed()).await;
        assert!(res.is_err(), "session should stay open");
        let calls = calls.lock().unwrap();
        assert!(calls.iter().all(Option::is_some), "{calls:?}");
        assert!((6..=8).contains(&calls.len()), "{}", calls.len());
    }

    /// Several interval changes before the next heartbeat don't push it past
    /// the deadline the check task is already waiting on.
    #[tokio::test(start_paused = true)]
    async fn consecutive_set_interval_keeps_session() {
        let (mut hb, ctl, calls) =
            healthy_pair(Duration::from_secs(1), Duration::from_secs(2)).await;

        tokio::time::sleep(Duration::from_millis(500)).await;
        ctl.set_interval(Duration::from_secs(10));
        tokio::time::sleep(Duration::from_millis(100)).await;
        ctl.set_interval(Duration::from_secs(20));

        let res = tokio::time::timeout(Duration::from_secs(120), hb.accept_typed()).await;
        assert!(res.is_err(), "session should stay open");
        let calls = calls.lock().unwrap();
        assert!(calls.iter().all(Option::is_some), "{calls:?}");
    }

    /// A zero interval doesn't stop heartbeating.
    #[tokio::test(start_paused = true)]
    async fn zero_interval_keeps_heartbeating() {
        let (mut hb, ctl, calls) =
            healthy_pair(Duration::from_secs(1), Duration::from_secs(2)).await;

        tokio::time::sleep(Duration::from_millis(500)).await;
        ctl.set_interval(Duration::ZERO);
        let res = tokio::time::timeout(Duration::from_secs(1), hb.accept_typed()).await;
        assert!(res.is_err(), "session should stay open");
        ctl.beat().await.expect("the requester should still run");

        ctl.set_interval(Duration::from_secs(1));
        let res = tokio::time::timeout(Duration::from_secs(30), hb.accept_typed()).await;
        assert!(res.is_err(), "session should stay open");
        let calls = calls.lock().unwrap();
        assert!(calls.iter().all(Option::is_some), "{calls:?}");
    }

    /// A handler that returns an error neither closes the session nor
    /// stops heartbeats.
    #[tokio::test(start_paused = true)]
    async fn handler_error_keeps_session_open() {
        let (left, right) = duplex(64 * 1024);
        let server = Typed::new(SessionBuilder::new(left).server().start());
        let (mut server, _server_ctl) = Heartbeat::start(server, config(None)).await.unwrap();
        tokio::spawn(async move { while server.accept_typed().await.is_ok() {} });

        let calls: Calls = Default::default();
        let c = calls.clone();
        let handler = move |lat: Option<Duration>| {
            c.lock().unwrap().push(lat);
            async { Err::<(), Box<dyn StdError>>("boom".into()) }
        };
        let client = Typed::new(SessionBuilder::new(right).client().start());
        let (mut hb, _ctl) = Heartbeat::start(client, config(Some(Arc::new(handler))))
            .await
            .unwrap();

        let res = tokio::time::timeout(Duration::from_secs(10), hb.accept_typed()).await;
        assert!(res.is_err(), "session should stay open");
        let calls = calls.lock().unwrap();
        assert!(calls.len() >= 9, "heartbeats should continue: {calls:?}");
        assert!(calls.iter().all(Option::is_some), "{calls:?}");
    }

    /// The responder echoes each heartbeat whole even when it arrives in
    /// pieces, and stops when the remote ends the stream rather than echoing
    /// zeroes.
    #[tokio::test(start_paused = true)]
    async fn responder_echoes_whole_heartbeats_and_stops_at_eof() {
        let (left, right) = duplex(64 * 1024);
        let server = Typed::new(SessionBuilder::new(left).server().start());
        let (mut server_open, mut server_accept) = server.split_typed();
        // Hold the client's own heartbeat stream without answering it; the
        // long interval keeps that from closing the session.
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok(stream) = server_accept.accept_typed().await {
                held.push(stream);
            }
        });

        let client = Typed::new(SessionBuilder::new(right).client().start());
        let cfg = HeartbeatConfig {
            interval: Duration::from_secs(3600),
            tolerance: Duration::from_secs(3600),
            handler: None,
        };
        let (mut hb, _ctl) = Heartbeat::start(client, cfg).await.unwrap();
        tokio::spawn(async move { while hb.accept_typed().await.is_ok() {} });

        let mut stream = server_open.open_typed(HEARTBEAT_TYPE).await.unwrap();
        stream.write_all(&[1, 2]).await.unwrap();
        tokio::time::sleep(Duration::from_millis(10)).await;
        stream.write_all(&[3, 4]).await.unwrap();
        let mut buf = [0u8; 4];
        tokio::time::timeout(Duration::from_secs(1), stream.read_exact(&mut buf))
            .await
            .expect("the responder should echo the heartbeat")
            .unwrap();
        assert_eq!(buf, [1, 2, 3, 4]);

        stream.shutdown().await.unwrap();
        let mut rest = Vec::new();
        let _ = tokio::time::timeout(Duration::from_secs(1), stream.read_to_end(&mut rest)).await;
        assert!(rest.is_empty(), "the responder echoed after EOF: {rest:?}");
    }
}
