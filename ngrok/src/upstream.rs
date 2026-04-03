use crate::config::ProxyProto;

/// Configuration for the upstream (backend) service that traffic is forwarded to.
///
/// An `Upstream` describes where ngrok should forward incoming traffic. Use it
/// with [`Agent::forward`](crate::Agent::forward) or the top-level
/// [`forward`](crate::forward) convenience function.
///
/// # Examples
///
/// ```no_run
/// use ngrok::Upstream;
///
/// // Simple HTTP backend
/// let upstream = Upstream::new("localhost:8080");
///
/// // HTTPS backend with TLS
/// let upstream = Upstream::new("https://localhost:8443");
///
/// // With custom PROXY protocol
/// let upstream = Upstream::new("localhost:8080")
///     .proxy_proto(ngrok::config::ProxyProto::V2);
/// ```
#[derive(Clone, Debug)]
pub struct Upstream {
    pub(crate) addr: String,
    pub(crate) protocol: Option<String>,
    pub(crate) proxy_proto: Option<ProxyProto>,
    pub(crate) verify_upstream_tls: Option<bool>,
}

impl Upstream {
    /// Create a new Upstream pointing to the given address.
    ///
    /// The address can be a host:port pair like `"localhost:8080"` or include
    /// a scheme like `"https://localhost:8443"`.
    pub fn new(addr: impl Into<String>) -> Self {
        Upstream {
            addr: addr.into(),
            protocol: None,
            proxy_proto: None,
            verify_upstream_tls: None,
        }
    }

    /// Set the application protocol (e.g., "http2") for the upstream.
    pub fn protocol(mut self, proto: impl Into<String>) -> Self {
        self.protocol = Some(proto.into());
        self
    }

    /// Set the PROXY protocol version to use when forwarding to the upstream.
    pub fn proxy_proto(mut self, version: ProxyProto) -> Self {
        self.proxy_proto = Some(version);
        self
    }

    /// Set whether to verify TLS certificates when connecting to the upstream.
    /// Defaults to `true`.
    pub fn verify_upstream_tls(mut self, verify: bool) -> Self {
        self.verify_upstream_tls = Some(verify);
        self
    }
}
