use std::collections::HashMap;

use crate::{common::{CommonOpts, TunnelConfig, private, FORWARDS_TO}, internals::proto::{BindExtra, BindOpts, self}};

pub struct TLSEndpoint {
    common_opts: CommonOpts,
}

impl TunnelConfig for TLSEndpoint {}

impl Default for TLSEndpoint {
    fn default() -> Self {
        TLSEndpoint {
            common_opts: CommonOpts::default(),
        }
    }
}

impl private::TunnelConfigPrivate for TLSEndpoint {
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
    fn proto(&self) -> String {"tls".into()}
    fn opts(&self) -> Option<BindOpts> {
        let tls_endpoint = proto::TLSEndpoint::default();
        // todo: fill out all the options here? or use the proto as our storage?
        Some(BindOpts::TLSEndpoint(tls_endpoint))
    }
    fn labels(&self) -> HashMap<String,String> {
        return HashMap::new();
    }
}

impl TLSEndpoint {
    pub fn with_metadata(&mut self, metadata: impl Into<String>) -> &mut Self {
        self.common_opts.metadata = Some(metadata.into());
        self
    }
}
