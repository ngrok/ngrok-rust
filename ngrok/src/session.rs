use std::{
    collections::{
        HashMap,
        VecDeque,
    },
    env,
    io,
    iter,
    sync::{
        Arc,
        atomic::{
            AtomicBool,
            Ordering,
        },
    },
    time::Duration,
};

use arc_swap::ArcSwap;
use async_trait::async_trait;
use bytes::Bytes;
use futures::{
    FutureExt,
    prelude::*,
};
use futures_rustls::rustls::{
    self,
    RootCertStore,
    pki_types,
};
use hyper_http_proxy::{
    Intercept,
    Proxy,
    ProxyConnector,
};
use hyper_util::client::legacy::connect::HttpConnector;
use muxado::heartbeat::HeartbeatConfig;
pub use muxado::heartbeat::HeartbeatHandler;
use once_cell::sync::{
    Lazy,
    OnceCell,
};
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
        Mutex,
        RwLock,
        mpsc::{
            Sender,
            channel,
        },
    },
};
use tokio_retry::strategy::jitter;
use tokio_util::compat::{
    FuturesAsyncReadCompatExt,
    TokioAsyncReadCompatExt,
};
use tower_service::Service;
use tracing::{
    debug,
    warn,
};
use url::Url;

pub use crate::internals::{
    proto::{
        CommandResp,
        Restart,
        Stop,
        StopTunnel,
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
        ProxyProto,
        TcpTunnelBuilder,
        TlsTunnelBuilder,
        TunnelConfig,
    },
    conn::ConnInner,
    internals::{
        proto::{
            AuthExtra,
            BindExtra,
            BindOpts,
            Error,
            HttpEndpoint,
            SecretString,
            TcpEndpoint,
            TlsEndpoint,
        },
        raw_session::{
            AcceptError as RawAcceptError,
            CommandHandlers,
            IncomingStreams,
            NOT_IMPLEMENTED,
            RawSession,
            RpcClient,
            StartSessionError,
        },
    },
    tunnel::{
        AcceptError,
        TunnelInner,
        TunnelInnerInfo,
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
    forwards_proto: String,
    verify_upstream_tls: bool,
    tx: Sender<Result<ConnInner, AcceptError>>,
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
    _dropref: awaitdrop::Ref,
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
        host: String,
        port: u16,
        tls_config: Arc<rustls::ClientConfig>,
        err: Option<AcceptError>,
    ) -> Result<Box<dyn IoStream>, ConnectError>;
}

#[async_trait]
impl<F, U> Connector for F
where
    F: Fn(String, u16, Arc<rustls::ClientConfig>, Option<AcceptError>) -> U + Send + Sync + 'static,
    U: Future<Output = Result<Box<dyn IoStream>, ConnectError>> + Send,
{
    async fn connect(
        &self,
        host: String,
        port: u16,
        tls_config: Arc<rustls::ClientConfig>,
        err: Option<AcceptError>,
    ) -> Result<Box<dyn IoStream>, ConnectError> {
        self(host, port, tls_config, err).await
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
    host: String,
    port: u16,
    tls_config: Arc<rustls::ClientConfig>,
    _: Option<AcceptError>,
) -> Result<Box<dyn IoStream>, ConnectError> {
    let stream = tokio::net::TcpStream::connect(&(host.as_str(), port))
        .await
        .map_err(ConnectError::Tcp)?
        .compat();

    let domain = pki_types::ServerName::try_from(host)
        .expect("host should have been validated by SessionBuilder::server_addr");

    let tls_conn = futures_rustls::TlsConnector::from(tls_config)
        .connect(domain, stream)
        .await
        .map_err(ConnectError::Tls)?;
    Ok(Box::new(tls_conn.compat()) as Box<dyn IoStream>)
}

#[derive(Debug, Clone, Error)]
#[error("unsupported proxy address: {0}")]
/// An unsupported proxy address was provided.
pub struct ProxyUnsupportedError(Url);

fn connect_proxy(url: Url) -> Result<Arc<dyn Connector>, ProxyUnsupportedError> {
    Ok(match url.scheme() {
        "http" | "https" => Arc::new(connect_http_proxy(url)),
        "socks5" => {
            let host = url.host_str().unwrap_or_default();
            let port = url.port().unwrap_or(1080);
            Arc::new(connect_socks_proxy(format!("{host}:{port}")))
        }
        _ => return Err(ProxyUnsupportedError(url)),
    })
}

fn connect_http_proxy(url: Url) -> impl Connector {
    move |host: String, port, tls_config, _| {
        let mut proxy = Proxy::new(
            Intercept::All,
            url.as_str().try_into().expect("urls should be valid uris"),
        );
        proxy.force_connect();
        let mut connector = HttpConnector::new();
        connector.enforce_http(false);
        async move {
            let mut connector = ProxyConnector::from_proxy(connector, proxy)
                .map_err(|e| ConnectError::ProxyConnect(Box::new(e)))?;

            let server_uri = format!("http://{host}:{port}")
                .parse()
                .expect("host should have been validated by SessionBuilder::server_addr");

            let conn = connector
                .call(server_uri)
                .await
                .map_err(|e| ConnectError::ProxyConnect(Box::new(e)))?;

            let tls_conn = futures_rustls::TlsConnector::from(tls_config)
                .connect(
                    pki_types::ServerName::try_from(host)
                        .expect("host should have been validated by SessionBuilder::server_addr"),
                    hyper_util::rt::TokioIo::new(conn).compat(),
                )
                .await
                .map_err(ConnectError::Tls)?;

            Ok(Box::new(tls_conn.compat()) as Box<dyn IoStream>)
        }
    }
}

fn connect_socks_proxy(proxy_addr: String) -> impl Connector {
    move |server_host: String, server_port, tls_config, _| {
        let proxy_addr = proxy_addr.clone();
        async move {
            let conn = tokio_socks::tcp::Socks5Stream::connect(
                proxy_addr.as_str(),
                format!("{server_host}:{server_port}"),
            )
            .await
            .map_err(|e| ConnectError::ProxyConnect(Box::new(e)))?
            .compat();

            let tls_conn = futures_rustls::TlsConnector::from(tls_config)
                .connect(
                    pki_types::ServerName::try_from(server_host)
                        .expect("host should have been validated by SessionBuilder::server_addr"),
                    conn,
                )
                .await
                .map_err(ConnectError::Tls)?;

            Ok(Box::new(tls_conn.compat()) as Box<dyn IoStream>)
        }
    }
}

/// The builder for an ngrok [Session].
#[derive(Clone)]
pub struct SessionBuilder {
    // Consuming libraries and applications can add a client type and version on
    // top of the "base" type and version declared by this library.
    versions: VecDeque<(String, String, Option<String>)>,
    authtoken: Option<SecretString>,
    metadata: Option<String>,
    heartbeat_interval: Option<i64>,
    heartbeat_tolerance: Option<i64>,
    heartbeat_handler: Option<Arc<dyn HeartbeatHandler>>,
    server_host: String,
    server_port: u16,
    ca_cert: Option<bytes::Bytes>,
    tls_config: Option<rustls::ClientConfig>,
    connector: Arc<dyn Connector>,
    reconnect: ReconnectBackoff,
    handlers: CommandHandlers,
    cookie: Option<SecretString>,
    id: Option<String>,
}

/// How long a session waits between attempts to reconnect after it loses its
/// connection: `initial` before the second attempt (the first is immediate),
/// doubling each time up to `max`, each wait scaled by a random factor in
/// [0, 1) when `jitter` is on. It never stops reconnecting.
#[derive(Copy, Clone, Debug)]
struct ReconnectBackoff {
    initial: Duration,
    max: Duration,
    jitter: bool,
}

impl Default for ReconnectBackoff {
    fn default() -> Self {
        ReconnectBackoff {
            initial: Duration::from_millis(50),
            max: Duration::from_secs(30),
            jitter: true,
        }
    }
}

impl ReconnectBackoff {
    /// The waits of one outage: called with each failed attempt's error, it
    /// answers how long to wait before the next attempt.
    ///
    /// The cap keeps the doubling from overflowing, and each wait small
    /// enough to add to the clock.
    fn start(&self) -> impl FnMut(&ConnectError) -> Option<Duration> + Send + use<> {
        let ReconnectBackoff {
            initial,
            max,
            jitter: jittered,
        } = *self;
        let mut waits = iter::successors(Some(initial.min(max)), move |wait| {
            Some((*wait * 2).min(max))
        })
        .map(move |wait| if jittered { jitter(wait) } else { wait });
        move |_: &ConnectError| waits.next()
    }
}

/// Errors arising at [SessionBuilder::connect] time.
#[derive(Error, Debug)]
#[non_exhaustive]
pub enum ConnectError {
    /// An error occurred when establishing a TCP connection to the ngrok
    /// server.
    #[error("failed to establish tcp connection")]
    Tcp(#[source] io::Error),
    /// A TLS handshake error occurred.
    ///
    /// This is usually a certificate validation issue, or an attempt to connect
    /// to something that doesn't actually speak TLS.
    #[error("tls handshake error")]
    Tls(#[source] io::Error),
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
    /// An error arising from a failure to connect through a proxy.
    #[error("failed to connect through proxy")]
    ProxyConnect(#[source] Box<dyn std::error::Error + Send + Sync + 'static>),
    /// The (re)connect function gave up.
    ///
    /// This will never be returned by the default connect function, and is
    /// instead used to cancel the reconnect loop.
    #[error("the connect function gave up")]
    Canceled,
}

impl Error for ConnectError {
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

/// The builder specified an invalid heartbeat interval.
///
/// This is most likely caused a [Duration] that's outside of the [i64::MAX]
/// nanosecond range.
#[derive(Copy, Clone, Debug, Error)]
#[error("invalid heartbeat interval: {0}")]
pub struct InvalidHeartbeatInterval(u128);
/// The builder specified an invalid heartbeat tolerance.
///
/// This is most likely caused a [Duration] that's outside of the [i64::MAX]
/// nanosecond range.
#[derive(Copy, Clone, Debug, Error)]
#[error("invalid heartbeat tolerance: {0}")]
pub struct InvalidHeartbeatTolerance(u128);

/// The builder provided an invalid server address
#[derive(Error, Debug, Clone)]
#[error("invalid server address: {0}")]
pub struct InvalidServerAddr(String);

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
            server_host: "connect.ngrok-agent.com".into(),
            server_port: 443,
            ca_cert: None,
            tls_config: None,
            connector: Arc::new(default_connect),
            reconnect: Default::default(),
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
    pub fn authtoken(&mut self, authtoken: impl Into<String>) -> &mut Self {
        self.authtoken = Some(authtoken.into().into());
        self
    }
    /// Shortcut for calling [SessionBuilder::authtoken] with the value of the
    /// NGROK_AUTHTOKEN environment variable.
    pub fn authtoken_from_env(&mut self) -> &mut Self {
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
    pub fn heartbeat_interval(
        &mut self,
        heartbeat_interval: Duration,
    ) -> Result<&mut Self, InvalidHeartbeatInterval> {
        let nanos = heartbeat_interval.as_nanos();
        let nanos = i64::try_from(nanos).map_err(|_| InvalidHeartbeatInterval(nanos))?;
        self.heartbeat_interval = Some(nanos);
        Ok(self)
    }

    /// Configures the duration to wait for a response to a heartbeat before
    /// assuming the session connection is dead and attempting to reconnect.
    ///
    /// See the [heartbeat_tolerance parameter in the ngrok docs] for additional
    /// details.
    ///
    /// [heartbeat_tolerance parameter in the ngrok docs]: https://ngrok.com/docs/ngrok-agent/config#heartbeat_tolerance
    pub fn heartbeat_tolerance(
        &mut self,
        heartbeat_tolerance: Duration,
    ) -> Result<&mut Self, InvalidHeartbeatTolerance> {
        let nanos = heartbeat_tolerance.as_nanos();
        let nanos = i64::try_from(nanos).map_err(|_| InvalidHeartbeatTolerance(nanos))?;
        self.heartbeat_tolerance = Some(nanos);
        Ok(self)
    }

    /// Configures the opaque, machine-readable metadata string for this session.
    /// Metadata is made available to you in the ngrok dashboard and the Agents API
    /// resource. It is a useful way to allow you to uniquely identify sessions. We
    /// suggest encoding the value in a structured format like JSON.
    ///
    /// See the [metdata parameter in the ngrok docs] for additional details.
    ///
    /// [metdata parameter in the ngrok docs]: https://ngrok.com/docs/ngrok-agent/config#metadata
    pub fn metadata(&mut self, metadata: impl Into<String>) -> &mut Self {
        self.metadata = Some(metadata.into());
        self
    }

    /// Configures the network address to dial to connect to the ngrok service.
    /// Use this option only if you are connecting to a custom agent ingress.
    ///
    /// See the [server_addr parameter in the ngrok docs] for additional details.
    ///
    /// [server_addr parameter in the ngrok docs]: https://ngrok.com/docs/ngrok-agent/config#server_addr
    pub fn server_addr(&mut self, addr: impl Into<String>) -> Result<&mut Self, InvalidServerAddr> {
        let addr = addr.into();
        let server_uri: Url = format!("http://{addr}")
            .parse()
            .map_err(|_| InvalidServerAddr(addr.clone()))?;

        self.server_host = server_uri
            .host_str()
            .map(String::from)
            .ok_or_else(|| InvalidServerAddr(addr.clone()))?;

        pki_types::ServerName::try_from(self.server_host.as_str())
            .map_err(|_| InvalidServerAddr(addr.clone()))?;

        self.server_port = server_uri.port().unwrap_or(443);

        Ok(self)
    }

    /// Sets the file path to a default certificate in PEM format to validate ngrok Session TLS connections.
    /// Setting to "trusted" is the default, using the ngrok CA certificate.
    /// Setting to "host" will verify using the certificates on the host operating system.
    /// A client config set via tls_config after calling root_cas will override this value.
    ///
    /// Corresponds to the [root_cas parameter in the ngrok docs]
    ///
    /// [root_cas parameter in the ngrok docs]: https://ngrok.com/docs/ngrok-agent/config#root_cas
    pub fn root_cas(&mut self, root_cas: impl Into<String>) -> Result<&mut Self, io::Error> {
        match root_cas.into().clone().as_str() {
            "trusted" => self.ca_cert = None,
            "host" => self.tls_config = Some(host_certs_tls_config().map_err(|e| e.kind())?),
            v => {
                std::fs::read(v).map(|root_cas| self.ca_cert = Some(Bytes::from(root_cas)))?;
            }
        }
        Ok(self)
    }

    /// Sets the default certificate in PEM format to validate ngrok Session TLS connections.
    /// A client config set via tls_config will override this value.
    ///
    /// Roughly corresponds to the "path to a certificate PEM file" option in the
    /// [root_cas parameter in the ngrok docs]
    ///
    /// [root_cas parameter in the ngrok docs]: https://ngrok.com/docs/ngrok-agent/config#root_cas
    pub fn ca_cert(&mut self, ca_cert: Bytes) -> &mut Self {
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
    pub fn tls_config(&mut self, config: rustls::ClientConfig) -> &mut Self {
        self.tls_config = Some(config);
        self
    }

    /// Configures a function which is called to establish the connection to the
    /// ngrok service. Use this option if you need to connect through an outbound
    /// proxy. In the event of network disruptions, it will be called each time
    /// the session reconnects.
    pub fn connector(&mut self, connect: impl Connector) -> &mut Self {
        self.connector = Arc::new(connect);
        self
    }

    /// Configures the session to connect to ngrok through an outbound
    /// HTTP or SOCKS5 proxy. This parameter is ignored if you override the connector
    /// with [SessionBuilder::connector].
    ///
    /// See the [proxy url parameter in the ngrok docs] for additional details.
    ///
    /// [proxy url parameter in the ngrok docs]: https://ngrok.com/docs/ngrok-agent/config#proxy_url
    pub fn proxy_url(&mut self, url: Url) -> Result<&mut Self, ProxyUnsupportedError> {
        self.connector = connect_proxy(url)?;
        Ok(self)
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
    pub fn handle_stop_command(&mut self, handler: impl CommandHandler<Stop>) -> &mut Self {
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
    pub fn handle_restart_command(&mut self, handler: impl CommandHandler<Restart>) -> &mut Self {
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
    pub fn handle_update_command(&mut self, handler: impl CommandHandler<Update>) -> &mut Self {
        self.handlers.on_update = Some(Arc::new(handler));
        self
    }

    /// Call the provided handler whenever a heartbeat response is received.
    ///
    /// If the handler returns an error, the heartbeat task will exit, resulting
    /// in the session eventually dying as well.
    pub fn handle_heartbeat(&mut self, callback: impl HeartbeatHandler) -> &mut Self {
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
        &mut self,
        client_type: impl Into<String>,
        version: impl Into<String>,
        comments: Option<impl Into<String>>,
    ) -> &mut Self {
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
        let (inner, mut incoming) = self.connect_inner(None).await?;

        let rt = inner.runtime.clone();

        let inner = Arc::new(ArcSwap::new(inner.into()));

        let session = Session {
            _dropref: dropref,
            inner: inner.clone(),
        };

        // store the session for use with StopTunnel
        incoming.session = Some(session.clone());

        rt.spawn(future::select(
            accept_incoming(incoming, inner).boxed(),
            dropped.wait(),
        ));

        Ok(session)
    }

    pub(crate) fn get_or_create_tls_config(&self) -> rustls::ClientConfig {
        // if the user has provided a custom TLS config, use that
        if let Some(tls_config) = &self.tls_config {
            return tls_config.clone();
        }
        // generate a default TLS config
        let mut root_store = rustls::RootCertStore::empty();
        let cert_pem = self.ca_cert.as_ref().map_or(CERT_BYTES, |it| it.as_ref());
        let certs = rustls_pemfile::read_all(&mut io::Cursor::new(cert_pem))
            .filter_map(|it| match it {
                Ok(Item::X509Certificate(bs)) => Some(bs),
                Err(e) => {
                    warn!(error = ?e, "skipping certificate which failed to parse");
                    None
                }
                Ok(_) => {
                    warn!("skipping non-x509 certificate");
                    None
                }
            })
            .collect::<Vec<_>>();
        root_store.add_parsable_certificates(certs);

        rustls::ClientConfig::builder()
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
                self.server_host.clone(),
                self.server_port,
                Arc::new(self.get_or_create_tls_config()),
                err.into(),
            )
            .await?;

        let mut heartbeat_config = HeartbeatConfig::default();
        if let Some(interval) = self.heartbeat_interval {
            heartbeat_config.interval = Duration::from_nanos(interval as u64);
        }
        if let Some(tolerance) = self.heartbeat_tolerance {
            heartbeat_config.tolerance = Duration::from_nanos(tolerance as u64);
        }
        heartbeat_config.handler = self.heartbeat_handler.clone();

        // convert these while we have ownership
        let heartbeat_interval = heartbeat_config.interval.as_nanos() as i64;
        let heartbeat_tolerance = heartbeat_config.tolerance.as_nanos() as i64;

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
    ///
    /// https://ngrok.com/docs/http/
    pub fn http_endpoint(&self) -> HttpTunnelBuilder {
        self.clone().into()
    }

    /// Start building a tunnel for a TCP endpoint.
    ///
    /// https://ngrok.com/docs/tcp/
    pub fn tcp_endpoint(&self) -> TcpTunnelBuilder {
        self.clone().into()
    }

    /// Start building a tunnel for a TLS endpoint.
    ///
    /// https://ngrok.com/docs/tls/
    pub fn tls_endpoint(&self) -> TlsTunnelBuilder {
        self.clone().into()
    }

    /// Start building a labeled tunnel.
    ///
    /// https://ngrok.com/docs/network-edge/edges/#tunnel-group
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
        let forwards_proto = tunnel_cfg.forwards_proto();
        let verify_upstream_tls = tunnel_cfg.verify_upstream_tls();

        // non-labeled tunnel
        let (tunnel, bound) = if tunnel_cfg.proto() != "" {
            let resp = client
                .listen(
                    &proto,
                    opts.clone().unwrap(), // this is crate-defined, and must exist if proto is non-empty
                    extra.clone(),
                    "",
                    &forwards_to,
                    &forwards_proto,
                )
                .await?;

            extra.token = resp.extra.token;
            let info = TunnelInnerInfo {
                id: resp.client_id,
                proto: resp.proto.clone(),
                url: resp.url,
                labels: HashMap::new(),
                forwards_to: tunnel_cfg.forwards_to(),
                metadata: extra.metadata.clone(),
            };

            (
                TunnelInner {
                    info,
                    session: self.clone(),
                    incoming: rx.into(),
                },
                BoundTunnel {
                    proto: resp.proto,
                    opts: resp.bind_opts.into(),
                    extra,
                    labels,
                    forwards_to,
                    forwards_proto,
                    verify_upstream_tls,
                    tx,
                },
            )
        } else {
            // labeled tunnel
            let resp = client
                .listen_label(
                    labels.clone(),
                    &extra.metadata,
                    &forwards_to,
                    &forwards_proto,
                )
                .await?;

            let info = TunnelInnerInfo {
                id: resp.id,
                proto: Default::default(),
                url: Default::default(),
                labels: tunnel_cfg.labels(),
                forwards_to: tunnel_cfg.forwards_to(),
                metadata: extra.metadata.clone(),
            };

            (
                TunnelInner {
                    info,
                    session: self.clone(),
                    incoming: rx.into(),
                },
                BoundTunnel {
                    extra,
                    proto: Default::default(),
                    opts: Default::default(),
                    forwards_to,
                    forwards_proto,
                    verify_upstream_tls,
                    labels,
                    tx,
                },
            )
        };

        let mut tunnels = inner.tunnels.write().await;
        tunnels.insert(tunnel.info.id.clone(), bound);

        Ok(tunnel)
    }

    /// Close a tunnel with an error from the remote.
    /// Skips the call to unlisten, since the remote has already rejected it.
    pub(crate) async fn close_tunnel_with_error(&self, id: impl AsRef<str>, err: AcceptError) {
        let id = id.as_ref();
        let inner = self.inner.load();
        if let Some(tun) = inner.tunnels.write().await.remove(id) {
            let _ = tun.tx.send(Err(err)).await;
        };
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

pub(crate) fn host_certs_tls_config() -> Result<rustls::ClientConfig, &'static io::Error> {
    // The root certificate store, lazily loaded once.
    static ROOT_STORE: Lazy<Result<RootCertStore, io::Error>> = Lazy::new(|| {
        let der_certs = rustls_native_certs::load_native_certs()?
            .into_iter()
            .collect::<Vec<_>>();
        let mut root_store = RootCertStore::empty();
        root_store.add_parsable_certificates(der_certs);
        Ok(root_store)
    });

    let root_store = ROOT_STORE.as_ref()?;
    Ok(rustls::ClientConfig::builder()
        .with_root_certificates(root_store.clone())
        .with_no_client_auth())
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
        let mut header = conn.header;
        let app_protocol = Some(tun.forwards_proto.to_string()).filter(|s| !s.is_empty());
        let verify_upstream_tls = tun.verify_upstream_tls;
        // Note: this is a bit of a hack. Normally, passthrough_tls is only
        // a thing on edge connections, but we're making sure it's set for
        // endpoint connections as well. In their case, we have to look at the
        // options used to bind the endpoint.
        if let Some(BindOpts::Tls(opts)) = &tun.opts {
            header.passthrough_tls = opts.tls_termination.is_none();
        }
        let proxy_proto = if let Some(
            BindOpts::Tls(TlsEndpoint { proxy_proto, .. })
            | BindOpts::Http(HttpEndpoint { proxy_proto, .. })
            | BindOpts::Tcp(TcpEndpoint { proxy_proto, .. }),
        ) = tun.opts
        {
            proxy_proto
        } else {
            ProxyProto::None
        };
        tun.tx
            .send(Ok(ConnInner {
                info: crate::conn::Info {
                    app_protocol,
                    verify_upstream_tls,
                    remote_addr,
                    header,
                    proxy_proto,
                },
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
                    &tun.forwards_proto,
                )
                .await
                .map_err(ConnectError::Rebind)?;
            debug!(?resp, %id, %tun.proto, ?tun.opts, ?tun.extra, %tun.forwards_to, "rebound tunnel");
            new_tunnels.insert(id.clone(), tun.clone());
        } else {
            let resp = client
                .listen_label(
                    tun.labels.clone(),
                    &tun.extra.metadata,
                    &tun.forwards_to,
                    &tun.forwards_proto,
                )
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
            incoming = match reconnect(&inner, error).await {
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

/// Reconnects a session that lost its connection with `cause`: tries at
/// once, then again after each wait the session's backoff gives, until an
/// attempt succeeds or fails with [ConnectError::Canceled]. Each attempt
/// passes the previous one's error to the connector as its cause.
async fn reconnect(
    inner: &Arc<ArcSwap<SessionInner>>,
    mut cause: AcceptError,
) -> Result<IncomingStreams, Arc<ConnectError>> {
    // Each successful reconnect stores a clone of the builder, so this reads
    // what the caller configured.
    let mut schedule = inner.load().builder.reconnect.start();
    loop {
        let error = match try_reconnect(inner.clone(), cause).await {
            Ok(incoming) => return Ok(incoming),
            Err(error) => Arc::new(error),
        };
        if let ConnectError::Canceled = *error {
            return Err(error);
        }
        let Some(wait) = schedule(&error) else {
            return Err(error);
        };
        tokio::time::sleep(wait).await;
        cause = AcceptError::Reconnect(error);
    }
}

#[cfg(test)]
mod test {
    use std::sync::atomic::AtomicUsize;

    use muxado::typed::{
        TypedAccept,
        TypedOpenClose,
    };
    use tokio::{
        io::{
            AsyncReadExt,
            AsyncWriteExt,
            DuplexStream,
        },
        sync::mpsc,
        time::Instant,
    };

    use super::*;
    use crate::internals::proto::{
        AUTH_REQ,
        AuthResp,
    };

    /// Plays the ngrok service for one connection: accepts the session's
    /// authentication, then closes the session, which the client sees as a
    /// lost connection.
    async fn serve_auth_then_disconnect(io: DuplexStream) {
        let mut sess = muxado::typed::Typed::new(muxado::SessionBuilder::new(io).server().start());
        // Hold the other streams the client opens, such as its heartbeat,
        // so that nothing but the close below ends the session.
        let mut held = Vec::new();
        let mut auth = loop {
            let stream = sess.accept_typed().await.expect("client opens a stream");
            if stream.typ() == AUTH_REQ {
                break stream;
            }
            held.push(stream);
        };
        let mut req = Vec::new();
        while serde_json::from_slice::<serde_json::Value>(&req).is_err() {
            let mut buf = [0u8; 1024];
            let n = auth.read(&mut buf).await.expect("client sends auth");
            assert_ne!(n, 0, "client closed the auth stream before a request");
            req.extend_from_slice(&buf[..n]);
        }
        let resp = AuthResp {
            version: "2".into(),
            client_id: "fake-client-id".into(),
            extra: Default::default(),
        };
        auth.write_all(&serde_json::to_vec(&resp).unwrap())
            .await
            .expect("write auth response");
        auth.shutdown().await.expect("end auth response");
        // Closing the session drops data still in flight, so wait for the
        // client to read the response and let go of the stream.
        let _ = auth.read_to_end(&mut Vec::new()).await;
        sess.close(muxado::Error::None, "".into())
            .await
            .expect("close session");
    }

    /// What the fake ngrok service does with one connect.
    #[derive(Clone, Copy)]
    enum Answer {
        /// Accept the session's authentication, then drop the connection.
        Serve,
        /// Fail the connect as a refused TCP connection.
        Refuse,
        /// Fail the connect with ConnectError::Canceled.
        Cancel,
    }

    /// A reconnect attempt: when it started, and the cause the session
    /// passed to its connector.
    #[derive(Debug)]
    struct Attempt {
        at: Instant,
        cause: Option<AcceptError>,
    }

    /// Connects `builder` to a fake ngrok service that answers connect k
    /// (from 0, the first connect) with `script(k)`.
    ///
    /// Returns the session, which must be kept alive for it to keep
    /// reconnecting, and a receiver of every reconnect attempt, that is
    /// every connect after the first.
    async fn connect_scripted(
        builder: &mut SessionBuilder,
        script: impl Fn(usize) -> Answer + Send + Sync + 'static,
    ) -> (Session, mpsc::UnboundedReceiver<Attempt>) {
        let connects = Arc::new(AtomicUsize::new(0));
        let (attempt_tx, attempt_rx) = mpsc::unbounded_channel();
        let connector =
            move |_: String, _: u16, _: Arc<rustls::ClientConfig>, cause: Option<AcceptError>| {
                let k = connects.fetch_add(1, Ordering::SeqCst);
                if k > 0 {
                    let _ = attempt_tx.send(Attempt {
                        at: Instant::now(),
                        cause,
                    });
                }
                let answer = script(k);
                async move {
                    match answer {
                        Answer::Serve => {
                            let (client, server) = tokio::io::duplex(64 * 1024);
                            tokio::spawn(serve_auth_then_disconnect(server));
                            Ok(Box::new(client) as Box<dyn IoStream>)
                        }
                        Answer::Refuse => {
                            Err(ConnectError::Tcp(io::ErrorKind::ConnectionRefused.into()))
                        }
                        Answer::Cancel => Err(ConnectError::Canceled),
                    }
                }
            };
        let session = builder
            .connector(connector)
            .connect()
            .await
            .expect("connects to the fake ngrok service");
        (session, attempt_rx)
    }

    /// Connects `builder` to a fake ngrok service that drops the session
    /// after it starts, and fails every reconnect attempt after that.
    async fn connect_then_fail_reconnects(
        builder: &mut SessionBuilder,
    ) -> (Session, mpsc::UnboundedReceiver<Attempt>) {
        connect_scripted(builder, |k| {
            if k == 0 {
                Answer::Serve
            } else {
                Answer::Refuse
            }
        })
        .await
    }

    /// The waits between the next `n + 1` reconnect attempts.
    ///
    /// On tokio's paused clock these are exact when every wait is a whole
    /// number of milliseconds, since the clock jumps straight to each
    /// timer's deadline and timers fire on millisecond ticks.
    async fn reconnect_gaps(
        attempts: &mut mpsc::UnboundedReceiver<Attempt>,
        n: usize,
    ) -> Vec<Duration> {
        let mut last = next_attempt(attempts).await.at;
        let mut gaps = Vec::new();
        for _ in 0..n {
            let attempt = next_attempt(attempts).await.at;
            gaps.push(attempt - last);
            last = attempt;
        }
        gaps
    }

    async fn next_attempt(attempts: &mut mpsc::UnboundedReceiver<Attempt>) -> Attempt {
        attempts.recv().await.expect("session keeps reconnecting")
    }

    /// Asserts that the session makes no reconnect attempt for a day, which
    /// on tokio's paused clock takes no real time.
    async fn assert_no_attempt_for_a_day(attempts: &mut mpsc::UnboundedReceiver<Attempt>) {
        let next = tokio::time::timeout(Duration::from_secs(24 * 60 * 60), attempts.recv()).await;
        assert!(next.is_err(), "the session reconnected again: {next:?}");
    }

    /// Adds a tunnel to `session` by hand, as a bound listener would, and
    /// returns the receiver its listener reads connections and errors from.
    async fn add_tunnel(session: &Session) -> mpsc::Receiver<Result<ConnInner, AcceptError>> {
        let (tx, rx) = channel(1);
        let tunnel = BoundTunnel {
            proto: "tcp".into(),
            opts: None,
            extra: Default::default(),
            labels: Default::default(),
            forwards_to: String::new(),
            forwards_proto: String::new(),
            verify_upstream_tls: false,
            tx,
        };
        session
            .inner
            .load()
            .tunnels
            .write()
            .await
            .insert("fake-tunnel".into(), tunnel);
        rx
    }

    /// The reconnect error a tunnel's listener receives within a day, on
    /// tokio's paused clock. Waiting no longer keeps a session that never
    /// stops reconnecting from hanging the test.
    async fn tunnel_error(
        tunnel: &mut mpsc::Receiver<Result<ConnInner, AcceptError>>,
    ) -> Arc<ConnectError> {
        let next = tokio::time::timeout(Duration::from_secs(24 * 60 * 60), tunnel.recv())
            .await
            .expect("the tunnel should receive an error within a day");
        match next {
            Some(Err(AcceptError::Reconnect(err))) => err,
            Some(Err(other)) => panic!("the tunnel should receive a reconnect error: {other:?}"),
            Some(Ok(_)) => panic!("the tunnel should receive an error, not a connection"),
            None => panic!("the tunnel closed without an error"),
        }
    }

    /// The first reconnect attempt starts the moment the connection drops,
    /// with the disconnect as its cause; each later attempt's cause is the
    /// previous attempt's error.
    #[tokio::test(start_paused = true)]
    async fn reconnect_starts_at_once_and_passes_on_each_cause() {
        let (_session, mut attempts) = connect_then_fail_reconnects(&mut Session::builder()).await;
        let dropped = Instant::now();

        let first = next_attempt(&mut attempts).await;
        let second = next_attempt(&mut attempts).await;

        assert_eq!(first.at, dropped);
        assert!(
            matches!(first.cause, Some(AcceptError::Transport(_))),
            "the first attempt's cause should be the dropped connection: {:?}",
            first.cause,
        );
        match second.cause {
            Some(AcceptError::Reconnect(err)) => assert!(
                matches!(&*err, ConnectError::Tcp(e) if e.kind() == io::ErrorKind::ConnectionRefused),
                "the second attempt's cause should be the first attempt's error: {err:?}",
            ),
            other => panic!("the second attempt's cause should be a reconnect error: {other:?}"),
        }
    }

    /// A connector returning Canceled stops reconnecting, and every tunnel
    /// receives that error.
    #[tokio::test(start_paused = true)]
    async fn canceled_connect_stops_reconnecting_and_tells_the_tunnels() {
        let (session, mut attempts) = connect_scripted(&mut Session::builder(), |k| match k {
            0 => Answer::Serve,
            3 => Answer::Cancel,
            _ => Answer::Refuse,
        })
        .await;
        let mut tunnel = add_tunnel(&session).await;

        for _ in 0..3 {
            next_attempt(&mut attempts).await;
        }
        assert_no_attempt_for_a_day(&mut attempts).await;

        let err = tunnel_error(&mut tunnel).await;
        assert!(
            matches!(*err, ConnectError::Canceled),
            "the tunnel should receive the cancel: {err:?}",
        );
    }

    /// Closing the session while it waits to reconnect stops reconnecting
    /// without another connect, and every tunnel receives Canceled.
    #[tokio::test(start_paused = true)]
    async fn closing_during_a_wait_stops_reconnecting() {
        let (mut session, mut attempts) =
            connect_then_fail_reconnects(&mut Session::builder()).await;
        let mut tunnel = add_tunnel(&session).await;
        next_attempt(&mut attempts).await;

        let _ = session.close().await;

        assert_no_attempt_for_a_day(&mut attempts).await;
        let err = tunnel_error(&mut tunnel).await;
        assert!(
            matches!(*err, ConnectError::Canceled),
            "the tunnel should receive Canceled: {err:?}",
        );
    }

    /// With jitter off, the default waits are exactly 50 ms before the
    /// second attempt, doubling each time up to 30 s.
    #[tokio::test(start_paused = true)]
    async fn reconnect_waits_without_jitter_double_from_50ms_to_30s() {
        let mut builder = Session::builder();
        builder.reconnect.jitter = false;
        let (_session, mut attempts) = connect_then_fail_reconnects(&mut builder).await;

        let gaps = reconnect_gaps(&mut attempts, 12).await;

        let expected = [
            50, 100, 200, 400, 800, 1_600, 3_200, 6_400, 12_800, 25_600, 30_000, 30_000,
        ]
        .map(Duration::from_millis);
        assert_eq!(gaps, expected);
    }

    /// A session whose connection drops after it starts, and whose every
    /// reconnect attempt then fails, waits at most 50 ms before its second
    /// attempt, at most twice as long before each one after that, and never
    /// more than 30 s, at jittered intervals.
    ///
    /// The clock is tokio's paused clock, which jumps to each timer, so an
    /// uncapped backoff's hours-long waits take no real time.
    #[tokio::test(start_paused = true)]
    async fn reconnect_attempts_double_up_to_30s_and_are_jittered() {
        let (_session, mut attempt_rx) =
            connect_then_fail_reconnects(&mut Session::builder()).await;

        // Sixteen attempts, so fifteen gaps: ten while the delay doubles from
        // 50 ms to 25.6 s, then five at the cap. Each gap is checked as it
        // arrives, so an uncapped backoff fails before its waits grow large
        // enough to overflow the clock.
        let mut last = next_attempt(&mut attempt_rx).await.at;
        let mut gaps = Vec::new();
        for k in 0..15 {
            let attempt = next_attempt(&mut attempt_rx).await.at;
            let gap = attempt - last;
            last = attempt;
            gaps.push(gap);
            let bound = Duration::from_millis(50 << k).min(Duration::from_secs(30));
            assert!(
                gap <= bound,
                "reconnect wait {k} is longer than {bound:?}; gaps: {gaps:?}",
            );
        }
        let capped = &gaps[10..];
        assert!(
            capped.windows(2).any(|w| w[0] != w[1]),
            "reconnect delays at the cap are all equal, so they are not jittered; gaps: {gaps:?}",
        );
    }

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
