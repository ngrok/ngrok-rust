use std::{
    collections::HashMap,
    env,
    io,
    num::ParseIntError,
    sync::Arc,
    time::Duration,
};

use arc_swap::ArcSwap;
use async_rustls::rustls::{
    self,
    client::InvalidDnsNameError,
};
use futures::{
    future::BoxFuture,
    FutureExt,
};
use muxado::heartbeat::HeartbeatConfig;
use rustls_pemfile::Item;
use thiserror::Error;
use tokio::{
    io::{
        AsyncRead,
        AsyncWrite,
    },
    sync::{
        mpsc::{
            channel,
            Sender,
        },
        Mutex,
        RwLock,
    },
};
use tokio_retry::{
    strategy::ExponentialBackoff,
    Retry,
};
use tokio_util::compat::{
    FuturesAsyncReadCompatExt,
    TokioAsyncReadCompatExt,
};
use tracing::{
    debug,
    warn,
};

pub use crate::internals::raw_session::RpcError;
use crate::{
    config::{
        HttpTunnelBuilder,
        LabeledTunnelBuilder,
        TcpTunnelBuilder,
        TlsTunnelBuilder,
        TunnelConfig,
    },
    internals::{
        proto::{
            AuthExtra,
            BindExtra,
            BindOpts,
        },
        raw_session::{
            AcceptError as RawAcceptError,
            IncomingStreams,
            RawSession,
            RpcClient,
            StartSessionError,
        },
    },
    tunnel::{
        AcceptError,
        Conn,
        TunnelInner,
    },
};

const CERT_BYTES: &[u8] = include_bytes!("../assets/ngrok.ca.crt");
const NOT_IMPLEMENTED: &str = "the agent has not defined a callback for this operation";

#[derive(Clone)]
struct BoundTunnel {
    proto: String,
    opts: Option<BindOpts>,
    extra: BindExtra,
    labels: HashMap<String, String>,
    forwards_to: String,
    tx: Sender<Result<Conn, AcceptError>>,
}

type TunnelConns = HashMap<String, BoundTunnel>;

/// An ngrok session.
#[derive(Clone)]
pub struct Session {
    inner: Arc<ArcSwap<SessionInner>>,
}

struct SessionInner {
    client: Mutex<RpcClient>,
    tunnels: RwLock<TunnelConns>,
    builder: SessionBuilder,
}

/// A trait alias for types that can provide the base ngrok transport, i.e.
/// bidirectional byte streams.
///
/// It is blanket-implemented for all types that satisfy its bounds. Most
/// commonly, it will be a tls-wrapped tcp stream.
pub trait IoStream: AsyncRead + AsyncWrite + Unpin + Send + 'static {}
impl<T> IoStream for T where T: AsyncRead + AsyncWrite + Unpin + Send + 'static {}

/// The function used to establish the connection to the ngrok server.
///
/// This is effectively `async |addr, tls_config| -> Result<IoStream>`, with all
/// of the necessary boxing to turn the generics into trait objects.
pub type ConnectCallback = Arc<
    dyn Fn(
            String,
            Arc<rustls::ClientConfig>,
        ) -> BoxFuture<'static, Result<Box<dyn IoStream>, ConnectError>>
        + Send
        + Sync
        + 'static,
>;

fn default_connect() -> ConnectCallback {
    Arc::new(|addr, tls_config| {
        async move {
            let mut split = addr.split(':');
            let host = split.next().unwrap();
            let port = split
                .next()
                .map(str::parse::<u16>)
                .transpose()?
                .unwrap_or(443);
            let conn = tokio::net::TcpStream::connect(&(host, port))
                .await
                .map_err(ConnectError::Tcp)?
                .compat();

            let tls_conn = async_rustls::TlsConnector::from(tls_config)
                .connect(rustls::ServerName::try_from(host)?, conn)
                .await
                .map_err(ConnectError::Tls)?;
            Ok(Box::new(tls_conn.compat()) as Box<dyn IoStream>)
        }
        .boxed()
    })
}

/// The builder for an ngrok [Session].
#[derive(Clone)]
pub struct SessionBuilder {
    authtoken: Option<String>,
    metadata: Option<String>,
    heartbeat_interval: Option<Duration>,
    heartbeat_tolerance: Option<Duration>,
    server_addr: String,
    tls_config: rustls::ClientConfig,
    connect_callback: ConnectCallback,
    cookie: Option<String>,
    id: Option<String>,
}

/// Errors arising at [SessionBuilder::connect] time.
#[derive(Error, Debug)]
pub enum ConnectError {
    /// The builder specified an invalid heartbeat interval.
    ///
    /// This is most likely caused a [Duration] that's outside of the [i64::MAX]
    /// nanosecond range.
    #[error("invalid heartbeat interval: {0}")]
    InvalidHeartbeatInterval(u128),
    /// The builder specified an invalid heartbeat tolerance.
    ///
    /// This is most likely caused a [Duration] that's outside of the [i64::MAX]
    /// nanosecond range.
    #[error("invalid heartbeat tolerance: {0}")]
    InvalidHeartbeatTolerance(u128),
    /// The builder specified an invalid port.
    #[error("invalid server port")]
    InvalidServerPort(#[from] ParseIntError),
    /// The builder specified an invalid server name.
    #[error("invalid server name")]
    InvalidServerName(#[from] InvalidDnsNameError),
    /// An error occurred when establishing a TCP connection to the ngrok
    /// server.
    #[error("failed to establish tcp connection")]
    Tcp(io::Error),
    /// A TLS handshake error occurred.
    ///
    /// This is usually a certificate validation issue, or an attempt to connect
    /// to something that doesn't actually speak TLS.
    #[error("tls handshake error")]
    Tls(io::Error),
    /// An error occurred when starting the ngrok session.
    ///
    /// This might occur when there's a protocol mismatch interfering with the
    /// heartbeat routine.
    #[error("failed to start ngrok session")]
    Start(StartSessionError),
    /// An error occurred when attempting to authenticate.
    #[error("authentication failure")]
    Auth(RpcError),
}

impl Default for SessionBuilder {
    fn default() -> Self {
        let mut root_store = rustls::RootCertStore::empty();
        let mut cert_pem = io::Cursor::new(CERT_BYTES);
        root_store.add_parsable_certificates(
            rustls_pemfile::read_all(&mut cert_pem)
                .expect("a valid ngrok root certificate")
                .into_iter()
                .filter_map(|it| match it {
                    Item::X509Certificate(bs) => Some(bs),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .as_slice(),
        );

        let tls_config = rustls::ClientConfig::builder()
            .with_safe_defaults()
            .with_root_certificates(root_store)
            .with_no_client_auth();

        SessionBuilder {
            authtoken: None,
            metadata: None,
            heartbeat_interval: None,
            heartbeat_tolerance: None,
            server_addr: "tunnel.ngrok.com:443".into(),
            tls_config,
            connect_callback: default_connect(),
            cookie: None,
            id: None,
        }
    }
}

impl SessionBuilder {
    /// Authenticate the ngrok session with the given authtoken.
    pub fn authtoken(mut self, authtoken: impl Into<String>) -> Self {
        self.authtoken = Some(authtoken.into());
        self
    }

    /// Authenticate using the authtoken in the `NGROK_AUTHTOKEN` environment
    /// variable.
    pub fn authtoken_from_env(mut self) -> Self {
        self.authtoken = env::var("NGROK_AUTHTOKEN").ok();
        self
    }

    /// Set the heartbeat interval for the session.
    /// This value determines how often we send application level
    /// heartbeats to the server go check connection liveness.
    pub fn heartbeat_interval(mut self, heartbeat_interval: Duration) -> Self {
        self.heartbeat_interval = Some(heartbeat_interval);
        self
    }

    /// Set the heartbeat tolerance for the session.
    /// If the session's heartbeats are outside of their interval by this duration,
    /// the server will assume the session is dead and close it.
    pub fn heartbeat_tolerance(mut self, heartbeat_tolerance: Duration) -> Self {
        self.heartbeat_tolerance = Some(heartbeat_tolerance);
        self
    }

    /// Use the provided opaque metadata string for this session.
    /// Viewable from the ngrok dashboard or API.
    pub fn metadata(mut self, metadata: impl Into<String>) -> Self {
        self.metadata = Some(metadata.into());
        self
    }

    /// Connect to the provided ngrok server address.
    pub fn with_server_addr(&mut self, addr: impl Into<String>) -> &mut Self {
        self.server_addr = addr.into();
        self
    }

    /// Use the provided tls config when connecting to the ngrok server.
    pub fn tls_config(mut self, config: rustls::ClientConfig) -> Self {
        self.tls_config = config;
        self
    }

    /// Set the function used to establish the connection to the ngrok server.
    pub fn with_connect_callback(&mut self, callback: ConnectCallback) -> &mut Self {
        self.connect_callback = callback;
        self
    }

    /// Attempt to establish an ngrok session using the current configuration.
    pub async fn connect(&self) -> Result<Session, ConnectError> {
        let (inner, incoming) = self.connect_inner().await?;

        let inner = Arc::new(ArcSwap::new(inner.into()));

        tokio::spawn(accept_incoming(incoming, inner.clone()));

        Ok(Session { inner })
    }

    async fn connect_inner(&self) -> Result<(SessionInner, IncomingStreams), ConnectError> {
        let conn =
            (self.connect_callback)(self.server_addr.clone(), Arc::new(self.tls_config.clone()))
                .await?;

        let mut heartbeat_config = HeartbeatConfig::<fn(Duration)>::default();
        if let Some(interval) = self.heartbeat_interval {
            heartbeat_config.interval = interval;
        }
        if let Some(tolerance) = self.heartbeat_tolerance {
            heartbeat_config.tolerance = tolerance;
        }
        // convert these while we have ownership
        let interval_nanos = heartbeat_config.interval.as_nanos();
        let heartbeat_interval = i64::try_from(interval_nanos)
            .map_err(|_| ConnectError::InvalidHeartbeatInterval(interval_nanos))?;
        let tolerance_nanos = heartbeat_config.interval.as_nanos();
        let heartbeat_tolerance = i64::try_from(tolerance_nanos)
            .map_err(|_| ConnectError::InvalidHeartbeatTolerance(tolerance_nanos))?;

        let mut raw = RawSession::start(conn, heartbeat_config)
            .await
            .map_err(ConnectError::Start)?;

        // list of possibilities: https://doc.rust-lang.org/std/env/consts/constant.OS.html
        let os = match env::consts::OS {
            "macos" => "darwin",
            _ => env::consts::OS,
        };

        let resp = raw
            .auth(
                self.id.as_deref().unwrap_or_default(),
                AuthExtra {
                    version: env!("CARGO_PKG_VERSION").into(),
                    auth_token: self.authtoken.clone().unwrap_or_default(),
                    metadata: self.metadata.clone().unwrap_or_default(),
                    os: os.into(),
                    arch: std::env::consts::ARCH.into(),
                    heartbeat_interval,
                    heartbeat_tolerance,
                    restart_unsupported_error: Some(NOT_IMPLEMENTED.into()),
                    stop_unsupported_error: Some(NOT_IMPLEMENTED.into()),
                    update_unsupported_error: Some(NOT_IMPLEMENTED.into()),
                    client_type: "library/official/rust".into(),
                    cookie: self.cookie.clone().unwrap_or_default(),
                    ..Default::default()
                },
            )
            .await
            .map_err(ConnectError::Auth)?;

        let (client, incoming) = raw.split();

        let builder = SessionBuilder {
            cookie: resp.extra.cookie,
            id: resp.client_id.into(),
            ..self.clone()
        };

        Ok((
            SessionInner {
                client: client.into(),
                tunnels: Default::default(),
                builder,
            },
            incoming,
        ))
    }
}

impl Session {
    /// Create a new [SessionBuilder] to configure a new ngrok session.
    pub fn builder() -> SessionBuilder {
        SessionBuilder::default()
    }

    /// Start building a tunnel backing an HTTP endpoint.
    pub fn http_endpoint(&self) -> HttpTunnelBuilder {
        self.clone().into()
    }

    /// Start building a tunnel backing a TCP endpoint.
    pub fn tcp_endpoint(&self) -> TcpTunnelBuilder {
        self.clone().into()
    }

    /// Start building a tunnel backing a TLS endpoint.
    pub fn tls_endpoint(&self) -> TlsTunnelBuilder {
        self.clone().into()
    }

    /// Start building a labeled tunnel.
    pub fn labeled_tunnel(&self) -> LabeledTunnelBuilder {
        self.clone().into()
    }

    /// Start a new tunnel in this session.
    pub(crate) async fn start_tunnel<C>(&self, tunnel_cfg: C) -> Result<TunnelInner, RpcError>
    where
        C: TunnelConfig,
    {
        let inner = self.inner.load();
        let mut client = inner.client.lock().await;

        // let tunnelCfg: dyn TunnelConfig = TunnelConfig(opts);
        let (tx, rx) = channel(64);

        let proto = tunnel_cfg.proto();
        let opts = tunnel_cfg.opts();
        let mut extra = tunnel_cfg.extra();
        let labels = tunnel_cfg.labels();
        let forwards_to = tunnel_cfg.forwards_to();

        // non-labeled tunnel
        let (tunnel, bound) = if tunnel_cfg.proto() != "" {
            let resp = client
                .listen(
                    &proto,
                    opts.clone().unwrap(), // this is crate-defined, and must exist if proto is non-empty
                    extra.clone(),
                    "",
                    &forwards_to,
                )
                .await?;

            extra.token = resp.extra.token;

            (
                TunnelInner {
                    id: resp.client_id,
                    proto: resp.proto.clone(),
                    url: resp.url,
                    labels: HashMap::new(),
                    forwards_to: tunnel_cfg.forwards_to(),
                    metadata: extra.metadata.clone(),
                    session: self.clone(),
                    incoming: rx,
                },
                BoundTunnel {
                    proto: resp.proto,
                    opts: resp.bind_opts.into(),
                    extra,
                    labels,
                    forwards_to,
                    tx,
                },
            )
        } else {
            // labeled tunnel
            let resp = client
                .listen_label(labels.clone(), &extra.metadata, &forwards_to)
                .await?;

            (
                TunnelInner {
                    id: resp.id,
                    proto: Default::default(),
                    url: Default::default(),
                    labels: tunnel_cfg.labels(),
                    forwards_to: tunnel_cfg.forwards_to(),
                    metadata: extra.metadata.clone(),
                    session: self.clone(),
                    incoming: rx,
                },
                BoundTunnel {
                    extra,
                    proto: Default::default(),
                    opts: Default::default(),
                    forwards_to,
                    labels,
                    tx,
                },
            )
        };

        let mut tunnels = inner.tunnels.write().await;
        tunnels.insert(tunnel.id.clone(), bound);

        Ok(tunnel)
    }

    /// Close a tunnel with the given ID.
    pub async fn close_tunnel(&self, id: impl AsRef<str>) -> Result<(), RpcError> {
        let id = id.as_ref();
        let inner = self.inner.load();
        inner.client.lock().await.unlisten(id).await?;
        inner.tunnels.write().await.remove(id);
        Ok(())
    }
}

async fn accept_one(
    incoming: &mut IncomingStreams,
    inner: &ArcSwap<SessionInner>,
) -> Result<(), AcceptError> {
    let conn = match incoming.accept().await {
        Ok(conn) => conn,
        // Assume if we got a muxado error, the session is borked. Break and
        // propagate the error to all of the tunnels out in the wild.
        Err(RawAcceptError::Transport(error)) => return Err(error.into()),
        // The other errors are either a bad header or an unrecognized
        // stream type. They're non-fatal, but could signal a protocol
        // mismatch.
        Err(error) => {
            warn!(?error, "protocol error when accepting tunnel connection");
            return Ok(());
        }
    };
    let id = conn.header.id.clone();
    let remote_addr = conn.header.client_addr.parse().unwrap_or_else(|error| {
        warn!(
            client_addr = conn.header.client_addr,
            %error,
            "invalid remote addr for tunnel connection",
        );
        "0.0.0.0:0".parse().unwrap()
    });
    let inner = inner.load();
    let guard = inner.tunnels.read().await;
    let res = if let Some(tun) = guard.get(&id) {
        tun.tx
            .send(Ok(Conn {
                remote_addr,
                stream: conn.stream,
            }))
            .await
    } else {
        Ok(())
    };
    drop(guard);
    if res.is_err() {
        RwLock::write(&inner.tunnels).await.remove(&id);
    }
    Ok(())
}

async fn try_reconnect(inner: Arc<ArcSwap<SessionInner>>) -> Result<IncomingStreams, AcceptError> {
    let old_inner = inner.load();
    let (new_inner, new_incoming) = old_inner
        .builder
        .connect_inner()
        .await
        .map_err(|_| AcceptError::Transport(muxado::Error::SessionClosed))?;
    let mut client = new_inner.client.lock().await;
    let mut new_tunnels = new_inner.tunnels.write().await;
    let old_tunnels = old_inner.tunnels.read().await;

    for (id, tun) in old_tunnels.iter() {
        if !tun.proto.is_empty() {
            let resp = client
                .listen(
                    &tun.proto,
                    tun.opts.clone().unwrap(),
                    tun.extra.clone(),
                    id,
                    &tun.forwards_to,
                )
                .await
                .map_err(|_| AcceptError::Transport(muxado::Error::ErrorUnknown))?;
            debug!(?resp, %id, %tun.proto, ?tun.opts, ?tun.extra, %tun.forwards_to, "rebound tunnel");
            new_tunnels.insert(id.clone(), tun.clone());
        } else {
            let resp = client
                .listen_label(tun.labels.clone(), &tun.extra.metadata, &tun.forwards_to)
                .await
                .map_err(|_| AcceptError::Transport(muxado::Error::ErrorUnknown))?;

            if !resp.id.is_empty() {
                new_tunnels.insert(resp.id, tun.clone());
            } else {
                new_tunnels.insert(id.clone(), tun.clone());
            }
        }
    }

    drop(old_tunnels);
    drop(client);
    drop(new_tunnels);
    inner.store(new_inner.into());

    Ok(new_incoming)
}

async fn accept_incoming(mut incoming: IncomingStreams, inner: Arc<ArcSwap<SessionInner>>) {
    let error: AcceptError = loop {
        if let Err(error) = accept_one(&mut incoming, &inner).await {
            debug!(%error, "failed to accept stream, attempting reconnect");
            let reconnect = Retry::spawn(ExponentialBackoff::from_millis(50), || {
                try_reconnect(inner.clone())
            });
            incoming = match reconnect.await {
                Ok(incoming) => incoming,
                Err(error) => {
                    debug!(%error, "reconnect failed, giving up");
                    break error;
                }
            };
        }
    };
    for (_id, tun) in inner.load().tunnels.write().await.drain() {
        let _ = tun.tx.send(Err(error)).await;
    }
}
