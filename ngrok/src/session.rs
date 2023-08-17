use std::{
    collections::{
        HashMap,
        VecDeque,
    },
    env,
    future::Future,
    io,
    num::ParseIntError,
    sync::{
        atomic::{
            AtomicBool,
            Ordering,
        },
        Arc,
    },
    time::Duration,
};

use arc_swap::ArcSwap;
use async_rustls::rustls::{
    self,
    client::InvalidDnsNameError,
};
use async_trait::async_trait;
use bytes::Bytes;
use futures::{
    future,
    prelude::*,
    FutureExt,
};
use muxado::heartbeat::HeartbeatConfig;
pub use muxado::heartbeat::HeartbeatHandler;
use once_cell::sync::OnceCell;
use regex::Regex;
use rustls_pemfile::Item;
use thiserror::Error;
use tokio::{
    io::{
        AsyncRead,
        AsyncWrite,
    },
    runtime::Handle,
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
    RetryIf,
};
use tokio_util::compat::{
    FuturesAsyncReadCompatExt,
    TokioAsyncReadCompatExt,
};
use tracing::{
    debug,
    warn,
};

pub use crate::internals::{
    proto::{
        CommandResp,
        Restart,
        Stop,
        Update,
    },
    raw_session::{
        CommandHandler,
        RpcError,
    },
};
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
            NgrokError,
            SecretString,
        },
        raw_session::{
            AcceptError as RawAcceptError,
            CommandHandlers,
            IncomingStreams,
            RawSession,
            RpcClient,
            StartSessionError,
            NOT_IMPLEMENTED,
        },
    },
    tunnel::{
        AcceptError,
        Conn,
        TunnelInner,
    },
};

pub(crate) const CERT_BYTES: &[u8] = include_bytes!("../assets/ngrok.ca.crt");
const CLIENT_TYPE: &str = "ngrok-rust";
const VERSION: &str = env!("CARGO_PKG_VERSION");

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
///
/// Encapsulates an established session with the ngrok service. Sessions recover
/// from network failures by automatically reconnecting.
#[derive(Clone)]
pub struct Session {
    // Note: this is implicitly used to detect when the session (and its
    // tunnels) have been dropped in order to shut down the accept loop.
    #[allow(dead_code)]
    dropref: awaitdrop::Ref,
    inner: Arc<ArcSwap<SessionInner>>,
}

struct SessionInner {
    runtime: Handle,
    client: Mutex<RpcClient>,
    closed: AtomicBool,
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

/// Trait for establishing the connection to the ngrok server.
#[async_trait]
pub trait Connector: Sync + Send + 'static {
    /// The function used to establish the connection to the ngrok server.
    ///
    /// This is effectively `async |addr, tls_config, err| -> Result<IoStream>`.
    ///
    /// If it is being called due to a disconnect, the [AcceptError] argument will
    /// be populated.
    ///
    /// If it returns `Err(ConnectError::Canceled)`, reconnecting will be canceled
    /// and the session will be terminated. Note that this error will never be
    /// returned from the [default_connect] function.
    async fn connect(
        &self,
        addr: String,
        tls_config: Arc<rustls::ClientConfig>,
        err: Option<AcceptError>,
    ) -> Result<Box<dyn IoStream>, ConnectError>;
}

#[async_trait]
impl<F, U> Connector for F
where
    F: Fn(String, Arc<rustls::ClientConfig>, Option<AcceptError>) -> U + Send + Sync + 'static,
    U: Future<Output = Result<Box<dyn IoStream>, ConnectError>> + Send,
{
    async fn connect(
        &self,
        addr: String,
        tls_config: Arc<rustls::ClientConfig>,
        err: Option<AcceptError>,
    ) -> Result<Box<dyn IoStream>, ConnectError> {
        self(addr, tls_config, err).await
    }
}

/// The default ngrok connector.
///
/// Establishes a TCP connection to `addr`, and then performs a TLS handshake
/// using the `tls_config`.
///
/// Discards any errors during reconnect, allowing attempts to recur
/// indefinitely.
pub async fn default_connect(
    addr: String,
    tls_config: Arc<rustls::ClientConfig>,
    _: Option<AcceptError>,
) -> Result<Box<dyn IoStream>, ConnectError> {
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

/// The builder for an ngrok [Session].
#[derive(Clone)]
pub struct SessionBuilder {
    // Consuming libraries and applications can add a client type and version on
    // top of the "base" type and version declared by this library.
    versions: VecDeque<(String, String, Option<String>)>,
    authtoken: Option<SecretString>,
    metadata: Option<String>,
    heartbeat_interval: Option<Duration>,
    heartbeat_tolerance: Option<Duration>,
    heartbeat_handler: Option<Arc<dyn HeartbeatHandler>>,
    server_addr: String,
    ca_cert: Option<bytes::Bytes>,
    tls_config: Option<rustls::ClientConfig>,
    connector: Arc<dyn Connector>,
    handlers: CommandHandlers,
    cookie: Option<SecretString>,
    id: Option<String>,
}

/// Errors arising at [SessionBuilder::connect] time.
#[derive(Error, Debug)]
#[non_exhaustive]
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
    Start(#[source] StartSessionError),
    /// An error occurred when attempting to authenticate.
    #[error("authentication failure")]
    Auth(#[source] RpcError),
    /// An error occurred when rebinding tunnels during a reconnect
    #[error("error rebinding tunnel after reconnect")]
    Rebind(#[source] RpcError),
    /// The (re)connect function gave up.
    ///
    /// This will never be returned by the default connect function, and is
    /// instead used to cancel the reconnect loop.
    #[error("the connect function gave up")]
    Canceled,
}

impl NgrokError for ConnectError {
    fn error_code(&self) -> Option<&str> {
        match self {
            ConnectError::Auth(resp) | ConnectError::Rebind(resp) => resp.error_code(),
            _ => None,
        }
    }
    fn msg(&self) -> String {
        match self {
            ConnectError::Auth(resp) | ConnectError::Rebind(resp) => resp.msg(),
            _ => format!("{self}"),
        }
    }
}

impl Default for SessionBuilder {
    fn default() -> Self {
        SessionBuilder {
            versions: [(CLIENT_TYPE.to_string(), VERSION.to_string(), None)]
                .into_iter()
                .collect(),
            authtoken: None,
            metadata: None,
            heartbeat_interval: None,
            heartbeat_tolerance: None,
            heartbeat_handler: None,
            server_addr: "tunnel.ngrok.com:443".into(),
            ca_cert: None,
            tls_config: None,
            connector: Arc::new(default_connect),
            handlers: Default::default(),
            cookie: None,
            id: None,
        }
    }
}

fn sanitize_ua_string(s: impl AsRef<str>) -> String {
    static UA_BANNED: OnceCell<Regex> = OnceCell::new();
    UA_BANNED
        .get_or_init(|| Regex::new("[^/!#$%&'*+-.^_`|~0-9a-zA-Z]").unwrap())
        .replace_all(s.as_ref(), "#")
        .replace('/', "-")
}

impl SessionBuilder {
    /// Configures the session to authenticate with the provided authtoken. You
    /// can [find your existing authtoken] or [create a new one] in the ngrok
    /// dashboard.
    ///
    /// See the [authtoken parameter in the ngrok docs] for additional details.
    ///
    /// [find your existing authtoken]: https://dashboard.ngrok.com/get-started/your-authtoken
    /// [create a new one]: https://dashboard.ngrok.com/tunnels/authtokens
    /// [authtoken parameter in the ngrok docs]: https://ngrok.com/docs/ngrok-agent/config#authtoken
    pub fn authtoken(mut self, authtoken: impl Into<String>) -> Self {
        self.authtoken = Some(authtoken.into().into());
        self
    }
    /// Shortcut for calling [SessionBuilder::authtoken] with the value of the
    /// NGROK_AUTHTOKEN environment variable.
    pub fn authtoken_from_env(mut self) -> Self {
        self.authtoken = env::var("NGROK_AUTHTOKEN").ok().map(From::from);
        self
    }

    /// Configures how often the session will send heartbeat messages to the ngrok
    /// service to check session liveness.
    ///
    /// See the [heartbeat_interval parameter in the ngrok docs] for additional
    /// details.
    ///
    /// [heartbeat_interval parameter in the ngrok docs]: https://ngrok.com/docs/ngrok-agent/config#heartbeat_interval
    pub fn heartbeat_interval(mut self, heartbeat_interval: Duration) -> Self {
        self.heartbeat_interval = Some(heartbeat_interval);
        self
    }

    /// Configures the duration to wait for a response to a heartbeat before
    /// assuming the session connection is dead and attempting to reconnect.
    ///
    /// See the [heartbeat_tolerance parameter in the ngrok docs] for additional
    /// details.
    ///
    /// [heartbeat_tolerance parameter in the ngrok docs]: https://ngrok.com/docs/ngrok-agent/config#heartbeat_tolerance
    pub fn heartbeat_tolerance(mut self, heartbeat_tolerance: Duration) -> Self {
        self.heartbeat_tolerance = Some(heartbeat_tolerance);
        self
    }

    /// Configures the opaque, machine-readable metadata string for this session.
    /// Metadata is made available to you in the ngrok dashboard and the Agents API
    /// resource. It is a useful way to allow you to uniquely identify sessions. We
    /// suggest encoding the value in a structured format like JSON.
    ///
    /// See the [metdata parameter in the ngrok docs] for additional details.
    ///
    /// [metdata parameter in the ngrok docs]: https://ngrok.com/docs/ngrok-agent/config#metadata
    pub fn metadata(mut self, metadata: impl Into<String>) -> Self {
        self.metadata = Some(metadata.into());
        self
    }

    /// Configures the network address to dial to connect to the ngrok service.
    /// Use this option only if you are connecting to a custom agent ingress.
    ///
    /// See the [server_addr parameter in the ngrok docs] for additional details.
    ///
    /// [server_addr parameter in the ngrok docs]: https://ngrok.com/docs/ngrok-agent/config#server_addr
    pub fn server_addr(mut self, addr: impl Into<String>) -> Self {
        self.server_addr = addr.into();
        self
    }

    /// Sets the default certificate in PEM format to validate ngrok Session TLS connections.
    /// A client config set via tls_config will override this value.
    ///
    /// Roughly corresponds to the "path to a certificate PEM file" option in the
    /// [root_cas parameter in the ngrok docs]
    ///
    /// [root_cas parameter in the ngrok docs]: https://ngrok.com/docs/ngrok-agent/config#root_cas
    pub fn ca_cert(mut self, ca_cert: Bytes) -> Self {
        self.ca_cert = Some(ca_cert);
        self
    }

    /// Configures the TLS client used to connect to the ngrok service while
    /// establishing the session. Use this option only if you are connecting through
    /// a man-in-the-middle or deep packet inspection proxy. Passed to the
    /// connect callback set with `SessionBuilder::connect`.
    ///
    /// Roughly corresponds to the [root_cas parameter in the ngrok docs], but allows
    /// for deeper TLS configuration.
    ///
    /// [root_cas parameter in the ngrok docs]: https://ngrok.com/docs/ngrok-agent/config#root_cas
    pub fn tls_config(mut self, config: rustls::ClientConfig) -> Self {
        self.tls_config = Some(config);
        self
    }

    /// Configures a function which is called to establish the connection to the
    /// ngrok service. Use this option if you need to connect through an outbound
    /// proxy. In the event of network disruptions, it will be called each time
    /// the session reconnects.
    pub fn connector(mut self, connect: impl Connector) -> Self {
        self.connector = Arc::new(connect);
        self
    }

    /// Configures a function which is called when the ngrok service requests that
    /// this [Session] stops. Your application may choose to interpret this callback
    /// as a request to terminate the [Session] or the entire process.
    ///
    /// Errors returned by this function will be visible to the ngrok dashboard or
    /// API as the response to the Stop operation.
    ///
    /// Do not block inside this callback. It will cause the Dashboard or API
    /// stop operation to time out. Do not call [std::process::exit] inside this
    /// callback, it will also cause the operation to time out.
    pub fn handle_stop_command(mut self, handler: impl CommandHandler<Stop>) -> Self {
        self.handlers.on_stop = Some(Arc::new(handler));
        self
    }

    /// Configures a function which is called when the ngrok service requests
    /// that this [Session] updates. Your application may choose to interpret
    /// this callback as a request to restart the [Session] or the entire
    /// process.
    ///
    /// Errors returned by this function will be visible to the ngrok dashboard or
    /// API as the response to the Restart operation.
    ///
    /// Do not block inside this callback. It will cause the Dashboard or API
    /// stop operation to time out. Do not call [std::process::exit] inside this
    /// callback, it will also cause the operation to time out.
    pub fn handle_restart_command(mut self, handler: impl CommandHandler<Restart>) -> Self {
        self.handlers.on_restart = Some(Arc::new(handler));
        self
    }

    /// Configures a function which is called when the ngrok service requests
    /// that this [Session] updates. Your application may choose to interpret
    /// this callback as a request to update its configuration, itself, or to
    /// invoke some other application-specific behavior.
    ///
    /// Errors returned by this function will be visible to the ngrok dashboard or
    /// API as the response to the Restart operation.
    ///
    /// Do not block inside this callback. It will cause the Dashboard or API
    /// stop operation to time out. Do not call [std::process::exit] inside this
    /// callback, it will also cause the operation to time out.
    pub fn handle_update_command(mut self, handler: impl CommandHandler<Update>) -> Self {
        self.handlers.on_update = Some(Arc::new(handler));
        self
    }

    /// Call the provided handler whenever a heartbeat response is received.
    ///
    /// If the handler returns an error, the heartbeat task will exit, resulting
    /// in the session eventually dying as well.
    pub fn handle_heartbeat(mut self, callback: impl HeartbeatHandler) -> Self {
        self.heartbeat_handler = Some(Arc::new(callback));
        self
    }

    /// Add client type and version information for a client application.
    ///
    /// This is a way for applications and library consumers of this crate
    /// identify themselves.
    ///
    /// This will add a new entry to the `User-Agent` field in the "most significant"
    /// (first) position. Comments must follow [RFC 7230] or a connection error may occur.
    ///
    /// [RFC 7230]: https://datatracker.ietf.org/doc/html/rfc7230#section-3.2.6
    pub fn client_info(
        mut self,
        client_type: impl Into<String>,
        version: impl Into<String>,
        comments: Option<impl Into<String>>,
    ) -> Self {
        self.versions.push_front((
            client_type.into(),
            version.into(),
            comments.map(|c| c.into()),
        ));
        self
    }

    /// Begins a new ngrok [Session] by connecting to the ngrok service.
    /// `connect` blocks until the session is successfully established or fails with
    /// an error.
    pub async fn connect(&self) -> Result<Session, ConnectError> {
        let (dropref, dropped) = awaitdrop::awaitdrop();
        let (inner, incoming) = self.connect_inner(None).await?;

        let rt = inner.runtime.clone();

        let inner = Arc::new(ArcSwap::new(inner.into()));

        rt.spawn(future::select(
            accept_incoming(incoming, inner.clone()).boxed(),
            dropped.wait(),
        ));

        Ok(Session { dropref, inner })
    }

    pub(crate) fn get_or_create_tls_config(&self) -> rustls::ClientConfig {
        // if the user has provided a custom TLS config, use that
        if let Some(tls_config) = &self.tls_config {
            return tls_config.clone();
        }
        // generate a default TLS config
        let mut root_store = rustls::RootCertStore::empty();
        let cert_pem = self.ca_cert.as_ref().map_or(CERT_BYTES, |it| it.as_ref());
        root_store.add_parsable_certificates(
            rustls_pemfile::read_all(&mut io::Cursor::new(cert_pem))
                .expect("a valid ngrok root certificate")
                .into_iter()
                .filter_map(|it| match it {
                    Item::X509Certificate(bs) => Some(bs),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .as_slice(),
        );

        rustls::ClientConfig::builder()
            .with_safe_defaults()
            .with_root_certificates(root_store)
            .with_no_client_auth()
    }

    async fn connect_inner(
        &self,
        err: impl Into<Option<AcceptError>>,
    ) -> Result<(SessionInner, IncomingStreams), ConnectError> {
        let conn = self
            .connector
            .connect(
                self.server_addr.clone(),
                Arc::new(self.get_or_create_tls_config()),
                err.into(),
            )
            .await?;

        let mut heartbeat_config = HeartbeatConfig::default();
        if let Some(interval) = self.heartbeat_interval {
            heartbeat_config.interval = interval;
        }
        if let Some(tolerance) = self.heartbeat_tolerance {
            heartbeat_config.tolerance = tolerance;
        }
        heartbeat_config.handler = self.heartbeat_handler.clone();
        // convert these while we have ownership
        let interval_nanos = heartbeat_config.interval.as_nanos();
        let heartbeat_interval = i64::try_from(interval_nanos)
            .map_err(|_| ConnectError::InvalidHeartbeatInterval(interval_nanos))?;
        let tolerance_nanos = heartbeat_config.interval.as_nanos();
        let heartbeat_tolerance = i64::try_from(tolerance_nanos)
            .map_err(|_| ConnectError::InvalidHeartbeatTolerance(tolerance_nanos))?;

        let mut raw = RawSession::start(conn, heartbeat_config, self.handlers.clone())
            .await
            .map_err(ConnectError::Start)?;

        // list of possibilities: https://doc.rust-lang.org/std/env/consts/constant.OS.html
        let os = match env::consts::OS {
            "macos" => "darwin",
            _ => env::consts::OS,
        };

        let user_agent = self
            .versions
            .iter()
            .map(|(name, version, comments)| {
                format!(
                    "{}/{}{}",
                    sanitize_ua_string(name),
                    sanitize_ua_string(version),
                    comments
                        .as_ref()
                        .map_or(String::new(), |f| format!(" ({f})"))
                )
            })
            .collect::<Vec<_>>()
            .join(" ");

        let client_type = self.versions[0].0.clone();
        let version = self.versions[0].1.clone();

        let resp = raw
            .auth(
                self.id.as_deref().unwrap_or_default(),
                AuthExtra {
                    version,
                    client_type,
                    user_agent,
                    auth_token: self.authtoken.clone().unwrap_or_default(),
                    metadata: self.metadata.clone().unwrap_or_default(),
                    os: os.into(),
                    arch: std::env::consts::ARCH.into(),
                    heartbeat_interval,
                    heartbeat_tolerance,
                    restart_unsupported_error: self
                        .handlers
                        .on_restart
                        .is_none()
                        .then_some(NOT_IMPLEMENTED.into())
                        .or(Some("".into())),
                    stop_unsupported_error: self
                        .handlers
                        .on_stop
                        .is_none()
                        .then_some(NOT_IMPLEMENTED.into())
                        .or(Some("".into())),
                    update_unsupported_error: self
                        .handlers
                        .on_update
                        .is_none()
                        .then_some(NOT_IMPLEMENTED.into())
                        .or(Some("".into())),
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
                runtime: Handle::current(),
                client: client.into(),
                tunnels: Default::default(),
                closed: Default::default(),
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

    /// Start building a tunnel for an HTTP endpoint.
    pub fn http_endpoint(&self) -> HttpTunnelBuilder {
        self.clone().into()
    }

    /// Start building a tunnel for a TCP endpoint.
    pub fn tcp_endpoint(&self) -> TcpTunnelBuilder {
        self.clone().into()
    }

    /// Start building a tunnel for a TLS endpoint.
    pub fn tls_endpoint(&self) -> TlsTunnelBuilder {
        self.clone().into()
    }

    /// Start building a labeled tunnel.
    pub fn labeled_tunnel(&self) -> LabeledTunnelBuilder {
        self.clone().into()
    }

    /// Get the unique ID of this session.
    pub fn id(&self) -> String {
        self.inner
            .load()
            .builder
            .id
            .as_ref()
            .expect("Session ID not set")
            .clone()
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

    pub(crate) fn runtime(&self) -> Handle {
        self.inner.load().runtime.clone()
    }

    /// Close the ngrok session.
    pub async fn close(&mut self) -> Result<(), RpcError> {
        let inner = self.inner.load();
        let res = inner.client.lock().await.close().await;
        inner.closed.store(true, Ordering::SeqCst);
        res
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

async fn try_reconnect(
    inner: Arc<ArcSwap<SessionInner>>,
    err: impl Into<Option<AcceptError>>,
) -> Result<IncomingStreams, ConnectError> {
    let old_inner = inner.load();
    if old_inner.closed.load(Ordering::SeqCst) {
        return Err(ConnectError::Canceled);
    }
    let (new_inner, new_incoming) = old_inner.builder.connect_inner(err).await?;
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
                .map_err(ConnectError::Rebind)?;
            debug!(?resp, %id, %tun.proto, ?tun.opts, ?tun.extra, %tun.forwards_to, "rebound tunnel");
            new_tunnels.insert(id.clone(), tun.clone());
        } else {
            let resp = client
                .listen_label(tun.labels.clone(), &tun.extra.metadata, &tun.forwards_to)
                .await
                .map_err(ConnectError::Rebind)?;

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
            // This is gross, but should perform fine. Couple of notes:
            // * Mutex so that both the action and condition can share access to
            //   `error`. Realistically, the lock calls should be non-concurrent,
            //   but Rust can't prove that.
            // * Not setting the error in the action because then a a reference
            //   to a FnMut closure would escape via the returned Future, which is
            //   a no-no.
            let error = parking_lot::Mutex::new(Some(error));
            let reconnect = RetryIf::spawn(
                ExponentialBackoff::from_millis(50),
                || try_reconnect(inner.clone(), error.lock().clone()).map_err(Arc::new),
                |err: &Arc<ConnectError>| {
                    if let ConnectError::Canceled = **err {
                        false
                    } else {
                        *error.lock() = Some(AcceptError::Reconnect(err.clone()));
                        true
                    }
                },
            );
            incoming = match reconnect.await {
                Ok(incoming) => incoming,
                Err(error) => {
                    debug!(%error, "reconnect failed, giving up");
                    break AcceptError::Reconnect(error);
                }
            };
        }
    };
    for (_id, tun) in inner.load().tunnels.write().await.drain() {
        let _ = tun.tx.send(Err(error.clone())).await;
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_sanitize_ua() {
        assert_eq!(
            sanitize_ua_string("library/official/rust"),
            "library-official-rust"
        );
        assert_eq!(
            sanitize_ua_string("something@really☺weird"),
            "something#really#weird"
        );
    }
}
