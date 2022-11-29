use std::collections::HashMap;

use crate::{
    common::{
        self,
        private,
        CommonOpts,
        ProxyProtocol,
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
    /// Common tunnel options
    pub(crate) common_opts: CommonOpts,

    /// The domain to request for this edge.
    pub(crate) domain: Option<String>,

    /// Certificates to use for client authentication at the ngrok edge.
    pub(crate) mutual_tlsca: Vec<Vec<u8>>, // something more idiomatic here?

    /// The key to use for TLS termination at the ngrok edge in PEM format.
    pub(crate) key_pem: Option<Vec<u8>>, // something more idiomatic here?
    /// The certificate to use for TLS termination at the ngrok edge in PEM
    /// format.
    pub(crate) cert_pem: Option<Vec<u8>>, // something more idiomatic here?

    // An HTTP Server to run traffic on
    pub(crate) http_server: Option<String>, // todo
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
            tls_endpoint.hostname = domain.clone();
            // todo: ngrok-rs has "hostname" and "subdomain", ngrok-go has just "domain"?
        }
        tls_endpoint.proxy_proto = self.common_opts.as_proxy_proto();

        let tls_termination = match self.cert_pem.is_some() && self.key_pem.is_some() {
            true => Some(TlsTermination {
                cert: self.cert_pem.as_ref().unwrap().clone(),
                key: self.key_pem.as_ref().unwrap().clone(),
                sealed_key: Vec::new(), // todo: looks to be unused?
            }),
            false => None,
        };

        tls_endpoint.middleware = TlsMiddleware {
            ip_restriction: self.common_opts.cidr_to_proto_config(),
            mutual_tls: common::mutual_tls_to_proto_config(&self.mutual_tlsca),
            tls_termination,
        };

        Some(BindOpts::Tls(tls_endpoint))
    }
    fn labels(&self) -> HashMap<String, String> {
        HashMap::new()
    }
}

impl TLSEndpoint {
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
    pub fn with_domain(&mut self, domain: impl Into<String>) -> &mut Self {
        self.domain = Some(domain.into());
        self
    }
    pub fn with_mutual_tlsca(&mut self, mutual_tlsca: Vec<Vec<u8>>) -> &mut Self {
        self.mutual_tlsca = mutual_tlsca;
        self
    }
    pub fn with_key_pem(&mut self, key_pem: Vec<u8>) -> &mut Self {
        self.key_pem = Some(key_pem);
        self
    }
    pub fn with_cert_pem(&mut self, cert_pem: Vec<u8>) -> &mut Self {
        self.cert_pem = Some(cert_pem);
        self
    }
}
