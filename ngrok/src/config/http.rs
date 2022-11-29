use std::collections::HashMap;

use crate::{
    common::{
        self,
        private,
        CommonOpts,
        ProxyProtocol,
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

#[derive(Clone, Eq, PartialEq)]
pub enum Scheme {
    HTTP,
    HTTPS,
}

/// The options for a HTTP edge.
pub struct HTTPEndpoint {
    /// Common tunnel configuration options.
    pub(crate) common_opts: CommonOpts,

    /// The scheme that this edge should use.
    /// Defaults to [HTTPS].
    pub(crate) scheme: Scheme,

    /// The domain to request for this edge
    pub(crate) domain: Option<String>,

    /// If non-nil, start a goroutine which runs this http server
    /// accepting connections from the http tunnel
    pub(crate) http_server: Option<String>, // todo

    /// Certificates to use for client authentication at the ngrok edge.
    pub(crate) mutual_tlsca: Vec<Vec<u8>>, // something more idiomatic here?
    /// Enable gzip compression for HTTP responses.
    pub(crate) compression: bool,
    /// Convert incoming websocket connections to TCP-like streams.
    pub(crate) websocket_tcp_conversion: bool,
    /// Reject requests when 5XX responses exceed this ratio.
    /// Disabled when 0.
    pub(crate) circuit_breaker: f64,

    // Headers to be added to or removed from all requests at the ngrok edge.
    pub(crate) request_headers: Headers,
    /// Headers to be added to or removed from all responses at the ngrok edge.
    pub(crate) response_headers: Headers,

    /// Credentials for basic authentication.
    /// If empty, basic authentication is disabled.
    pub(crate) basic_auth: Vec<(String, String)>,
    /// OAuth configuration.
    /// If nil, OAuth is disabled.
    pub(crate) oauth: Option<Box<dyn OauthOptionsTrait>>,
    /// OIDC configuration.
    /// If nil, OIDC is disabled.
    pub(crate) oidc: Option<Box<dyn OidcOptionsTrait>>,
    /// WebhookVerification configuration.
    /// If nil, WebhookVerification is disabled.
    pub(crate) webhook_verification: Option<WebhookVerification>,
}

impl Default for HTTPEndpoint {
    fn default() -> Self {
        HTTPEndpoint {
            common_opts: CommonOpts::default(),
            scheme: Scheme::HTTPS,
            domain: None,
            http_server: None,
            mutual_tlsca: Vec::new(),
            compression: false,
            websocket_tcp_conversion: false,
            circuit_breaker: 0f64,
            request_headers: Headers::default(),
            response_headers: Headers::default(),
            basic_auth: Vec::new(),
            oauth: None,
            oidc: None,
            webhook_verification: None,
        }
    }
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
            http_endpoint.hostname = domain.clone();
            // todo: ngrok-rs has "hostname" and "subdomain", ngrok-go has just "domain"?
        }
        http_endpoint.proxy_proto = self.common_opts.as_proxy_proto();

        let basic_auth: Option<BasicAuth> = match self.basic_auth.is_empty() {
            true => None,
            false => {
                Some(BasicAuth {
                    credentials: self
                        .basic_auth
                        .clone()
                        .into_iter()
                        .map(|b| BasicAuthCredential {
                            username: b.0,
                            cleartext_password: b.1,
                            hashed_password: Vec::new(), // appears unused
                        })
                        .collect(),
                })
            }
        };

        http_endpoint.middleware = HttpMiddleware {
            compression: match self.compression {
                true => Some(Compression {}),
                false => None,
            },
            circuit_breaker: match self.circuit_breaker != 0f64 {
                true => Some(CircuitBreaker {
                    error_threshold: self.circuit_breaker,
                }),
                false => None,
            },
            ip_restriction: self.common_opts.cidr_to_proto_config(),
            basic_auth,
            oauth: oauth::to_proto_config(&self.oauth),
            oidc: oidc::to_proto_config(&self.oidc),
            webhook_verification: webhook_verification::to_proto_config(&self.webhook_verification),
            mutual_tls: common::mutual_tls_to_proto_config(&self.mutual_tlsca),
            request_headers: self.request_headers.to_proto_config(),
            response_headers: self.response_headers.to_proto_config(),
            websocket_tcp_converter: match self.websocket_tcp_conversion {
                true => Some(WebsocketTcpConverter {}),
                false => None,
            },
        };

        Some(BindOpts::Http(http_endpoint))
    }
    fn labels(&self) -> HashMap<String, String> {
        HashMap::new()
    }
}

impl HTTPEndpoint {
    // common
    pub fn with_allow_cidr_string(&mut self, cidr: impl Into<String>) -> &mut Self {
        self.common_opts.cidr_restrictions.allow(cidr);
        self
    }
    pub fn with_deny_cidr_string(&mut self, cidr: impl Into<String>) -> &mut Self {
        self.common_opts.cidr_restrictions.deny(cidr);
        self
    }
    pub fn with_proxy_proto(&mut self, proxy_proto: ProxyProtocol) -> &mut Self {
        self.common_opts.proxy_proto = Some(proxy_proto);
        self
    }
    pub fn with_metadata(&mut self, metadata: impl Into<String>) -> &mut Self {
        self.common_opts.metadata = Some(metadata.into());
        self
    }
    pub fn with_forwards_to(&mut self, forwards_to: impl Into<String>) -> &mut Self {
        self.common_opts.forwards_to = Some(forwards_to.into());
        self
    }
    // self
    pub fn with_scheme(&mut self, scheme: Scheme) -> &mut Self {
        self.scheme = scheme;
        self
    }
    pub fn with_domain(&mut self, domain: impl Into<String>) -> &mut Self {
        self.domain = Some(domain.into());
        self
    }
    pub fn with_mutual_tlsca(&mut self, mutual_tlsca: Vec<Vec<u8>>) -> &mut Self {
        self.mutual_tlsca = mutual_tlsca;
        self
    }
    pub fn with_compression(&mut self) -> &mut Self {
        self.compression = true;
        self
    }
    pub fn with_websocket_tcp_conversion(&mut self) -> &mut Self {
        self.websocket_tcp_conversion = true;
        self
    }
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

    pub fn with_basic_auth(
        &mut self,
        username: impl Into<String>,
        password: impl Into<String>,
    ) -> &mut Self {
        self.basic_auth.push((username.into(), password.into()));
        self
    }
    pub fn with_oauth<O>(&mut self, oauth: O) -> &mut Self
    where
        O: OauthOptionsTrait + 'static,
    {
        self.oauth = Some(Box::new(oauth));
        self
    }
    pub fn with_oidc<O>(&mut self, oidc: O) -> &mut Self
    where
        O: OidcOptionsTrait + 'static,
    {
        self.oidc = Some(Box::new(oidc));
        self
    }
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
