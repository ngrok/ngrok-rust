use std::collections::HashMap;

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
};

pub struct TCPEndpoint {
    pub(crate) common_opts: CommonOpts,
}

impl Default for TCPEndpoint {
    fn default() -> Self {
        TCPEndpoint {
            common_opts: CommonOpts::default(),
        }
    }
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
        // fill out all the options here, translating to proto here
        let mut tcp_endpoint = proto::TCPEndpoint::default();

        if let Some(proxy_proto) = self.common_opts.proxy_proto {
            tcp_endpoint.proxy_proto = proxy_proto;
        }

        Some(BindOpts::TCPEndpoint(tcp_endpoint))
    }
    fn labels(&self) -> HashMap<String, String> {
        return HashMap::new();
    }
}

impl TCPEndpoint {
    pub fn with_metadata(&mut self, metadata: impl Into<String>) -> &mut Self {
        self.common_opts.metadata = Some(metadata.into());
        self
    }
}
