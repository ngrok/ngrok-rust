use std::collections::HashMap;

use prost::bytes::{
    self,
    Bytes,
};

use super::common::ProxyProto;
use crate::{
    common::{
        self,
        private,
        CommonOpts,
        FORWARDS_TO,
    },
    headers::Headers,
    internals::proto::{
        self,
        BindExtra,
        BindOpts,
    },
    mw::{
        middleware_configuration::{
            BasicAuth,
            BasicAuthCredential,
            CircuitBreaker,
            Compression,
            WebsocketTcpConverter,
        },
        HttpMiddleware,
    },
    oauth::{
        self,
        OauthOptionsTrait,
    },
    oidc::{
        self,
        OidcOptionsTrait,
    },
    webhook_verification::{
        self,
        WebhookVerification,
    },
};

#[derive(Clone, Default, Eq, PartialEq)]
pub enum Scheme {
    HTTP,
    #[default]
    HTTPS,
}

/// The options for a HTTP edge.
#[derive(Default)]
pub struct HTTPEndpoint {
    pub(crate) common_opts: CommonOpts,
    pub(crate) scheme: Scheme,
    pub(crate) domain: Option<String>,
    pub(crate) mutual_tlsca: Vec<bytes::Bytes>,
    pub(crate) compression: bool,
    pub(crate) websocket_tcp_conversion: bool,
    pub(crate) circuit_breaker: f64,
    pub(crate) request_headers: Headers,
    pub(crate) response_headers: Headers,
    pub(crate) basic_auth: Vec<(String, String)>,
    pub(crate) oauth: Option<Box<dyn OauthOptionsTrait>>,
    pub(crate) oidc: Option<Box<dyn OidcOptionsTrait>>,
    pub(crate) webhook_verification: Option<WebhookVerification>,
}

impl private::TunnelConfigPrivate for HTTPEndpoint {
    fn forwards_to(&self) -> String {
        self.common_opts
            .forwards_to
            .clone()
            .unwrap_or(FORWARDS_TO.into())
    }
    fn extra(&self) -> BindExtra {
        BindExtra {
            token: Default::default(),
            ip_policy_ref: Default::default(),
            metadata: self.common_opts.metadata.clone().unwrap_or_default(),
        }
    }
    fn proto(&self) -> String {
        if self.scheme == Scheme::HTTP {
            return "http".into();
        }
        "https".into()
    }
    fn opts(&self) -> Option<BindOpts> {
        // fill out all the options, translating to proto here
        let mut http_endpoint = proto::HttpEndpoint::default();

        if let Some(domain) = self.domain.as_ref() {
            // note: hostname and subdomain are going away in favor of just domain
            http_endpoint.hostname = domain.clone();
        }
        http_endpoint.proxy_proto = self.common_opts.proxy_proto;

        let basic_auth: Option<BasicAuth> = (!self.basic_auth.is_empty()).then_some(BasicAuth {
            credentials: self
                .basic_auth
                .iter()
                .map(|b| BasicAuthCredential {
                    username: b.0.clone(),
                    cleartext_password: b.1.clone(),
                    hashed_password: Vec::new(), // unused in this context
                })
                .collect(),
        });

        http_endpoint.middleware = HttpMiddleware {
            compression: self.compression.then_some(Compression {}),
            circuit_breaker: (self.circuit_breaker != 0f64).then_some(CircuitBreaker {
                error_threshold: self.circuit_breaker,
            }),
            ip_restriction: self.common_opts.ip_restriction(),
            basic_auth,
            oauth: oauth::to_proto_config(&self.oauth),
            oidc: oidc::to_proto_config(&self.oidc),
            webhook_verification: webhook_verification::to_proto_config(&self.webhook_verification),
            mutual_tls: common::mutual_tls(&self.mutual_tlsca),
            request_headers: self.request_headers.to_proto_config(),
            response_headers: self.response_headers.to_proto_config(),
            websocket_tcp_converter: self
                .websocket_tcp_conversion
                .then_some(WebsocketTcpConverter {}),
        };

        Some(BindOpts::Http(http_endpoint))
    }
    fn labels(&self) -> HashMap<String, String> {
        HashMap::new()
    }
}

impl HTTPEndpoint {
    /// Restriction placed on the origin of incoming connections to the edge to only allow these CIDR ranges.
    /// Call multiple times to add additional CIDR ranges.
    pub fn with_allow_cidr_string(&mut self, cidr: impl Into<String>) -> &mut Self {
        self.common_opts.cidr_restrictions.allow(cidr);
        self
    }
    /// Restriction placed on the origin of incoming connections to the edge to deny these CIDR ranges.
    /// Call multiple times to add additional CIDR ranges.
    pub fn with_deny_cidr_string(&mut self, cidr: impl Into<String>) -> &mut Self {
        self.common_opts.cidr_restrictions.deny(cidr);
        self
    }
    /// The version of PROXY protocol to use with this tunnel, None if not using.
    pub fn with_proxy_proto(&mut self, proxy_proto: ProxyProto) -> &mut Self {
        self.common_opts.proxy_proto = proxy_proto;
        self
    }
    /// Tunnel-specific opaque metadata. Viewable via the API.
    pub fn with_metadata(&mut self, metadata: impl Into<String>) -> &mut Self {
        self.common_opts.metadata = Some(metadata.into());
        self
    }
    /// Tunnel backend metadata. Viewable via the dashboard and API, but has no
    /// bearing on tunnel behavior.
    pub fn with_forwards_to(&mut self, forwards_to: impl Into<String>) -> &mut Self {
        self.common_opts.forwards_to = Some(forwards_to.into());
        self
    }
    /// The scheme that this edge should use.
    /// Defaults to [HTTPS].
    pub fn with_scheme(&mut self, scheme: Scheme) -> &mut Self {
        self.scheme = scheme;
        self
    }
    /// The domain to request for this edge
    pub fn with_domain(&mut self, domain: impl Into<String>) -> &mut Self {
        self.domain = Some(domain.into());
        self
    }
    /// Certificates to use for client authentication at the ngrok edge.
    pub fn with_mutual_tlsca(&mut self, mutual_tlsca: Bytes) -> &mut Self {
        self.mutual_tlsca.push(mutual_tlsca);
        self
    }
    /// Enable gzip compression for HTTP responses.
    pub fn with_compression(&mut self) -> &mut Self {
        self.compression = true;
        self
    }
    /// Convert incoming websocket connections to TCP-like streams.
    pub fn with_websocket_tcp_conversion(&mut self) -> &mut Self {
        self.websocket_tcp_conversion = true;
        self
    }
    /// Reject requests when 5XX responses exceed this ratio.
    /// Disabled when 0.
    pub fn with_circuit_breaker(&mut self, circuit_breaker: f64) -> &mut Self {
        self.circuit_breaker = circuit_breaker;
        self
    }

    /// with_request_header adds a header to all requests to this edge.
    pub fn with_request_header(
        &mut self,
        name: impl Into<String>,
        value: impl Into<String>,
    ) -> &mut Self {
        self.request_headers.add(name, value);
        self
    }
    /// with_response_header adds a header to all responses coming from this edge.
    pub fn with_response_header(
        &mut self,
        name: impl Into<String>,
        value: impl Into<String>,
    ) -> &mut Self {
        self.response_headers.add(name, value);
        self
    }
    /// with_remove_request_header removes a header from requests to this edge.
    pub fn with_remove_request_header(&mut self, name: impl Into<String>) -> &mut Self {
        self.request_headers.remove(name);
        self
    }
    /// with_remove_response_header removes a header from responses from this edge.
    pub fn with_remove_response_header(&mut self, name: impl Into<String>) -> &mut Self {
        self.response_headers.remove(name);
        self
    }

    /// Credentials for basic authentication.
    /// If not called, basic authentication is disabled.
    pub fn with_basic_auth(
        &mut self,
        username: impl Into<String>,
        password: impl Into<String>,
    ) -> &mut Self {
        self.basic_auth.push((username.into(), password.into()));
        self
    }

    /// OAuth configuration.
    /// If not called, OAuth is disabled.
    pub fn with_oauth<O>(&mut self, oauth: O) -> &mut Self
    where
        O: OauthOptionsTrait + 'static,
    {
        self.oauth = Some(Box::new(oauth));
        self
    }

    /// OIDC configuration.
    /// If not called, OIDC is disabled.
    pub fn with_oidc<O>(&mut self, oidc: O) -> &mut Self
    where
        O: OidcOptionsTrait + 'static,
    {
        self.oidc = Some(Box::new(oidc));
        self
    }

    /// WebhookVerification configuration.
    /// If not called, WebhookVerification is disabled.
    pub fn with_webhook_verification(
        &mut self,
        provider: impl Into<String>,
        secret: impl Into<String>,
    ) -> &mut Self {
        self.webhook_verification = Some(WebhookVerification {
            provider: provider.into(),
            secret: secret.into(),
        });
        self
    }
}
