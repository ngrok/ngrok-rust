use std::collections::HashMap;

use async_trait::async_trait;

use super::TunnelBuilder;
use crate::{
    config::common::{
        default_forwards_to,
        CommonOpts,
        TunnelConfig,
    },
    internals::proto::{
        BindExtra,
        BindOpts,
    },
    session::RpcError,
    tunnel::LabeledTunnel,
    Session,
};

/// Options for labeled tunnels.
#[derive(Default, Clone)]
struct LabeledOptions {
    pub(crate) common_opts: CommonOpts,
    pub(crate) labels: HashMap<String, String>,
}

impl TunnelConfig for LabeledOptions {
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
        "".into()
    }
    fn opts(&self) -> Option<BindOpts> {
        None
    }
    fn labels(&self) -> HashMap<String, String> {
        self.labels.clone()
    }
}

impl_builder! {
    /// A builder for a labeled tunnel.
    LabeledTunnelBuilder, LabeledOptions, LabeledTunnel
}

impl LabeledTunnelBuilder {
    /// Sets the opaque metadata string for this tunnel.
    /// Viewable via the API.
    pub fn metadata(mut self, metadata: impl Into<String>) -> Self {
        self.options.common_opts.metadata = Some(metadata.into());
        self
    }

    /// Add a label, value pair for this tunnel.
    pub fn label(mut self, label: impl Into<String>, value: impl Into<String>) -> Self {
        self.options.labels.insert(label.into(), value.into());
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
            LabeledTunnelBuilder {
                session: None,
                options: Default::default(),
            }
            .metadata(METADATA)
            .label(LABEL_KEY, LABEL_VAL)
            .options,
        );
    }

    fn tunnel_test<C>(tunnel_cfg: C)
    where
        C: TunnelConfig,
    {
        assert_eq!(default_forwards_to(), tunnel_cfg.forwards_to());

        let extra = tunnel_cfg.extra();
        assert_eq!(String::default(), *extra.token);
        assert_eq!(METADATA, extra.metadata);
        assert_eq!(String::default(), extra.ip_policy_ref);

        assert_eq!("", tunnel_cfg.proto());

        assert!(tunnel_cfg.opts().is_none());

        let mut labels: HashMap<String, String> = HashMap::new();
        labels.insert(LABEL_KEY.into(), LABEL_VAL.into());
        assert_eq!(labels, tunnel_cfg.labels());
    }
}
