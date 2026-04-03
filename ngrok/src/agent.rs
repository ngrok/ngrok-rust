use std::{
    env,
    sync::Arc,
    time::Duration,
};

use bytes::Bytes;
use futures_rustls::rustls;
use tokio::sync::Mutex;
use url::Url;

use crate::{
    endpoint::{
        EndpointForwarder,
    },
    endpoint_builder::{
        EndpointForwardBuilder,
        EndpointListenBuilder,
        EndpointOptions,
    },
    internals::raw_session::RpcError,
    session::{
        Connector,
        Session,
        SessionBuilder,
    },
    upstream::Upstream,
};

/// A long-lived ngrok agent identity/configuration.
///
/// The `Agent` manages connections to the ngrok service, automatically handling
/// reconnection. Use [`Agent::builder()`] to create a new agent, or the
/// module-level [`default_agent()`](crate::default_agent) for the simplest
/// usage.
///
/// # Examples
///
/// ```no_run
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let agent = ngrok::Agent::builder()
///     .authtoken("your-token")
///     .build()?;
///
/// let listener = agent.listen()
///     .url("https://app.ngrok.app")
///     .start()
///     .await?;
/// # Ok(())
/// # }
/// ```
#[derive(Clone)]
pub struct Agent {
    inner: Arc<AgentInner>,
}

struct AgentInner {
    config: Mutex<AgentConfig>,
    session: Mutex<Option<Session>>,
}

#[derive(Clone)]
struct AgentConfig {
    authtoken: Option<String>,
    metadata: Option<String>,
    description: Option<String>,
    server_host: Option<String>,
    ca_cert: Option<Bytes>,
    tls_config: Option<rustls::ClientConfig>,
    connector: Option<Arc<dyn Connector>>,
    proxy_url: Option<String>,
    heartbeat_interval: Option<Duration>,
    heartbeat_tolerance: Option<Duration>,
    client_info: Vec<(String, String, Option<String>)>,
    auto_connect: bool,
}

impl Default for AgentConfig {
    fn default() -> Self {
        AgentConfig {
            authtoken: None,
            metadata: None,
            description: None,
            server_host: None,
            ca_cert: None,
            tls_config: None,
            connector: None,
            proxy_url: None,
            heartbeat_interval: None,
            heartbeat_tolerance: None,
            client_info: Vec::new(),
            auto_connect: true,
        }
    }
}

/// Builder for configuring an [`Agent`].
///
/// Create a builder with [`Agent::builder()`], configure it with the fluent
/// API, then call [`build()`](AgentBuilder::build) to produce an `Agent`.
///
/// # Examples
///
/// ```no_run
/// # fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let agent = ngrok::Agent::builder()
///     .authtoken("your-token")
///     .metadata("my-agent")
///     .build()?;
/// # Ok(())
/// # }
/// ```
pub struct AgentBuilder {
    config: AgentConfig,
}

/// Information about a specific active connection to the ngrok service.
pub struct AgentSession {
    agent: Agent,
}

impl AgentSession {
    /// Returns a reference to the agent that owns this session.
    pub fn agent(&self) -> &Agent {
        &self.agent
    }
}

impl Agent {
    /// Create a new [`AgentBuilder`] to configure an agent.
    pub fn builder() -> AgentBuilder {
        AgentBuilder {
            config: AgentConfig::default(),
        }
    }

    /// Connect the agent to the ngrok service.
    ///
    /// If the agent is already connected, this is a no-op.
    /// If `auto_connect` is true (the default), this is called automatically
    /// on the first [`listen`](Self::listen) or [`forward`](Self::forward) call.
    pub async fn connect(&self) -> Result<(), RpcError> {
        let mut session_guard = self.inner.session.lock().await;
        if session_guard.is_some() {
            return Ok(());
        }
        let config = self.inner.config.lock().await.clone();
        let session = build_session(&config).await?;
        *session_guard = Some(session);
        Ok(())
    }

    /// Disconnect the agent from the ngrok service.
    pub async fn disconnect(&self) -> Result<(), RpcError> {
        let mut session_guard = self.inner.session.lock().await;
        if let Some(mut session) = session_guard.take() {
            session.close().await?;
        }
        Ok(())
    }

    /// Returns session information if connected.
    pub fn session(&self) -> Option<AgentSession> {
        // We can't check without blocking, so return a session wrapper
        Some(AgentSession {
            agent: self.clone(),
        })
    }

    /// Start building an endpoint listener.
    ///
    /// Returns an [`EndpointListenBuilder`] that can be configured with
    /// URL, traffic policy, metadata, etc. and then started with `.start()`.
    pub fn listen(&self) -> EndpointListenBuilder {
        EndpointListenBuilder {
            agent: self.clone(),
            opts: EndpointOptions::default(),
        }
    }

    /// Start building an endpoint forwarder.
    ///
    /// Returns an [`EndpointForwardBuilder`] that can be configured and
    /// then started with `.start()`.
    pub fn forward(&self, upstream: Upstream) -> EndpointForwardBuilder {
        EndpointForwardBuilder {
            agent: self.clone(),
            upstream,
            opts: EndpointOptions::default(),
        }
    }

    /// Convenience method to forward to an address with endpoint options.
    pub async fn forward_to(
        &self,
        addr: &str,
        opts: EndpointOptions,
    ) -> Result<EndpointForwarder, RpcError> {
        EndpointForwardBuilder {
            agent: self.clone(),
            upstream: Upstream::new(addr),
            opts,
        }
        .start()
        .await
    }

    /// Ensure the agent is connected, connecting if necessary.
    pub(crate) async fn ensure_connected(&self) -> Result<Session, RpcError> {
        {
            let session_guard = self.inner.session.lock().await;
            if let Some(ref session) = *session_guard {
                return Ok(session.clone());
            }
        }

        // Need to connect
        let config = self.inner.config.lock().await.clone();
        if !config.auto_connect {
            return Err(RpcError::Response(crate::internals::proto::ErrResp {
                msg: "agent not connected and auto_connect is disabled".into(),
                error_code: None,
            }));
        }

        let session = build_session(&config).await?;
        let mut session_guard = self.inner.session.lock().await;
        // Double-check in case another task connected while we were building
        if session_guard.is_none() {
            *session_guard = Some(session.clone());
        }
        Ok(session_guard.as_ref().unwrap().clone())
    }
}

impl AgentBuilder {
    /// Set the authtoken for authenticating with the ngrok service.
    ///
    /// You can [find your existing authtoken](https://dashboard.ngrok.com/get-started/your-authtoken)
    /// or [create a new one](https://dashboard.ngrok.com/tunnels/authtokens)
    /// in the ngrok dashboard.
    pub fn authtoken(&mut self, token: impl Into<String>) -> &mut Self {
        self.config.authtoken = Some(token.into());
        self
    }

    /// Use the `NGROK_AUTHTOKEN` environment variable as the authtoken.
    pub fn authtoken_from_env(&mut self) -> &mut Self {
        self.config.authtoken = env::var("NGROK_AUTHTOKEN").ok();
        self
    }

    /// Set the URL to connect to the ngrok service (custom agent ingress).
    pub fn connect_url(&mut self, url: impl Into<String>) -> &mut Self {
        self.config.server_host = Some(url.into());
        self
    }

    /// Set the CA certificate(s) in PEM format for validating the ngrok
    /// service TLS connection.
    pub fn connect_cas(&mut self, bytes: impl Into<Vec<u8>>) -> &mut Self {
        self.config.ca_cert = Some(Bytes::from(bytes.into()));
        self
    }

    /// Set opaque metadata for this agent session.
    pub fn metadata(&mut self, meta: impl Into<String>) -> &mut Self {
        self.config.metadata = Some(meta.into());
        self
    }

    /// Set a human-readable description for this agent.
    pub fn description(&mut self, desc: impl Into<String>) -> &mut Self {
        self.config.description = Some(desc.into());
        self
    }

    /// Set the TLS client configuration used to connect to the ngrok service.
    pub fn tls_config(&mut self, config: rustls::ClientConfig) -> &mut Self {
        self.config.tls_config = Some(config);
        self
    }

    /// Set a custom connector for establishing connections to the ngrok
    /// service.
    pub fn connector(&mut self, connect: impl Connector) -> &mut Self {
        self.config.connector = Some(Arc::new(connect));
        self
    }

    /// Set a proxy URL for connecting to the ngrok service.
    pub fn proxy_url(&mut self, url: impl Into<String>) -> &mut Self {
        self.config.proxy_url = Some(url.into());
        self
    }

    /// Set the heartbeat interval.
    pub fn heartbeat_interval(&mut self, interval: Duration) -> &mut Self {
        self.config.heartbeat_interval = Some(interval);
        self
    }

    /// Set the heartbeat tolerance (how long to wait for a response).
    pub fn heartbeat_tolerance(&mut self, tolerance: Duration) -> &mut Self {
        self.config.heartbeat_tolerance = Some(tolerance);
        self
    }

    /// Add client type and version information.
    pub fn client_info(
        &mut self,
        client_type: impl Into<String>,
        version: impl Into<String>,
        comments: Option<impl Into<String>>,
    ) -> &mut Self {
        self.config
            .client_info
            .push((client_type.into(), version.into(), comments.map(|c| c.into())));
        self
    }

    /// Set whether the agent should auto-connect on the first
    /// `listen()`/`forward()` call.
    ///
    /// Defaults to `true`.
    pub fn auto_connect(&mut self, auto: bool) -> &mut Self {
        self.config.auto_connect = auto;
        self
    }

    /// Build the [`Agent`] without connecting to the ngrok service.
    ///
    /// The agent will connect lazily on the first `listen()` or `forward()`
    /// call if `auto_connect` is true (the default), or you can call
    /// [`Agent::connect()`] explicitly.
    pub fn build(&self) -> Result<Agent, RpcError> {
        Ok(Agent {
            inner: Arc::new(AgentInner {
                config: Mutex::new(self.config.clone()),
                session: Mutex::new(None),
            }),
        })
    }
}

/// Build a Session from agent configuration.
async fn build_session(config: &AgentConfig) -> Result<Session, RpcError> {
    let mut builder = SessionBuilder::default();

    if let Some(ref token) = config.authtoken {
        builder.authtoken(token);
    }
    if let Some(ref meta) = config.metadata {
        builder.metadata(meta);
    }
    if let Some(ref host) = config.server_host {
        builder
            .server_addr(host)
            .map_err(|e| RpcError::Response(crate::internals::proto::ErrResp {
                msg: format!("invalid server address: {}", e),
                error_code: None,
            }))?;
    }
    if let Some(ref ca) = config.ca_cert {
        builder.ca_cert(ca.clone());
    }
    if let Some(ref tls) = config.tls_config {
        builder.tls_config(tls.clone());
    }
    if let Some(ref connector) = config.connector {
        // We need to clone the Arc and pass it through; the connector trait
        // requires Connector impl, so we wrap Arc<dyn Connector>
        builder.connector(ArcConnector(connector.clone()));
    }
    if let Some(ref proxy_url) = config.proxy_url {
        let url: Url = proxy_url.parse().map_err(|e| {
            RpcError::Response(crate::internals::proto::ErrResp {
                msg: format!("invalid proxy URL '{}': {}", proxy_url, e),
                error_code: None,
            })
        })?;
        builder.proxy_url(url).map_err(|e| {
            RpcError::Response(crate::internals::proto::ErrResp {
                msg: format!("unsupported proxy URL: {}", e),
                error_code: None,
            })
        })?;
    }
    if let Some(interval) = config.heartbeat_interval {
        builder.heartbeat_interval(interval).map_err(|e| {
            RpcError::Response(crate::internals::proto::ErrResp {
                msg: format!("invalid heartbeat interval: {}", e),
                error_code: None,
            })
        })?;
    }
    if let Some(tolerance) = config.heartbeat_tolerance {
        builder.heartbeat_tolerance(tolerance).map_err(|e| {
            RpcError::Response(crate::internals::proto::ErrResp {
                msg: format!("invalid heartbeat tolerance: {}", e),
                error_code: None,
            })
        })?;
    }
    for (client_type, version, comments) in &config.client_info {
        builder.client_info(client_type, version, comments.clone());
    }

    builder
        .connect()
        .await
        .map_err(|e| RpcError::Response(crate::internals::proto::ErrResp {
            msg: format!("failed to connect: {}", e),
            error_code: None,
        }))
}

/// Wrapper to make Arc<dyn Connector> implement Connector.
struct ArcConnector(Arc<dyn Connector>);

#[async_trait::async_trait]
impl Connector for ArcConnector {
    async fn connect(
        &self,
        host: String,
        port: u16,
        tls_config: Arc<rustls::ClientConfig>,
        err: Option<crate::tunnel::AcceptError>,
    ) -> Result<Box<dyn crate::session::IoStream>, crate::session::ConnectError> {
        self.0.connect(host, port, tls_config, err).await
    }
}
