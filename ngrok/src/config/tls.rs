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

#[derive(Default)]
pub struct TLSEndpoint {
    common_opts: CommonOpts,
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
        // fill out all the options here, translating to proto here
        let mut tls_endpoint = proto::TlsEndpoint::default();

        if let Some(proxy_proto) = self.common_opts.proxy_proto {
            tls_endpoint.proxy_proto = proxy_proto;
        }

        Some(BindOpts::Tls(tls_endpoint))
    }
    fn labels(&self) -> HashMap<String, String> {
        HashMap::new()
    }
}

impl TLSEndpoint {
    pub fn with_metadata(&mut self, metadata: impl Into<String>) -> &mut Self {
        self.common_opts.metadata = Some(metadata.into());
        self
    }
}
