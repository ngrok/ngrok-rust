use std::collections::HashMap;

use async_trait::async_trait;
use prost::bytes::{
    self,
    Bytes,
};

use super::{
    common::ProxyProto,
    TunnelBuilder,
};
use crate::{
    config::common::{
        CommonOpts,
        TunnelConfig,
        FORWARDS_TO,
    },
    internals::proto::{
        self,
        gen::{
            middleware_configuration::TlsTermination,
            TlsMiddleware,
        },
        BindExtra,
        BindOpts,
    },
    session::RpcError,
    tunnel::TlsTunnel,
    Session,
};

/// The options for TLS edges.
#[derive(Default, Clone)]
struct TlsOptions {
    pub(crate) common_opts: CommonOpts,
    pub(crate) domain: Option<String>,
    pub(crate) mutual_tlsca: Vec<bytes::Bytes>,
    pub(crate) key_pem: Option<bytes::Bytes>,
    pub(crate) cert_pem: Option<bytes::Bytes>,
}

impl TunnelConfig for TlsOptions {
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
        "tls".into()
    }
    fn opts(&self) -> Option<BindOpts> {
        // fill out all the options, translating to proto here
        let mut tls_endpoint = proto::TlsEndpoint::default();

        if let Some(domain) = self.domain.as_ref() {
            // note: hostname and subdomain are going away in favor of just domain
            tls_endpoint.hostname = domain.clone();
        }
        tls_endpoint.proxy_proto = self.common_opts.proxy_proto;

        // doing some backflips to check both cert_pem and key_pem are set, and avoid unwrapping
        let tls_termination = self
            .cert_pem
            .as_ref()
            .zip(self.key_pem.as_ref())
            .map(|(c, k)| TlsTermination {
                cert: c.to_vec(),
                key: k.to_vec(),
                sealed_key: Vec::new(),
            });

        tls_endpoint.middleware = TlsMiddleware {
            ip_restriction: self.common_opts.ip_restriction(),
            mutual_tls: (!self.mutual_tlsca.is_empty())
                .then_some(self.mutual_tlsca.as_slice().into()),
            tls_termination,
        };

        Some(BindOpts::Tls(tls_endpoint))
    }
    fn labels(&self) -> HashMap<String, String> {
        HashMap::new()
    }
}

impl_builder! {
    /// A builder for a tunnel backing a TCP endpoint.
    TlsTunnelBuilder, TlsOptions, TlsTunnel
}

impl TlsTunnelBuilder {
    /// Restriction placed on the origin of incoming connections to the edge to only allow these CIDR ranges.
    /// Call multiple times to add additional CIDR ranges.
    pub fn with_allow_cidr_string(&mut self, cidr: impl Into<String>) -> &mut Self {
        self.options.common_opts.cidr_restrictions.allow(cidr);
        self
    }
    /// Restriction placed on the origin of incoming connections to the edge to deny these CIDR ranges.
    /// Call multiple times to add additional CIDR ranges.
    pub fn with_deny_cidr_string(&mut self, cidr: impl Into<String>) -> &mut Self {
        self.options.common_opts.cidr_restrictions.deny(cidr);
        self
    }
    /// The version of PROXY protocol to use with this tunnel, None if not using.
    pub fn with_proxy_proto(&mut self, proxy_proto: ProxyProto) -> &mut Self {
        self.options.common_opts.proxy_proto = proxy_proto;
        self
    }
    /// Tunnel-specific opaque metadata. Viewable via the API.
    pub fn with_metadata(&mut self, metadata: impl Into<String>) -> &mut Self {
        self.options.common_opts.metadata = Some(metadata.into());
        self
    }
    /// Tunnel backend metadata. Viewable via the dashboard and API, but has no
    /// bearing on tunnel behavior.
    pub fn with_forwards_to(&mut self, forwards_to: impl Into<String>) -> &mut Self {
        self.options.common_opts.forwards_to = Some(forwards_to.into());
        self
    }
    /// The domain to request for this edge.
    pub fn with_domain(&mut self, domain: impl Into<String>) -> &mut Self {
        self.options.domain = Some(domain.into());
        self
    }
    /// Certificates to use for client authentication at the ngrok edge.
    pub fn with_mutual_tlsca(&mut self, mutual_tlsca: Bytes) -> &mut Self {
        self.options.mutual_tlsca.push(mutual_tlsca);
        self
    }
    /// The key to use for TLS termination at the ngrok edge in PEM format.
    pub fn with_key_pem(&mut self, key_pem: Bytes) -> &mut Self {
        self.options.key_pem = Some(key_pem);
        self
    }
    /// The certificate to use for TLS termination at the ngrok edge in PEM
    /// format.
    pub fn with_cert_pem(&mut self, cert_pem: Bytes) -> &mut Self {
        self.options.cert_pem = Some(cert_pem);
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
    const KEY: &[u8] = "test cert".as_bytes();
    const CERT: &[u8] = "test cert".as_bytes();
    const DOMAIN: &str = "test domain";

    #[test]
    fn test_interface_to_proto() {
        // pass to a function accepting the trait to avoid
        // "creates a temporary which is freed while still in use"
        tunnel_test(
            &TlsTunnelBuilder {
                session: None,
                options: Default::default(),
            }
            .with_allow_cidr_string(ALLOW_CIDR)
            .with_deny_cidr_string(DENY_CIDR)
            .with_proxy_proto(ProxyProto::V2)
            .with_metadata(METADATA)
            .with_domain(DOMAIN)
            .with_mutual_tlsca(CA_CERT.into())
            .with_mutual_tlsca(CA_CERT2.into())
            .with_key_pem(KEY.into())
            .with_cert_pem(CERT.into())
            .with_forwards_to(TEST_FORWARD)
            .options,
        );
    }

    fn tunnel_test<C>(tunnel_cfg: C)
    where
        C: TunnelConfig,
    {
        assert_eq!(TEST_FORWARD, tunnel_cfg.forwards_to());

        let extra = tunnel_cfg.extra();
        assert_eq!(String::default(), extra.token);
        assert_eq!(METADATA, extra.metadata);
        assert_eq!(String::default(), extra.ip_policy_ref);

        assert_eq!("tls", tunnel_cfg.proto());

        let opts = tunnel_cfg.opts().unwrap();
        assert!(matches!(opts, BindOpts::Tls { .. }));
        if let BindOpts::Tls(endpoint) = opts {
            assert_eq!(DOMAIN, endpoint.hostname);
            assert_eq!(String::default(), endpoint.subdomain);
            assert!(matches!(endpoint.proxy_proto, ProxyProto::V2 { .. }));
            assert!(!endpoint.mutual_tls_at_agent);

            let middleware = endpoint.middleware;
            let ip_restriction = middleware.ip_restriction.unwrap();
            assert_eq!(Vec::from([ALLOW_CIDR]), ip_restriction.allow_cidrs);
            assert_eq!(Vec::from([DENY_CIDR]), ip_restriction.deny_cidrs);

            let tls_termination = middleware.tls_termination.unwrap();
            assert_eq!(CERT, tls_termination.cert);
            assert_eq!(KEY, tls_termination.key);
            assert!(tls_termination.sealed_key.is_empty());

            let mutual_tls = middleware.mutual_tls.unwrap();
            let mut agg = CA_CERT.to_vec();
            agg.extend(CA_CERT2.to_vec());
            assert_eq!(agg, mutual_tls.mutual_tls_ca);
        }

        assert_eq!(HashMap::new(), tunnel_cfg.labels());
    }
}
