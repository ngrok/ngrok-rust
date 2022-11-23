use std::collections::HashMap;

use crate::{
    common::{
        private,
        CommonOpts,
        FORWARDS_TO,
    },
    internals::proto::{
        BindExtra,
        BindOpts,
    },
};

pub struct LabeledTunnel {
    common_opts: CommonOpts,
}

impl Default for LabeledTunnel {
    fn default() -> Self {
        LabeledTunnel {
            common_opts: CommonOpts::default(),
        }
    }
}

impl private::TunnelConfigPrivate for LabeledTunnel {
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
        "".into()
    }
    fn opts(&self) -> Option<BindOpts> {
        None
    }
    fn labels(&self) -> HashMap<String, String> {
        return HashMap::new();
    }
}

impl LabeledTunnel {
    pub fn with_metadata(&mut self, metadata: impl Into<String>) -> &mut Self {
        self.common_opts.metadata = Some(metadata.into());
        self
    }
}
