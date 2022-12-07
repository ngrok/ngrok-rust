use std::collections::HashMap;

use super::common::ProxyProto;
use crate::{
    config::common::{
        private::Sealed,
        CommonOpts,
        TunnelConfig,
        FORWARDS_TO,
    },
    internals::proto::{
        self,
        gen::TcpMiddleware,
        BindExtra,
        BindOpts,
    },
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

#[cfg(test)]
mod test {
    use super::*;

    const METADATA: &str = "testmeta";
    const TEST_FORWARD: &str = "testforward";
    const REMOTE_ADDR: &str = "4.tcp.ngrok.io:1337";
    const ALLOW_CIDR: &str = "0.0.0.0/0";
    const DENY_CIDR: &str = "10.1.1.1/32";

    #[test]
    fn test_interface_to_proto() {
        // pass to a function accepting the trait to avoid
        // "creates a temporary which is freed while still in use"
        tunnel_test(
            TCPEndpoint::default()
                .with_allow_cidr_string(ALLOW_CIDR)
                .with_deny_cidr_string(DENY_CIDR)
                .with_proxy_proto(ProxyProto::V2)
                .with_metadata(METADATA)
                .with_remote_addr(REMOTE_ADDR)
                .with_forwards_to(TEST_FORWARD),
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

        assert_eq!("tcp", tunnel_cfg.proto());

        let opts = tunnel_cfg.opts().unwrap();
        assert!(matches!(opts, BindOpts::Tcp { .. }));
        if let BindOpts::Tcp(endpoint) = opts {
            assert_eq!(REMOTE_ADDR, endpoint.addr);
            assert!(matches!(endpoint.proxy_proto, ProxyProto::V2 { .. }));

            let ip_restriction = endpoint.middleware.ip_restriction.unwrap();
            assert_eq!(Vec::from([ALLOW_CIDR]), ip_restriction.allow_cidrs);
            assert_eq!(Vec::from([DENY_CIDR]), ip_restriction.deny_cidrs);
        }

        assert_eq!(HashMap::new(), tunnel_cfg.labels());
    }
}
