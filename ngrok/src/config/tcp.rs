use std::collections::HashMap;

use crate::{internals::proto::{BindExtra, BindOpts, self}, common::{CommonOpts, TunnelConfig, FORWARDS_TO, private}};

pub struct TCPEndpoint {
    pub(crate) common_opts: CommonOpts,
}

impl TunnelConfig for TCPEndpoint {}

impl Default for TCPEndpoint {
    fn default() -> Self {
        TCPEndpoint {
            common_opts: CommonOpts::default(),
        }
    }
}

impl private::TunnelConfigPrivate for TCPEndpoint {
    fn forwards_to(&self) -> String {
        self.common_opts.forwards_to.clone().unwrap_or(FORWARDS_TO.into())
    }
    fn extra(&self) -> BindExtra {
        BindExtra {
            token: Default::default(),
            ip_policy_ref: Default::default(),
            metadata: self.common_opts.metadata.clone().unwrap_or_default(),
        }
    }
    fn proto(&self) -> String {"tcp".into()}
    fn opts(&self) -> Option<BindOpts> {
        let tcp_endpoint = proto::TCPEndpoint::default();
        // todo: fill out all the options here? or use the proto as our storage?
        Some(BindOpts::TCPEndpoint(tcp_endpoint))
    }
    fn labels(&self) -> HashMap<String,String> {
        return HashMap::new();
    }
}

impl TCPEndpoint {
    pub fn with_metadata(&mut self, metadata: impl Into<String>) -> &mut Self {
        self.common_opts.metadata = Some(metadata.into());
        self
    }
}