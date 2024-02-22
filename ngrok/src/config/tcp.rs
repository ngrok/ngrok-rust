use std::collections::HashMap;

use url::Url;

use super::{
    common::ProxyProto,
    Policy,
};
// These are used for doc comment links.
#[allow(unused_imports)]
use crate::config::{
    ForwarderBuilder,
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

    fn forwards_proto(&self) -> String {
        // not supported
        String::new()
    }

    fn disable_app_cert_verification(&self) -> bool {
        self.common_opts.disable_app_cert_verification
    }

    fn opts(&self) -> Option<BindOpts> {
        // fill out all the options, translating to proto here
        let mut tcp_endpoint = proto::TcpEndpoint::default();

        if let Some(remote_addr) = self.remote_addr.as_ref() {
            tcp_endpoint.addr = remote_addr.clone();
        }
        tcp_endpoint.proxy_proto = self.common_opts.proxy_proto;

        tcp_endpoint.ip_restriction = self.common_opts.ip_restriction();

        tcp_endpoint.policy = self.common_opts.policy.clone().map(From::from);

        Some(BindOpts::Tcp(tcp_endpoint))
    }
    fn labels(&self) -> HashMap<String, String> {
        HashMap::new()
    }
}

impl_builder! {
    /// A builder for a tunnel backing a TCP endpoint.
    ///
    /// https://ngrok.com/docs/tcp/
    TcpTunnelBuilder, TcpOptions, TcpTunnel, endpoint
}

/// The options for a TCP edge.
impl TcpTunnelBuilder {
    /// Add the provided CIDR to the allowlist.
    ///
    /// https://ngrok.com/docs/tcp/ip-restrictions/
    pub fn allow_cidr(&mut self, cidr: impl Into<String>) -> &mut Self {
        self.options.common_opts.cidr_restrictions.allow(cidr);
        self
    }
    /// Add the provided CIDR to the denylist.
    ///
    /// https://ngrok.com/docs/tcp/ip-restrictions/
    pub fn deny_cidr(&mut self, cidr: impl Into<String>) -> &mut Self {
        self.options.common_opts.cidr_restrictions.deny(cidr);
        self
    }
    /// Sets the PROXY protocol version for connections over this tunnel.
    pub fn proxy_proto(&mut self, proxy_proto: ProxyProto) -> &mut Self {
        self.options.common_opts.proxy_proto = proxy_proto;
        self
    }
    /// Sets the opaque metadata string for this tunnel.
    ///
    /// https://ngrok.com/docs/api/resources/tunnels/#tunnel-fields
    pub fn metadata(&mut self, metadata: impl Into<String>) -> &mut Self {
        self.options.common_opts.metadata = Some(metadata.into());
        self
    }
    /// Sets the ForwardsTo string for this tunnel. This can be viewed via the
    /// API or dashboard.
    ///
    /// This overrides the default process info if using
    /// [TunnelBuilder::listen], and is in turn overridden by the url provided
    /// to [ForwarderBuilder::listen_and_forward].
    ///
    /// https://ngrok.com/docs/api/resources/tunnels/#tunnel-fields
    pub fn forwards_to(&mut self, forwards_to: impl Into<String>) -> &mut Self {
        self.options.common_opts.forwards_to = Some(forwards_to.into());
        self
    }

    /// Disables backend TLS certificate verification for forwards from this tunnel.
    pub fn disable_app_cert_verification(&mut self) -> &mut Self {
        self.options.common_opts.disable_app_cert_verification = true;
        self
    }

    /// Sets the TCP address to request for this edge.
    ///
    /// https://ngrok.com/docs/network-edge/domains-and-tcp-addresses/#tcp-addresses
    pub fn remote_addr(&mut self, remote_addr: impl Into<String>) -> &mut Self {
        self.options.remote_addr = Some(remote_addr.into());
        self
    }

    /// Set the policy for this edge.
    pub fn policy<S>(&mut self, s: S) -> Result<&mut Self, S::Error>
    where
        S: TryInto<Policy>,
    {
        self.options.common_opts.policy = Some(s.try_into()?);
        Ok(self)
    }

    pub(crate) async fn for_forwarding_to(&mut self, to_url: &Url) -> &mut Self {
        self.options.common_opts.for_forwarding_to(to_url);
        self
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::config::policies::test::POLICY_JSON;

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
            &TcpTunnelBuilder {
                session: None,
                options: Default::default(),
            }
            .allow_cidr(ALLOW_CIDR)
            .deny_cidr(DENY_CIDR)
            .proxy_proto(ProxyProto::V2)
            .metadata(METADATA)
            .remote_addr(REMOTE_ADDR)
            .forwards_to(TEST_FORWARD)
            .policy(POLICY_JSON)
            .unwrap()
            .options,
        );
    }

    fn tunnel_test<C>(tunnel_cfg: &C)
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
