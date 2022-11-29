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

/// Options for labeled tunnels.
#[derive(Default)]
pub struct LabeledTunnel {
    /// Common tunnel configuration options.
    pub(crate) common_opts: CommonOpts,

    /// A map of label, value pairs for this tunnel.
    pub(crate) labels: HashMap<String, String>,

    // An HTTP Server to run traffic on
    pub(crate) http_server: Option<String>, // todo
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
        self.labels.clone()
    }
}

impl LabeledTunnel {
    // common
    pub fn with_metadata(&mut self, metadata: impl Into<String>) -> &mut Self {
        self.common_opts.metadata = Some(metadata.into());
        self
    }
    // self
    pub fn with_label(&mut self, label: impl Into<String>, value: impl Into<String>) -> &mut Self {
        self.labels.insert(label.into(), value.into());
        self
    }
}
