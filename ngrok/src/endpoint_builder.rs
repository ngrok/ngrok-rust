use url::Url;

use crate::{
    agent::Agent,
    config::TunnelConfig,
    endpoint::{
        EndpointForwarder,
        EndpointListener,
    },
    internals::{
        proto::BindExtra,
        raw_session::RpcError,
    },
    tunnel::TunnelInner,
    upstream::Upstream,
};

/// Options for configuring an ngrok endpoint.
///
/// Replaces the protocol-specific builders (`HttpTunnelBuilder`,
/// `TcpTunnelBuilder`, `TlsTunnelBuilder`) with a unified configuration.
/// The protocol is inferred from the URL scheme.
#[derive(Clone, Debug, Default)]
pub struct EndpointOptions {
    /// The URL for this endpoint. The scheme determines the protocol:
    /// - `https://` or `http://` → HTTP endpoint
    /// - `tcp://` → TCP endpoint
    /// - `tls://` → TLS endpoint
    pub(crate) url: Option<String>,
    /// Traffic policy as a YAML or JSON string.
    pub(crate) traffic_policy: Option<String>,
    /// Opaque metadata string.
    pub(crate) metadata: Option<String>,
    /// Human-readable description.
    pub(crate) description: Option<String>,
    /// Binding configuration (e.g., "public", "internal").
    pub(crate) bindings: Vec<String>,
    /// Whether endpoint pooling is enabled.
    pub(crate) pooling_enabled: Option<bool>,
}

/// Builder returned by [`Agent::listen`].
///
/// Use this to configure and start an [`EndpointListener`] that accepts
/// incoming connections from the ngrok service.
///
/// # Examples
///
/// ```no_run
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// // Listen on a random HTTPS endpoint
/// let listener = ngrok::listen().start().await?;
///
/// // Listen on a specific domain
/// let listener = ngrok::listen()
///     .url("https://app.ngrok.app")
///     .start()
///     .await?;
///
/// // Listen on a TCP endpoint
/// let listener = ngrok::listen()
///     .url("tcp://1.tcp.ngrok.io:12345")
///     .start()
///     .await?;
/// # Ok(())
/// # }
/// ```
pub struct EndpointListenBuilder {
    pub(crate) agent: Agent,
    pub(crate) opts: EndpointOptions,
}

/// Builder returned by [`Agent::forward`].
///
/// Use this to configure and start an [`EndpointForwarder`] that automatically
/// forwards traffic from the ngrok service to an upstream.
///
/// # Examples
///
/// ```no_run
/// use ngrok::Upstream;
///
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let fwd = ngrok::forward(Upstream::new("localhost:8080"))
///     .url("https://app.ngrok.app")
///     .start()
///     .await?;
/// # Ok(())
/// # }
/// ```
pub struct EndpointForwardBuilder {
    pub(crate) agent: Agent,
    pub(crate) upstream: Upstream,
    pub(crate) opts: EndpointOptions,
}

macro_rules! endpoint_builder_methods {
    ($builder:ident) => {
        impl $builder {
            /// Set the URL for this endpoint.
            ///
            /// The scheme determines the protocol:
            /// - `https://domain` or `http://domain` → HTTP endpoint
            /// - `tcp://host:port` → TCP endpoint
            /// - `tls://domain` → TLS endpoint
            ///
            /// If no URL is set, a random HTTPS endpoint is assigned.
            pub fn url(mut self, url: impl Into<String>) -> Self {
                self.opts.url = Some(url.into());
                self
            }

            /// Set the traffic policy for this endpoint.
            ///
            /// The policy should be a YAML or JSON string defining rules for
            /// incoming and outgoing traffic. This replaces all per-protocol
            /// middleware configuration (OAuth, compression, headers, etc.).
            pub fn traffic_policy(mut self, policy: impl Into<String>) -> Self {
                self.opts.traffic_policy = Some(policy.into());
                self
            }

            /// Set opaque metadata for this endpoint.
            ///
            /// Metadata is viewable via the ngrok API and dashboard.
            pub fn metadata(mut self, meta: impl Into<String>) -> Self {
                self.opts.metadata = Some(meta.into());
                self
            }

            /// Set a human-readable description for this endpoint.
            pub fn description(mut self, desc: impl Into<String>) -> Self {
                self.opts.description = Some(desc.into());
                self
            }

            /// Set binding configuration for this endpoint.
            pub fn bindings(
                mut self,
                bindings: impl IntoIterator<Item = impl Into<String>>,
            ) -> Self {
                self.opts.bindings = bindings.into_iter().map(|b| b.into()).collect();
                self
            }

            /// Enable or disable endpoint pooling.
            ///
            /// When enabled, multiple endpoints with the same host/port/binding
            /// can share traffic.
            pub fn pooling_enabled(mut self, enabled: bool) -> Self {
                self.opts.pooling_enabled = Some(enabled);
                self
            }
        }
    };
}

endpoint_builder_methods!(EndpointListenBuilder);
endpoint_builder_methods!(EndpointForwardBuilder);

/// Internal endpoint options used for dispatching to the correct protocol.
#[derive(Clone, Default)]
pub(crate) struct InternalEndpointOpts {
    pub(crate) traffic_policy: Option<String>,
    pub(crate) metadata: Option<String>,
    pub(crate) bindings: Vec<String>,
    pub(crate) pooling_enabled: Option<bool>,
    pub(crate) forwards_to: Option<String>,
    pub(crate) forwards_proto: Option<String>,
    pub(crate) verify_upstream_tls: bool,
}

impl InternalEndpointOpts {
    fn from_endpoint_options(opts: &EndpointOptions) -> Self {
        InternalEndpointOpts {
            traffic_policy: opts.traffic_policy.clone(),
            metadata: opts.metadata.clone(),
            bindings: opts.bindings.clone(),
            pooling_enabled: opts.pooling_enabled,
            forwards_to: None,
            forwards_proto: None,
            verify_upstream_tls: true,
        }
    }
}

fn make_rpc_error(msg: impl Into<String>) -> RpcError {
    RpcError::Response(crate::internals::proto::ErrResp {
        msg: msg.into(),
        error_code: None,
    })
}

/// Parse a URL string into (scheme, host_port).
fn parse_endpoint_url(url: &str) -> Result<(String, Option<String>), RpcError> {
    // Handle simple cases like "tcp://host:port"
    let parsed = url::Url::parse(url)
        .map_err(|e| make_rpc_error(format!("invalid endpoint URL '{}': {}", url, e)))?;

    let scheme = parsed.scheme().to_lowercase();
    let host_port = match scheme.as_str() {
        "tcp" => {
            // For TCP we need host:port
            let host = parsed.host_str().unwrap_or_default();
            let port = parsed.port();
            if host.is_empty() && port.is_none() {
                None
            } else if let Some(p) = port {
                Some(format!("{}:{}", host, p))
            } else {
                Some(host.to_string())
            }
        }
        "https" | "http" | "tls" => {
            // For HTTP/HTTPS/TLS we want the domain
            parsed.host_str().map(|h| h.to_string())
        }
        _ => {
            return Err(make_rpc_error(format!(
                "unsupported endpoint URL scheme '{}'. Supported: https, http, tcp, tls",
                scheme
            )));
        }
    };

    Ok((scheme, host_port))
}

// ===== HTTP internal config =====

#[derive(Clone)]
struct HttpInternalConfig {
    domain: Option<String>,
    scheme: String,
    inner: InternalEndpointOpts,
}

impl TunnelConfig for HttpInternalConfig {
    fn forwards_to(&self) -> String {
        self.inner
            .forwards_to
            .clone()
            .unwrap_or_else(|| crate::config::default_forwards_to().to_string())
    }
    fn forwards_proto(&self) -> String {
        self.inner.forwards_proto.clone().unwrap_or_default()
    }
    fn verify_upstream_tls(&self) -> bool {
        self.inner.verify_upstream_tls
    }
    fn extra(&self) -> BindExtra {
        BindExtra {
            token: Default::default(),
            ip_policy_ref: Default::default(),
            metadata: self.inner.metadata.clone().unwrap_or_default(),
            bindings: self.inner.bindings.clone(),
            pooling_enabled: self.inner.pooling_enabled.unwrap_or(false),
        }
    }
    fn proto(&self) -> String {
        self.scheme.clone()
    }
    fn opts(&self) -> Option<crate::internals::proto::BindOpts> {
        use crate::internals::proto::{
            BindOpts,
            HttpEndpoint,
        };
        let mut http = HttpEndpoint::default();
        if let Some(ref domain) = self.domain {
            http.domain = domain.clone();
        }
        if let Some(ref tp) = self.inner.traffic_policy {
            http.traffic_policy =
                Some(crate::internals::proto::PolicyWrapper::String(tp.clone()));
        }
        Some(BindOpts::Http(http))
    }
    fn labels(&self) -> std::collections::HashMap<String, String> {
        Default::default()
    }
}

// ===== TCP internal config =====

#[derive(Clone)]
struct TcpInternalConfig {
    remote_addr: Option<String>,
    inner: InternalEndpointOpts,
}

impl TunnelConfig for TcpInternalConfig {
    fn forwards_to(&self) -> String {
        self.inner
            .forwards_to
            .clone()
            .unwrap_or_else(|| crate::config::default_forwards_to().to_string())
    }
    fn forwards_proto(&self) -> String {
        String::new()
    }
    fn verify_upstream_tls(&self) -> bool {
        self.inner.verify_upstream_tls
    }
    fn extra(&self) -> BindExtra {
        BindExtra {
            token: Default::default(),
            ip_policy_ref: Default::default(),
            metadata: self.inner.metadata.clone().unwrap_or_default(),
            bindings: self.inner.bindings.clone(),
            pooling_enabled: self.inner.pooling_enabled.unwrap_or(false),
        }
    }
    fn proto(&self) -> String {
        "tcp".into()
    }
    fn opts(&self) -> Option<crate::internals::proto::BindOpts> {
        use crate::internals::proto::{
            BindOpts,
            TcpEndpoint,
        };
        let mut tcp = TcpEndpoint::default();
        if let Some(ref addr) = self.remote_addr {
            tcp.addr = addr.clone();
        }
        if let Some(ref tp) = self.inner.traffic_policy {
            tcp.traffic_policy =
                Some(crate::internals::proto::PolicyWrapper::String(tp.clone()));
        }
        Some(BindOpts::Tcp(tcp))
    }
    fn labels(&self) -> std::collections::HashMap<String, String> {
        Default::default()
    }
}

// ===== TLS internal config =====

#[derive(Clone)]
struct TlsInternalConfig {
    domain: Option<String>,
    inner: InternalEndpointOpts,
}

impl TunnelConfig for TlsInternalConfig {
    fn forwards_to(&self) -> String {
        self.inner
            .forwards_to
            .clone()
            .unwrap_or_else(|| crate::config::default_forwards_to().to_string())
    }
    fn forwards_proto(&self) -> String {
        String::new()
    }
    fn verify_upstream_tls(&self) -> bool {
        self.inner.verify_upstream_tls
    }
    fn extra(&self) -> BindExtra {
        BindExtra {
            token: Default::default(),
            ip_policy_ref: Default::default(),
            metadata: self.inner.metadata.clone().unwrap_or_default(),
            bindings: self.inner.bindings.clone(),
            pooling_enabled: self.inner.pooling_enabled.unwrap_or(false),
        }
    }
    fn proto(&self) -> String {
        "tls".into()
    }
    fn opts(&self) -> Option<crate::internals::proto::BindOpts> {
        use crate::internals::proto::{
            BindOpts,
            TlsEndpoint,
        };
        let mut tls = TlsEndpoint::default();
        if let Some(ref domain) = self.domain {
            tls.domain = domain.clone();
        }
        if let Some(ref tp) = self.inner.traffic_policy {
            tls.traffic_policy =
                Some(crate::internals::proto::PolicyWrapper::String(tp.clone()));
        }
        Some(BindOpts::Tls(tls))
    }
    fn labels(&self) -> std::collections::HashMap<String, String> {
        Default::default()
    }
}

impl EndpointListenBuilder {
    /// Start the endpoint and begin accepting connections.
    ///
    /// This is the terminal method that actually creates the endpoint on the
    /// ngrok service. The protocol is inferred from the URL scheme set with
    /// [`url`](Self::url).
    pub async fn start(self) -> Result<EndpointListener, RpcError> {
        let session = self.agent.ensure_connected().await?;
        let inner_opts = InternalEndpointOpts::from_endpoint_options(&self.opts);

        let tunnel = dispatch_listen(&session, &self.opts.url, inner_opts).await?;
        Ok(EndpointListener::from_tunnel(tunnel))
    }
}

impl EndpointForwardBuilder {
    /// Start the endpoint and begin forwarding traffic to the upstream.
    ///
    /// This is the terminal method that creates the endpoint and starts
    /// forwarding all incoming connections to the configured upstream.
    pub async fn start(self) -> Result<EndpointForwarder, RpcError> {
        let session = self.agent.ensure_connected().await?;
        let mut inner_opts = InternalEndpointOpts::from_endpoint_options(&self.opts);

        // Parse the upstream URL for forwarding
        let to_url: Url = self.upstream.addr.parse().unwrap_or_else(|_| {
            format!("http://{}", self.upstream.addr)
                .parse()
                .expect("invalid upstream address")
        });

        inner_opts.forwards_to = Some(to_url.to_string());
        if let Some(ref proto) = self.upstream.protocol {
            inner_opts.forwards_proto = Some(proto.clone());
        }
        if let Some(verify) = self.upstream.verify_upstream_tls {
            inner_opts.verify_upstream_tls = verify;
        }

        let tunnel = dispatch_listen(&session, &self.opts.url, inner_opts).await?;

        // Get a copy for info before moving the tunnel into the forwarder
        let info_tunnel = tunnel.make_info();
        let listener = EndpointListener::from_tunnel(tunnel);
        let info_listener = EndpointListener::from_tunnel(info_tunnel);

        let join = tokio::spawn(async move {
            crate::tunnel_ext::forward_tunnel(&mut EndpointListenerAsTunnel(listener), to_url)
                .await
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
        });

        Ok(EndpointForwarder {
            inner: info_listener,
            join,
        })
    }
}

/// Internal: a wrapper that makes EndpointListener look like a Tunnel for forwarding.
struct EndpointListenerAsTunnel(EndpointListener);

impl futures::Stream for EndpointListenerAsTunnel {
    type Item = Result<crate::conn::EndpointConn, crate::tunnel::AcceptError>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        use std::pin::Pin;

        Pin::new(&mut self.0.inner).poll_next(cx).map(|o| {
            o.map(|r| {
                r.map(|c| crate::conn::EndpointConn {
                    inner: c,
                })
            })
        })
    }
}

impl Unpin for EndpointListenerAsTunnel {}

impl crate::tunnel::TunnelInfo for EndpointListenerAsTunnel {
    fn id(&self) -> &str {
        self.0.inner.id()
    }
    fn forwards_to(&self) -> &str {
        self.0.inner.forwards_to()
    }
    fn metadata(&self) -> &str {
        self.0.inner.metadata()
    }
}

#[async_trait::async_trait]
impl crate::tunnel::TunnelCloser for EndpointListenerAsTunnel {
    async fn close(&mut self) -> Result<(), RpcError> {
        self.0.inner.close().await
    }
}

impl crate::tunnel::EndpointInfo for EndpointListenerAsTunnel {
    fn url(&self) -> &str {
        self.0.inner.url()
    }
    fn proto(&self) -> &str {
        self.0.inner.proto()
    }
}

impl crate::tunnel::Tunnel for EndpointListenerAsTunnel {
    type Conn = crate::conn::EndpointConn;
}

/// Dispatch to the correct protocol based on the URL scheme.
async fn dispatch_listen(
    session: &crate::session::Session,
    url: &Option<String>,
    inner_opts: InternalEndpointOpts,
) -> Result<TunnelInner, RpcError> {
    match url {
        None => {
            // Default: HTTPS endpoint with random domain
            let cfg = HttpInternalConfig {
                domain: None,
                scheme: "https".into(),
                inner: inner_opts,
            };
            session.start_tunnel(&cfg).await
        }
        Some(url_str) => {
            let (scheme, host_port) = parse_endpoint_url(url_str)?;
            match scheme.as_str() {
                "https" | "http" => {
                    let cfg = HttpInternalConfig {
                        domain: host_port,
                        scheme,
                        inner: inner_opts,
                    };
                    session.start_tunnel(&cfg).await
                }
                "tcp" => {
                    let cfg = TcpInternalConfig {
                        remote_addr: host_port,
                        inner: inner_opts,
                    };
                    session.start_tunnel(&cfg).await
                }
                "tls" => {
                    let cfg = TlsInternalConfig {
                        domain: host_port,
                        inner: inner_opts,
                    };
                    session.start_tunnel(&cfg).await
                }
                _ => Err(make_rpc_error(format!(
                    "unsupported endpoint URL scheme '{}'",
                    scheme
                ))),
            }
        }
    }
}
