use std::collections::HashMap;

use super::common::ProxyProto;
use crate::{
    common::{
        private::Sealed,
        CommonOpts,
        TunnelConfig,
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
    pub(crate) common_opts: CommonOpts,
    pub(crate) remote_addr: Option<String>,
}

impl Sealed for TCPEndpoint {}
impl TunnelConfig for TCPEndpoint {
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
        tcp_endpoint.proxy_proto = self.common_opts.proxy_proto;

        tcp_endpoint.middleware = TcpMiddleware {
            ip_restriction: self.common_opts.ip_restriction(),
        };

        Some(BindOpts::Tcp(tcp_endpoint))
    }
    fn labels(&self) -> HashMap<String, String> {
        HashMap::new()
    }
}

/// The options for a TCP edge.
impl TCPEndpoint {
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
    /// The TCP address to request for this edge.
    pub fn with_remote_addr(&mut self, remote_addr: impl Into<String>) -> &mut Self {
        self.remote_addr = Some(remote_addr.into());
        self
    }
}
