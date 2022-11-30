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
    pub(crate) common_opts: CommonOpts,
    pub(crate) labels: HashMap<String, String>,
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

/// Options for labeled tunnels.
impl LabeledTunnel {
    /// Tunnel-specific opaque metadata. Viewable via the API.
    pub fn with_metadata(&mut self, metadata: impl Into<String>) -> &mut Self {
        self.common_opts.metadata = Some(metadata.into());
        self
    }

    /// Add a label, value pair for this tunnel.
    pub fn with_label(&mut self, label: impl Into<String>, value: impl Into<String>) -> &mut Self {
        self.labels.insert(label.into(), value.into());
        self
    }
}
