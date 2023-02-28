use std::collections::HashMap;

use async_trait::async_trait;

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
    },
    session::RpcError,
    tunnel::TcpTunnel,
    Session,
};

/// The options for a TCP edge.
#[derive(Default, Clone)]
struct TcpOptions {
    pub(crate) common_opts: CommonOpts,
    pub(crate) remote_addr: Option<String>,
}

impl TunnelConfig for TcpOptions {
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
        "tcp".into()
    }
    fn opts(&self) -> Option<BindOpts> {
        // fill out all the options, translating to proto here
        let mut tcp_endpoint = proto::TcpEndpoint::default();

        if let Some(remote_addr) = self.remote_addr.as_ref() {
            tcp_endpoint.addr = remote_addr.clone();
        }
        tcp_endpoint.proxy_proto = self.common_opts.proxy_proto;

        tcp_endpoint.ip_restriction = self.common_opts.ip_restriction();

        Some(BindOpts::Tcp(tcp_endpoint))
    }
    fn labels(&self) -> HashMap<String, String> {
        HashMap::new()
    }
}

impl_builder! {
    /// A builder for a tunnel backing a TCP endpoint.
    TcpTunnelBuilder, TcpOptions, TcpTunnel
}

/// The options for a TCP edge.
impl TcpTunnelBuilder {
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
    /// Sets the TCP address to request for this edge.
    pub fn remote_addr(mut self, remote_addr: impl Into<String>) -> Self {
        self.options.remote_addr = Some(remote_addr.into());
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
            TcpTunnelBuilder {
                session: None,
                options: Default::default(),
            }
            .allow_cidr(ALLOW_CIDR)
            .deny_cidr(DENY_CIDR)
            .proxy_proto(ProxyProto::V2)
            .metadata(METADATA)
            .remote_addr(REMOTE_ADDR)
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

        assert_eq!("tcp", tunnel_cfg.proto());

        let opts = tunnel_cfg.opts().unwrap();
        assert!(matches!(opts, BindOpts::Tcp { .. }));
        if let BindOpts::Tcp(endpoint) = opts {
            assert_eq!(REMOTE_ADDR, endpoint.addr);
            assert!(matches!(endpoint.proxy_proto, ProxyProto::V2 { .. }));

            let ip_restriction = endpoint.ip_restriction.unwrap();
            assert_eq!(Vec::from([ALLOW_CIDR]), ip_restriction.allow_cidrs);
            assert_eq!(Vec::from([DENY_CIDR]), ip_restriction.deny_cidrs);
        }

        assert_eq!(HashMap::new(), tunnel_cfg.labels());
    }
}
