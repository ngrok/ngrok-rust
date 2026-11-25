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
use tokio_util::sync::CancellationToken;

use crate::{
    errors::Error,
    session::ShutdownHandle,
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
    // Cancelled when a heartbeat is missed. Once cancelled, the session is
    // considered dead: accepts and opens fail with [Error::SessionClosed].
    dead: CancellationToken,
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
    /// heartbeat reply was received. At that point the session has already
    /// been closed, and the handler won't be called again.
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
    /// considered dead. Once it is, the session is closed.
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
        let dead = CancellationToken::new();
        let shutdown = sess.shutdown_handle();

        let mut hb = Heartbeat {
            runtime: Handle::current(),
            drop_waiter: drop_waiter.clone(),
            typ: HEARTBEAT_TYPE,
            dead: dead.clone(),
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

        ctl.start_requester(
            stream,
            drx,
            mtx,
            dead.clone(),
            shutdown.clone(),
            drop_waiter.wait(),
        )
        .await?;
        ctl.start_check(mrx, cfg.handler, dead, shutdown, drop_waiter.wait())?;

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
        dead: CancellationToken,
        shutdown: Option<ShutdownHandle>,
        dropped: awaitdrop::WaitFuture,
    ) -> Result<(), io::Error> {
        let (mut interval, mut tolerance) = self.get_durations();
        let durations = self.durations.clone();

        // Why the check loop ended.
        enum Ended {
            // The heartbeat is being torn down, or the requester has already
            // dealt with a failed heartbeat stream.
            Quietly,
            TimedOut,
        }

        tokio::spawn(async move {
            let cb = cb.as_ref();
            let check = async {
                let mut deadline = tokio::time::Instant::now() + interval + tolerance;
                loop {
                    match tokio::time::timeout_at(deadline, mark.recv()).await {
                        Ok(Some(lat)) => {
                            // A failing handler says nothing about whether
                            // the connection is alive. Closing the session
                            // over it would make a handler that always fails
                            // reconnect as fast as the network allows.
                            if let Some(cb) = cb
                                && let Err(error) = cb.handle_heartbeat(lat.into()).await
                            {
                                tracing::warn!(%error, "heartbeat handler error");
                            }
                        }
                        // The requester exited. Either the heartbeat stream
                        // failed, in which case the requester has already
                        // closed the session, or the heartbeat is being torn
                        // down. Either way this isn't a missed heartbeat, so
                        // exit without reporting one.
                        Ok(None) => return Ended::Quietly,
                        Err(_) => {
                            tracing::warn!(
                                ?interval,
                                ?tolerance,
                                "heartbeat timeout, closing session"
                            );
                            return Ended::TimedOut;
                        }
                    };

                    (interval, tolerance) = get_durations(&durations);
                    deadline = tokio::time::Instant::now() + interval + tolerance;
                }
            };
            // Only the loop stops when the heartbeat is dropped. What follows
            // closes the session, which usually makes its owner drop the
            // heartbeat, for example to reconnect. That mustn't cut short the
            // handler's last call.
            let ended = select! {
                ended = check => ended,
                _ = dropped => Ended::Quietly,
            };

            if matches!(ended, Ended::TimedOut) {
                // The deadline passed without a heartbeat reply. As in the Go
                // implementation, consider the session dead and close it so
                // that accept fails and owners can reconnect. Do this before
                // calling the handler so that a slow handler can't delay it.
                dead.cancel();
                if let Some(shutdown) = shutdown {
                    shutdown.shutdown();
                }
                if let Some(cb) = cb
                    && let Err(error) = cb.handle_heartbeat(None).await
                {
                    tracing::debug!(%error, "heartbeat handler error");
                }
            }
            tracing::debug!("check exited");
        });

        Ok(())
    }

    async fn start_requester(
        &mut self,
        mut stream: TypedStream,
        mut on_demand: mpsc::Receiver<oneshot::Sender<Duration>>,
        mark: mpsc::Sender<Duration>,
        dead: CancellationToken,
        shutdown: Option<ShutdownHandle>,
        drop_waiter: awaitdrop::WaitFuture,
    ) -> Result<(), io::Error> {
        let mut interval = self.get_durations().0;
        // When the last scheduled heartbeat was sent, and when the next one
        // is due. The first is due right away.
        let mut last_tick = tokio::time::Instant::now();
        let mut next_tick = last_tick;
        let durations = self.durations.clone();
        let interval_changed = self.interval_changed.clone();

        // The heartbeat stream failed, so no more heartbeats can be sent.
        // Close the session rather than waiting for the check task to time
        // out on a session that can no longer be monitored.
        let fail_dead = dead.clone();
        let fail = move |reason: &str| {
            if !fail_dead.is_cancelled() {
                tracing::debug!(reason, "heartbeat stream failed, closing session");
            }
            fail_dead.cancel();
            if let Some(shutdown) = &shutdown {
                shutdown.shutdown();
            }
        };

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
                            _ = dead.cancelled() => return,
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
                            fail("error writing heartbeat");
                            return;
                        }

                        let mut resp_bytes = [0u8; 4];

                        tracing::debug!("waiting for response");

                        // The read has no timeout of its own: the check task
                        // closes the session if no reply arrives within
                        // interval + tolerance, which cancels this read.
                        select! {
                            _ = dead.cancelled() => {
                                tracing::debug!("session dead, abandoning heartbeat");
                                return;
                            }
                            res = stream.read_exact(&mut resp_bytes[..]) => if res.is_err() {
                                fail("error reading heartbeat response");
                                return;
                            }
                        }

                        tracing::debug!("got response");

                        let resp_id = i32::from_be_bytes(resp_bytes);

                        if id != resp_id {
                            fail("heartbeat response id mismatch");
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
            let stream = select! {
                biased;
                _ = self.dead.cancelled() => return Err(Error::SessionClosed),
                res = self.inner.accept_typed() => res?,
            };
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

        select! {
            biased;
            _ = self.dead.cancelled() => Err(Error::SessionClosed),
            res = self.inner.open_typed(typ) => res,
        }
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
        let dead = self.dead;
        let (open, accept) = self.inner.split_typed();
        (
            Heartbeat {
                runtime: runtime.clone(),
                drop_waiter: drop_waiter.clone(),
                typ,
                dead: dead.clone(),
                inner: open,
            },
            Heartbeat {
                runtime,
                drop_waiter,
                typ,
                dead,
                inner: accept,
            },
        )
    }

    fn shutdown_handle(&self) -> Option<ShutdownHandle> {
        self.inner.shutdown_handle()
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

    /// A remote that never answers heartbeats gets the session closed within
    /// interval + tolerance: accept fails, open fails, open streams fail, the
    /// transport is dropped, and the handler is told once.
    #[tokio::test(start_paused = true)]
    async fn missed_heartbeat_closes_session() {
        let (left, right) = duplex(64 * 1024);
        // The remote accepts streams but never answers anything on them.
        let mut server = Typed::new(SessionBuilder::new(left).server().start());
        let held = tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok(stream) = server.accept_typed().await {
                held.push(stream);
            }
            // accept fails once the client drops the transport.
        });

        let (handler, calls) = recorder();
        let client = Typed::new(SessionBuilder::new(right).client().start());
        let (hb, _ctl) = Heartbeat::start(client, config(Some(handler)))
            .await
            .unwrap();
        let (mut open, mut accept) = hb.split_typed();
        let mut user_stream = open.open_typed(StreamType::clamp(1)).await.unwrap();
        user_stream.write_all(b"hi").await.unwrap();

        let start = tokio::time::Instant::now();
        let res = tokio::time::timeout(Duration::from_secs(60), accept.accept_typed())
            .await
            .expect("accept should fail after a missed heartbeat");
        assert!(matches!(res, Err(Error::SessionClosed)));
        assert!(start.elapsed() <= Duration::from_secs(2) + Duration::from_millis(10));

        assert!(matches!(
            open.open_typed(StreamType::clamp(1)).await,
            Err(Error::SessionClosed)
        ));

        // A stream from the dead session doesn't hang.
        let mut buf = Vec::new();
        let read = tokio::time::timeout(Duration::from_secs(1), user_stream.read_to_end(&mut buf))
            .await
            .expect("stream read should not hang after the session closes");
        assert!(read.is_err() || buf.is_empty(), "{read:?}");

        // The transport is closed, so the remote sees the session end.
        tokio::time::timeout(Duration::from_secs(1), held)
            .await
            .expect("remote should see the transport close")
            .unwrap();

        tokio::time::sleep(Duration::from_secs(10)).await;
        assert_eq!(*calls.lock().unwrap(), vec![None]);
    }

    /// A remote that answers heartbeats keeps the session alive.
    #[tokio::test(start_paused = true)]
    async fn answered_heartbeats_keep_session() {
        let (left, right) = duplex(64 * 1024);
        let server = Typed::new(SessionBuilder::new(left).server().start());
        let (mut server, _server_ctl) = Heartbeat::start(server, config(None)).await.unwrap();
        tokio::spawn(async move { while server.accept_typed().await.is_ok() {} });

        let (handler, calls) = recorder();
        let client = Typed::new(SessionBuilder::new(right).client().start());
        let (mut hb, ctl) = Heartbeat::start(client, config(Some(handler)))
            .await
            .unwrap();

        let res = tokio::time::timeout(Duration::from_secs(30), hb.accept_typed()).await;
        assert!(res.is_err(), "session should stay open");
        ctl.beat().await.expect("on-demand heartbeat");

        let calls = calls.lock().unwrap();
        assert!(calls.len() >= 25, "{}", calls.len());
        assert!(calls.iter().all(Option::is_some), "{calls:?}");
    }

    /// An on-demand heartbeat doesn't hang when the remote never answers.
    #[tokio::test(start_paused = true)]
    async fn beat_fails_after_missed_heartbeat() {
        let (left, right) = duplex(64 * 1024);
        let mut server = Typed::new(SessionBuilder::new(left).server().start());
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok(stream) = server.accept_typed().await {
                held.push(stream);
            }
        });

        let client = Typed::new(SessionBuilder::new(right).client().start());
        let (_hb, ctl) = Heartbeat::start(client, config(None)).await.unwrap();
        let res = tokio::time::timeout(Duration::from_secs(60), ctl.beat())
            .await
            .expect("beat should not hang");
        assert!(res.is_err(), "{res:?}");
    }

    /// A local graceful close or a remote disconnect isn't reported to the
    /// handler as a missed heartbeat.
    #[tokio::test(start_paused = true)]
    async fn close_is_not_a_missed_heartbeat() {
        for local in [true, false] {
            let (left, right) = duplex(64 * 1024);
            let server = Typed::new(SessionBuilder::new(left).server().start());
            let (server, _server_ctl) = Heartbeat::start(server, config(None)).await.unwrap();
            let (mut server_open, mut server_accept) = server.split_typed();
            // Accepting runs the server's heartbeat responder.
            tokio::spawn(async move { while server_accept.accept_typed().await.is_ok() {} });
            let (server_closed_tx, server_closed_rx) = oneshot::channel::<()>();
            if !local {
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(1500)).await;
                    let _ = server_open.close(Error::None, "bye".into()).await;
                    let _ = server_closed_tx.send(());
                });
            }

            let (handler, calls) = recorder();
            let client = Typed::new(SessionBuilder::new(right).client().start());
            let (mut hb, _ctl) = Heartbeat::start(client, config(Some(handler)))
                .await
                .unwrap();
            if local {
                tokio::time::sleep(Duration::from_millis(1500)).await;
                hb.close(Error::None, "bye".into()).await.unwrap();
            } else {
                server_closed_rx.await.unwrap();
            }
            let res = tokio::time::timeout(Duration::from_secs(10), hb.accept_typed()).await;
            assert!(
                matches!(res, Ok(Err(_))),
                "local={local}: accept should fail"
            );

            tokio::time::sleep(Duration::from_secs(30)).await;
            let calls = calls.lock().unwrap();
            assert!(!calls.is_empty(), "local={local}");
            assert!(
                calls.iter().all(Option::is_some),
                "local={local}: close isn't a missed heartbeat: {calls:?}"
            );
        }
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

    /// A heartbeat stream that fails, or a reply that doesn't match the
    /// request, closes the session without reporting a missed heartbeat.
    #[tokio::test(start_paused = true)]
    async fn heartbeat_stream_failure_closes_session() {
        for mismatch in [false, true] {
            let (left, right) = duplex(64 * 1024);
            let mut server = Typed::new(SessionBuilder::new(left).server().start());
            let server = tokio::spawn(async move {
                let mut held = Vec::new();
                while let Ok(mut stream) = server.accept_typed().await {
                    let mut buf = [0u8; 4];
                    if stream.read_exact(&mut buf).await.is_err() {
                        continue;
                    }
                    if mismatch {
                        buf[0] = !buf[0];
                        let _ = stream.write_all(&buf).await;
                        held.push(stream);
                    }
                    // Otherwise drop the stream without answering.
                }
            });

            let (handler, calls) = recorder();
            let client = Typed::new(SessionBuilder::new(right).client().start());
            let cfg = HeartbeatConfig {
                interval: Duration::from_secs(10),
                tolerance: Duration::from_secs(10),
                handler: Some(handler),
            };
            let (mut hb, _ctl) = Heartbeat::start(client, cfg).await.unwrap();

            // Well before the heartbeat timeout.
            let res = tokio::time::timeout(Duration::from_secs(1), hb.accept_typed())
                .await
                .unwrap_or_else(|_| panic!("mismatch={mismatch}: accept should fail"));
            assert!(matches!(res, Err(Error::SessionClosed)));
            tokio::time::timeout(Duration::from_secs(1), server)
                .await
                .unwrap_or_else(|_| panic!("mismatch={mismatch}: remote should see the close"))
                .unwrap();

            tokio::time::sleep(Duration::from_secs(60)).await;
            let calls = calls.lock().unwrap();
            assert!(calls.is_empty(), "mismatch={mismatch}: {calls:?}");
        }
    }

    /// Once a heartbeat is missed, a handler that takes a while over the
    /// `None` call runs to completion even if the heartbeat is torn down in
    /// the meantime, as happens when the owner reconnects.
    #[tokio::test(start_paused = true)]
    async fn slow_handler_survives_teardown() {
        let (left, right) = duplex(64 * 1024);
        let mut server = Typed::new(SessionBuilder::new(left).server().start());
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok(stream) = server.accept_typed().await {
                held.push(stream);
            }
        });

        let finished = Arc::new(Mutex::new(Vec::new()));
        let f = finished.clone();
        let handler = move |lat: Option<Duration>| {
            let f = f.clone();
            async move {
                if lat.is_none() {
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
                f.lock().unwrap().push(lat);
                Ok::<(), Box<dyn StdError>>(())
            }
        };
        let client = Typed::new(SessionBuilder::new(right).client().start());
        let (mut hb, ctl) = Heartbeat::start(client, config(Some(Arc::new(handler))))
            .await
            .unwrap();

        let res = tokio::time::timeout(Duration::from_secs(60), hb.accept_typed())
            .await
            .expect("accept should fail after a missed heartbeat");
        assert!(matches!(res, Err(Error::SessionClosed)));
        drop(ctl);
        drop(hb);

        tokio::time::sleep(Duration::from_secs(10)).await;
        assert_eq!(*finished.lock().unwrap(), vec![None]);
    }

    /// An open that is waiting on the wrapped session when a heartbeat is
    /// missed fails rather than hanging.
    #[tokio::test(start_paused = true)]
    async fn pending_open_fails_after_missed_heartbeat() {
        // A session whose opens never finish and that can't be shut down.
        struct Stuck(Typed<crate::MuxadoSession>);
        #[async_trait]
        impl TypedAccept for Stuck {
            async fn accept_typed(&mut self) -> Result<TypedStream, Error> {
                self.0.accept_typed().await
            }
        }
        #[async_trait]
        impl TypedOpenClose for Stuck {
            async fn open_typed(&mut self, typ: StreamType) -> Result<TypedStream, Error> {
                if typ == HEARTBEAT_TYPE {
                    self.0.open_typed(typ).await
                } else {
                    future::pending().await
                }
            }
            async fn close(&mut self, error: Error, msg: String) -> Result<(), Error> {
                self.0.close(error, msg).await
            }
        }
        impl TypedSession for Stuck {
            type TypedAccept = Self;
            type TypedOpen = Self;
            fn split_typed(self) -> (Self, Self) {
                unimplemented!()
            }
        }

        let (left, right) = duplex(64 * 1024);
        let mut server = Typed::new(SessionBuilder::new(left).server().start());
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok(stream) = server.accept_typed().await {
                held.push(stream);
            }
        });

        let client = Stuck(Typed::new(SessionBuilder::new(right).client().start()));
        let (mut hb, _ctl) = Heartbeat::start(client, config(None)).await.unwrap();
        let res =
            tokio::time::timeout(Duration::from_secs(60), hb.open_typed(StreamType::clamp(1)))
                .await
                .expect("open should not hang after a missed heartbeat");
        assert!(matches!(res, Err(Error::SessionClosed)));
    }
}
