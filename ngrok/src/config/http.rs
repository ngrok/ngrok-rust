use std::{
    collections::HashMap,
    str::FromStr,
};

use async_trait::async_trait;
use bytes::{
    self,
    Bytes,
};
use thiserror::Error;

use super::{
    common::ProxyProto,
    TunnelBuilder,
};
use crate::{
    config::{
        common::{
            default_forwards_to,
            CommonOpts,
            TunnelConfig,
        },
        headers::Headers,
        oauth::OauthOptions,
        oidc::OidcOptions,
        webhook_verification::WebhookVerification,
    },
    internals::proto::{
        BasicAuth,
        BasicAuthCredential,
        BindExtra,
        BindOpts,
        CircuitBreaker,
        Compression,
        HttpEndpoint,
        WebsocketTcpConverter,
    },
    session::RpcError,
    tunnel::HttpTunnel,
    Session,
};

/// Error representing invalid string for Scheme
#[derive(Debug, Clone, Error)]
#[error("invalid scheme string: {}", .0)]
pub struct InvalidSchemeString(String);

/// The URL scheme for this HTTP endpoint.
///
/// [Scheme::HTTPS] will enable TLS termination at the ngrok edge.
#[derive(Clone, Default, Eq, PartialEq)]
pub enum Scheme {
    /// The `http` URL scheme.
    HTTP,
    /// The `https` URL scheme.
    #[default]
    HTTPS,
}

impl FromStr for Scheme {
    type Err = InvalidSchemeString;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        use Scheme::*;
        Ok(match s.to_uppercase().as_str() {
            "HTTP" => HTTP,
            "HTTPS" => HTTPS,
            _ => return Err(InvalidSchemeString(s.into())),
        })
    }
}

/// The options for a HTTP edge.
#[derive(Default, Clone)]
struct HttpOptions {
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
    pub(crate) oauth: Option<OauthOptions>,
    pub(crate) oidc: Option<OidcOptions>,
    pub(crate) webhook_verification: Option<WebhookVerification>,
}

impl TunnelConfig for HttpOptions {
    fn forwards_to(&self) -> String {
        self.common_opts
            .forwards_to
            .clone()
            .unwrap_or(default_forwards_to().into())
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
        let http_endpoint = HttpEndpoint {
            proxy_proto: self.common_opts.proxy_proto,
            hostname: self.domain.clone().unwrap_or_default(),
            compression: self.compression.then_some(Compression {}),
            circuit_breaker: (self.circuit_breaker != 0f64).then_some(CircuitBreaker {
                error_threshold: self.circuit_breaker,
            }),
            ip_restriction: self.common_opts.ip_restriction(),
            basic_auth: (!self.basic_auth.is_empty()).then_some(self.basic_auth.as_slice().into()),
            oauth: self.oauth.clone().map(From::from),
            oidc: self.oidc.clone().map(From::from),
            webhook_verification: self.webhook_verification.clone().map(From::from),
            mutual_tls_ca: (!self.mutual_tlsca.is_empty())
                .then_some(self.mutual_tlsca.as_slice().into()),
            request_headers: self
                .request_headers
                .has_entries()
                .then_some(self.request_headers.clone().into()),
            response_headers: self
                .response_headers
                .has_entries()
                .then_some(self.response_headers.clone().into()),
            websocket_tcp_converter: self
                .websocket_tcp_conversion
                .then_some(WebsocketTcpConverter {}),
            ..Default::default()
        };

        Some(BindOpts::Http(http_endpoint))
    }
    fn labels(&self) -> HashMap<String, String> {
        HashMap::new()
    }
}

// transform into the wire protocol format
impl From<&[(String, String)]> for BasicAuth {
    fn from(v: &[(String, String)]) -> Self {
        BasicAuth {
            credentials: v.iter().cloned().map(From::from).collect(),
        }
    }
}

// transform into the wire protocol format
impl From<(String, String)> for BasicAuthCredential {
    fn from(b: (String, String)) -> Self {
        BasicAuthCredential {
            username: b.0,
            cleartext_password: b.1,
            hashed_password: vec![], // unused in this context
        }
    }
}

impl_builder! {
    /// A builder for a tunnel backing an HTTP endpoint.
    HttpTunnelBuilder, HttpOptions, HttpTunnel
}

impl HttpTunnelBuilder {
    /// Add the provided CIDR to the allowlist.
    pub fn allow_cidr(mut self, cidr: impl Into<String>) -> Self {
        self.options.common_opts.cidr_restrictions.allow(cidr);
        self
    }
    /// Add the provided CIDR to the denylist.
    pub fn deny_cidr(mut self, cidr: impl Into<String>) -> Self {
        self.options.common_opts.cidr_restrictions.deny(cidr);
        self
    }
    /// Sets the PROXY protocol version for connections over this tunnel.
    pub fn proxy_proto(mut self, proxy_proto: ProxyProto) -> Self {
        self.options.common_opts.proxy_proto = proxy_proto;
        self
    }
    /// Sets the opaque metadata string for this tunnel.
    pub fn metadata(mut self, metadata: impl Into<String>) -> Self {
        self.options.common_opts.metadata = Some(metadata.into());
        self
    }
    /// Sets the ForwardsTo string for this tunnel. This can be viewed via the
    /// API or dashboard.
    pub fn forwards_to(mut self, forwards_to: impl Into<String>) -> Self {
        self.options.common_opts.forwards_to = Some(forwards_to.into());
        self
    }
    /// Sets the scheme for this edge.
    pub fn scheme(mut self, scheme: Scheme) -> Self {
        self.options.scheme = scheme;
        self
    }
    /// Sets the domain to request for this edge.
    pub fn domain(mut self, domain: impl Into<String>) -> Self {
        self.options.domain = Some(domain.into());
        self
    }
    /// Adds a certificate in PEM format to use for mutual TLS authentication.
    ///
    /// These will be used to authenticate client certificates for requests at
    /// the ngrok edge.
    pub fn mutual_tlsca(mut self, mutual_tlsca: Bytes) -> Self {
        self.options.mutual_tlsca.push(mutual_tlsca);
        self
    }
    /// Enables gzip compression.
    pub fn compression(mut self) -> Self {
        self.options.compression = true;
        self
    }
    /// Enables the websocket-to-tcp converter.
    pub fn websocket_tcp_conversion(mut self) -> Self {
        self.options.websocket_tcp_conversion = true;
        self
    }
    /// Sets the 5XX response ratio at which the ngrok edge will stop sending
    /// requests to this tunnel.
    pub fn circuit_breaker(mut self, circuit_breaker: f64) -> Self {
        self.options.circuit_breaker = circuit_breaker;
        self
    }

    /// Adds a header to all requests to this edge.
    pub fn request_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.options.request_headers.add(name, value);
        self
    }
    /// Adds a header to all responses coming from this edge.
    pub fn response_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.options.response_headers.add(name, value);
        self
    }
    /// Removes a header from requests to this edge.
    pub fn remove_request_header(mut self, name: impl Into<String>) -> Self {
        self.options.request_headers.remove(name);
        self
    }
    /// Removes a header from responses from this edge.
    pub fn remove_response_header(mut self, name: impl Into<String>) -> Self {
        self.options.response_headers.remove(name);
        self
    }

    /// Adds the provided credentials to the list of basic authentication
    /// credentials.
    pub fn basic_auth(mut self, username: impl Into<String>, password: impl Into<String>) -> Self {
        self.options
            .basic_auth
            .push((username.into(), password.into()));
        self
    }

    /// Set the OAuth configuraton for this edge.
    pub fn oauth(mut self, oauth: OauthOptions) -> Self {
        self.options.oauth = Some(oauth);
        self
    }

    /// Set the OIDC configuration for this edge.
    pub fn oidc(mut self, oidc: OidcOptions) -> Self {
        self.options.oidc = Some(oidc);
        self
    }

    /// Configures webhook verification for this edge.
    pub fn webhook_verification(
        mut self,
        provider: impl Into<String>,
        secret: impl Into<String>,
    ) -> Self {
        self.options.webhook_verification = Some(WebhookVerification {
            provider: provider.into(),
            secret: secret.into().into(),
        });
        self
    }
}

#[cfg(test)]
mod test {
    use super::*;

    const METADATA: &str = "testmeta";
    const TEST_FORWARD: &str = "testforward";
    const ALLOW_CIDR: &str = "0.0.0.0/0";
    const DENY_CIDR: &str = "10.1.1.1/32";
    const CA_CERT: &[u8] = "test ca cert".as_bytes();
    const CA_CERT2: &[u8] = "test ca cert2".as_bytes();
    const DOMAIN: &str = "test domain";

    #[test]
    fn test_interface_to_proto() {
        // pass to a function accepting the trait to avoid
        // "creates a temporary which is freed while still in use"
        tunnel_test(
            &HttpTunnelBuilder {
                session: None,
                options: Default::default(),
            }
            .allow_cidr(ALLOW_CIDR)
            .deny_cidr(DENY_CIDR)
            .proxy_proto(ProxyProto::V2)
            .metadata(METADATA)
            .scheme(Scheme::from_str("hTtPs").unwrap())
            .domain(DOMAIN)
            .mutual_tlsca(CA_CERT.into())
            .mutual_tlsca(CA_CERT2.into())
            .compression()
            .websocket_tcp_conversion()
            .circuit_breaker(0.5)
            .request_header("X-Req-Yup", "true")
            .response_header("X-Res-Yup", "true")
            .remove_request_header("X-Req-Nope")
            .remove_response_header("X-Res-Nope")
            .oauth(OauthOptions::new("google"))
            .oauth(
                OauthOptions::new("google")
                    .allow_email("<user>@<domain>")
                    .allow_domain("<domain>")
                    .scope("<scope>"),
            )
            .oidc(OidcOptions::new("<url>", "<id>", "<secret>"))
            .oidc(
                OidcOptions::new("<url>", "<id>", "<secret>")
                    .allow_email("<user>@<domain>")
                    .allow_domain("<domain>")
                    .scope("<scope>"),
            )
            .webhook_verification("twilio", "asdf")
            .basic_auth("ngrok", "online1line")
            .forwards_to(TEST_FORWARD)
            .options,
        );
    }

    fn tunnel_test<C>(tunnel_cfg: C)
    where
        C: TunnelConfig,
    {
        assert_eq!(TEST_FORWARD, tunnel_cfg.forwards_to());

        let extra = tunnel_cfg.extra();
        assert_eq!(String::default(), *extra.token);
        assert_eq!(METADATA, extra.metadata);
        assert_eq!(String::default(), extra.ip_policy_ref);

        assert_eq!("https", tunnel_cfg.proto());

        let opts = tunnel_cfg.opts().unwrap();
        assert!(matches!(opts, BindOpts::Http { .. }));
        if let BindOpts::Http(endpoint) = opts {
            assert_eq!(DOMAIN, endpoint.hostname);
            assert_eq!(String::default(), endpoint.subdomain);
            assert!(matches!(endpoint.proxy_proto, ProxyProto::V2 { .. }));

            let ip_restriction = endpoint.ip_restriction.unwrap();
            assert_eq!(Vec::from([ALLOW_CIDR]), ip_restriction.allow_cidrs);
            assert_eq!(Vec::from([DENY_CIDR]), ip_restriction.deny_cidrs);

            let mutual_tls = endpoint.mutual_tls_ca.unwrap();
            let mut agg = CA_CERT.to_vec();
            agg.extend(CA_CERT2.to_vec());
            assert_eq!(agg, mutual_tls.mutual_tls_ca);

            assert!(endpoint.compression.is_some());
            assert!(endpoint.websocket_tcp_converter.is_some());
            assert_eq!(0.5f64, endpoint.circuit_breaker.unwrap().error_threshold);

            let request_headers = endpoint.request_headers.unwrap();
            assert_eq!(["X-Req-Yup:true"].to_vec(), request_headers.add);
            assert_eq!(["X-Req-Nope"].to_vec(), request_headers.remove);

            let response_headers = endpoint.response_headers.unwrap();
            assert_eq!(["X-Res-Yup:true"].to_vec(), response_headers.add);
            assert_eq!(["X-Res-Nope"].to_vec(), response_headers.remove);

            let webhook = endpoint.webhook_verification.unwrap();
            assert_eq!("twilio", webhook.provider);
            assert_eq!("asdf", *webhook.secret);
            assert!(webhook.sealed_secret.is_empty());

            let creds = endpoint.basic_auth.unwrap().credentials;
            assert_eq!(1, creds.len());
            assert_eq!("ngrok", creds[0].username);
            assert_eq!("online1line", creds[0].cleartext_password);
            assert!(creds[0].hashed_password.is_empty());

            let oauth = endpoint.oauth.unwrap();
            assert_eq!("google", oauth.provider);
            assert_eq!(["<user>@<domain>"].to_vec(), oauth.allow_emails);
            assert_eq!(["<domain>"].to_vec(), oauth.allow_domains);
            assert_eq!(["<scope>"].to_vec(), oauth.scopes);
            assert_eq!(String::default(), oauth.client_id);
            assert_eq!(String::default(), *oauth.client_secret);
            assert!(oauth.sealed_client_secret.is_empty());

            let oidc = endpoint.oidc.unwrap();
            assert_eq!("<url>", oidc.issuer_url);
            assert_eq!(["<user>@<domain>"].to_vec(), oidc.allow_emails);
            assert_eq!(["<domain>"].to_vec(), oidc.allow_domains);
            assert_eq!(["<scope>"].to_vec(), oidc.scopes);
            assert_eq!("<id>", oidc.client_id);
            assert_eq!("<secret>", *oidc.client_secret);
            assert!(oidc.sealed_client_secret.is_empty());
        }

        assert_eq!(HashMap::new(), tunnel_cfg.labels());
    }
}
