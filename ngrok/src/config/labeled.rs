use std::collections::HashMap;

use crate::{
    config::common::{
        private::Sealed,
        CommonOpts,
        TunnelConfig,
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

impl Sealed for LabeledTunnel {}
impl TunnelConfig for LabeledTunnel {
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

#[cfg(test)]
mod test {
    use super::*;

    const METADATA: &str = "testmeta";
    const LABEL_KEY: &str = "edge";
    const LABEL_VAL: &str = "edghts_2IC6RJ6CQnuh7waciWyaGKc50Nt";

    #[test]
    fn test_interface_to_proto() {
        // pass to a function accepting the trait to avoid
        // "creates a temporary which is freed while still in use"
        tunnel_test(
            LabeledTunnel::default()
                .with_metadata(METADATA)
                .with_label(LABEL_KEY, LABEL_VAL),
        );
    }

    fn tunnel_test<C>(tunnel_cfg: C)
    where
        C: TunnelConfig,
    {
        assert_eq!(FORWARDS_TO, tunnel_cfg.forwards_to());

        let extra = tunnel_cfg.extra();
        assert_eq!(String::default(), extra.token);
        assert_eq!(METADATA, extra.metadata);
        assert_eq!(String::default(), extra.ip_policy_ref);

        assert_eq!("", tunnel_cfg.proto());

        assert!(tunnel_cfg.opts().is_none());

        let mut labels: HashMap<String, String> = HashMap::new();
        labels.insert(LABEL_KEY.into(), LABEL_VAL.into());
        assert_eq!(labels, tunnel_cfg.labels());
    }
}
