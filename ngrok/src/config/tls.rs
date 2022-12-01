use std::collections::HashMap;

use prost::bytes::{
    self,
    Bytes,
};

use super::common::ProxyProto;
use crate::{
    common::{
        private,
        CommonOpts,
        FORWARDS_TO,
    },
    internals::proto::{
        self,
        BindExtra,
        BindOpts,
    },
    mw::{
        middleware_configuration::TlsTermination,
        TlsMiddleware,
    },
};

/// The options for TLS edges.
#[derive(Default)]
pub struct TLSEndpoint {
    pub(crate) common_opts: CommonOpts,
    pub(crate) domain: Option<String>,
    pub(crate) mutual_tlsca: Vec<bytes::Bytes>,
    pub(crate) key_pem: Option<bytes::Bytes>,
    pub(crate) cert_pem: Option<bytes::Bytes>,
}

impl private::TunnelConfigPrivate for TLSEndpoint {
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
        let tls_termination =
            self.cert_pem
            .as_ref()
            .and_then(|c| self.key_pem.as_ref().map(|k| (c, k)))
            .map(|(c, k)| TlsTermination {
                cert: c.to_vec(),
                key: k.to_vec(),
                sealed_key: Vec::new(),
            });

        tls_endpoint.middleware = TlsMiddleware {
            ip_restriction: self.common_opts.ip_restriction(),
            mutual_tls: (!self.mutual_tlsca.is_empty()).then_some(self.mutual_tlsca.as_slice().into()),
            tls_termination,
        };

        Some(BindOpts::Tls(tls_endpoint))
    }
    fn labels(&self) -> HashMap<String, String> {
        HashMap::new()
    }
}

impl TLSEndpoint {
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
    /// The domain to request for this edge.
    pub fn with_domain(&mut self, domain: impl Into<String>) -> &mut Self {
        self.domain = Some(domain.into());
        self
    }
    /// Certificates to use for client authentication at the ngrok edge.
    pub fn with_mutual_tlsca(&mut self, mutual_tlsca: Bytes) -> &mut Self {
        self.mutual_tlsca.push(mutual_tlsca);
        self
    }
    /// The key to use for TLS termination at the ngrok edge in PEM format.
    pub fn with_key_pem(&mut self, key_pem: Bytes) -> &mut Self {
        self.key_pem = Some(key_pem);
        self
    }
    /// The certificate to use for TLS termination at the ngrok edge in PEM
    /// format.
    pub fn with_cert_pem(&mut self, cert_pem: Bytes) -> &mut Self {
        self.cert_pem = Some(cert_pem);
        self
    }
}
