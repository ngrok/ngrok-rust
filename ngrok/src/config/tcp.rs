use std::collections::HashMap;

use crate::{
    common::{
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
    mw::TcpMiddleware,
};

/// The options for a TCP edge.
#[derive(Default)]
pub struct TCPEndpoint {
    /// Common tunnel configuration options.
    pub(crate) common_opts: CommonOpts,

    /// The TCP address to request for this edge.
    pub(crate) remote_addr: Option<String>,

    // An HTTP Server to run traffic on
    pub(crate) http_server: Option<String>, // todo
}

impl private::TunnelConfigPrivate for TCPEndpoint {
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
        "tcp".into()
    }
    fn opts(&self) -> Option<BindOpts> {
        // fill out all the options, translating to proto here
        let mut tcp_endpoint = proto::TcpEndpoint::default();

        if let Some(remote_addr) = self.remote_addr.as_ref() {
            tcp_endpoint.addr = remote_addr.clone();
        }
        tcp_endpoint.proxy_proto = self.common_opts.as_proxy_proto();

        tcp_endpoint.middleware = TcpMiddleware {
            ip_restriction: self.common_opts.cidr_to_proto_config(),
        };

        Some(BindOpts::Tcp(tcp_endpoint))
    }
    fn labels(&self) -> HashMap<String, String> {
        HashMap::new()
    }
}

impl TCPEndpoint {
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
    pub fn with_remote_addr(&mut self, remote_addr: impl Into<String>) -> &mut Self {
        self.remote_addr = Some(remote_addr.into());
        self
    }
}
