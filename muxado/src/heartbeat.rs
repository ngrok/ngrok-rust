use std::{
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
use futures::FutureExt;
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
    errors::ErrorType,
    typed::{
        StreamType,
        TypedSession,
        TypedStream,
    },
};

const HEARTBEAT_TYPE: StreamType = StreamType::clamp(0xFFFFFFFF);

pub struct Heartbeat<S, F> {
    typ: StreamType,
    inner: S,

    durations: Arc<(AtomicU64, AtomicU64)>,

    callback: Option<Option<F>>,

    on_demand: Option<mpsc::Sender<oneshot::Sender<Duration>>>,
}

pub struct HeartbeatConfig<F = fn(Duration)> {
    pub interval: Duration,
    pub tolerance: Duration,
    pub callback: Option<F>,
}

impl<F> Default for HeartbeatConfig<F> {
    fn default() -> Self {
        HeartbeatConfig {
            interval: Duration::from_secs(10),
            tolerance: Duration::from_secs(15),
            callback: None,
        }
    }
}

impl<S, F> Heartbeat<S, F>
where
    S: TypedSession + 'static,
    F: FnMut(Duration) + Send + 'static,
{
    pub fn new(sess: S, cfg: HeartbeatConfig<F>) -> Self {
        Heartbeat {
            typ: HEARTBEAT_TYPE,
            inner: sess,
            durations: Arc::new((
                (cfg.interval.as_nanos() as u64).into(),
                (cfg.tolerance.as_nanos() as u64).into(),
            )),
            callback: Some(cfg.callback),
            on_demand: None,
        }
    }

    pub fn set_interval(&self, interval: Duration) {
        self.durations
            .0
            .store(interval.as_nanos() as u64, Ordering::Relaxed);
    }

    pub fn set_tolerance(&self, tolerance: Duration) {
        self.durations
            .1
            .store(tolerance.as_nanos() as u64, Ordering::Relaxed);
    }

    pub async fn start(&mut self) -> Result<(), io::Error> {
        let (dtx, drx) = mpsc::channel(1);
        self.on_demand = Some(dtx);
        let (mtx, mrx) = mpsc::channel(1);
        self.start_requester(drx, mtx).await?;
        self.start_check(mrx)?;

        Ok(())
    }

    fn start_check(&mut self, mut mark: mpsc::Receiver<Duration>) -> Result<(), io::Error> {
        let (mut interval, mut tolerance) = self.get_durations();
        let durations = self.durations.clone();

        let mut cb = self.callback.take().ok_or_else(|| {
            io::Error::new(io::ErrorKind::AlreadyExists, "heartbeat already started")
        })?;

        tokio::spawn(
            async move {
                let mut deadline = tokio::time::Instant::now() + interval + tolerance;
                loop {
                    match tokio::time::timeout_at(deadline, mark.recv()).await {
                        Err(_e) => {
                            cb.as_mut().map(|cb| cb(Duration::from_secs(0)));
                        }
                        Ok(Some(lat)) => {
                            cb.as_mut().map(|cb| cb(lat));
                        }
                        Ok(None) => {
                            return;
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
            .then(|_| async move {
                tracing::debug!("check exited");
            }),
        );

        Ok(())
    }

    async fn start_requester(
        &mut self,
        mut on_demand: mpsc::Receiver<oneshot::Sender<Duration>>,
        mark: mpsc::Sender<Duration>,
    ) -> Result<(), io::Error> {
        let mut stream = self
            .inner
            .open_typed(self.typ)
            .await
            .map_err(|_| io::ErrorKind::ConnectionReset)?;

        let (interval, _) = self.get_durations();
        let mut ticker = tokio::time::interval(interval);

        tokio::spawn(
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
            .then(|_| async move {
                tracing::debug!("requester exited");
            }),
        );

        Ok(())
    }

    fn get_durations(&self) -> (Duration, Duration) {
        get_durations(&self.durations)
    }

    fn start_responder(&mut self, mut stream: TypedStream) {
        tokio::spawn(async move {
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
        });
    }
}

#[async_trait]
impl<S, F> TypedSession for Heartbeat<S, F>
where
    S: TypedSession + 'static,
    F: FnMut(Duration) + Send + 'static,
{
    async fn accept_typed(&mut self) -> Result<TypedStream, ErrorType> {
        loop {
            let stream = self.inner.accept_typed().await?;
            let typ = stream.typ();

            if typ == self.typ {
                self.start_responder(stream);
                continue;
            }

            return Ok(stream);
        }
    }

    async fn open_typed(&mut self, typ: StreamType) -> Result<TypedStream, ErrorType> {
        // Don't open a heartbeat stream manually
        if typ == self.typ {
            return Err(ErrorType::StreamRefused);
        }

        self.inner.open_typed(typ).await
    }
}

fn get_durations(durations: &Arc<(AtomicU64, AtomicU64)>) -> (Duration, Duration) {
    (
        Duration::from_nanos(durations.0.load(Ordering::Relaxed)),
        Duration::from_nanos(durations.1.load(Ordering::Relaxed)),
    )
}
