use std::collections::HashMap;

use async_trait::async_trait;
use bytes::{
    self,
    Bytes,
};

use super::{
    common::ProxyProto,
    TunnelBuilder,
};
use crate::{
    config::common::{
        default_forwards_to,
        CommonOpts,
        TunnelConfig,
    },
    internals::proto::{
        self,
        BindExtra,
        BindOpts,
        TlsTermination,
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
                key: k.to_vec().into(),
                sealed_key: Vec::new(),
            });

        tls_endpoint.ip_restriction = self.common_opts.ip_restriction();
        tls_endpoint.mutual_tls_at_edge =
            (!self.mutual_tlsca.is_empty()).then_some(self.mutual_tlsca.as_slice().into());
        tls_endpoint.tls_termination = tls_termination;

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

    /// Sets the key and certificate in PEM format for TLS termination at the
    /// ngrok edge.
    pub fn termination(mut self, cert_pem: Bytes, key_pem: Bytes) -> Self {
        self.options.key_pem = Some(key_pem);
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
            .allow_cidr(ALLOW_CIDR)
            .deny_cidr(DENY_CIDR)
            .proxy_proto(ProxyProto::V2)
            .metadata(METADATA)
            .domain(DOMAIN)
            .mutual_tlsca(CA_CERT.into())
            .mutual_tlsca(CA_CERT2.into())
            .termination(CERT.into(), KEY.into())
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

        assert_eq!("tls", tunnel_cfg.proto());

        let opts = tunnel_cfg.opts().unwrap();
        assert!(matches!(opts, BindOpts::Tls { .. }));
        if let BindOpts::Tls(endpoint) = opts {
            assert_eq!(DOMAIN, endpoint.hostname);
            assert_eq!(String::default(), endpoint.subdomain);
            assert!(matches!(endpoint.proxy_proto, ProxyProto::V2 { .. }));
            assert!(!endpoint.mutual_tls_at_agent);

            let ip_restriction = endpoint.ip_restriction.unwrap();
            assert_eq!(Vec::from([ALLOW_CIDR]), ip_restriction.allow_cidrs);
            assert_eq!(Vec::from([DENY_CIDR]), ip_restriction.deny_cidrs);

            let tls_termination = endpoint.tls_termination.unwrap();
            assert_eq!(CERT, tls_termination.cert);
            assert_eq!(KEY, *tls_termination.key);
            assert!(tls_termination.sealed_key.is_empty());

            let mutual_tls = endpoint.mutual_tls_at_edge.unwrap();
            let mut agg = CA_CERT.to_vec();
            agg.extend(CA_CERT2.to_vec());
            assert_eq!(agg, mutual_tls.mutual_tls_ca);
        }

        assert_eq!(HashMap::new(), tunnel_cfg.labels());
    }
}
