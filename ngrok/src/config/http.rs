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

#[derive(Clone, Eq, PartialEq)]
pub enum Scheme {
    HTTP,
    HTTPS,
}

pub struct HTTPEndpoint {
    common_opts: CommonOpts,
    scheme: Scheme,
    hostname: Option<String>,
    basic_auth: Option<(String, String)>,
}

impl Default for HTTPEndpoint {
    fn default() -> Self {
        HTTPEndpoint {
            common_opts: CommonOpts::default(),
            scheme: Scheme::HTTPS,
            hostname: None,
            basic_auth: None,
        }
    }
}

impl private::TunnelConfigPrivate for HTTPEndpoint {
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
        if self.scheme == Scheme::HTTP {
            return "http".into();
        }
        "https".into()
    }
    fn opts(&self) -> Option<BindOpts> {
        // fill out all the options here, translating to proto here
        let mut http_endpoint = proto::HttpEndpoint::default();

        if let Some(proxy_proto) = self.common_opts.proxy_proto {
            http_endpoint.proxy_proto = proxy_proto;
        }

        Some(BindOpts::Http(http_endpoint))
    }
    fn labels(&self) -> HashMap<String, String> {
        HashMap::new()
    }
}

impl HTTPEndpoint {
    pub fn with_metadata(&mut self, metadata: impl Into<String>) -> &mut Self {
        self.common_opts.metadata = Some(metadata.into());
        self
    }
    pub fn with_hostname(&mut self, hostname: impl Into<String>) -> &mut Self {
        self.hostname = Some(hostname.into());
        self
    }
    pub fn with_basic_auth(
        &mut self,
        username: impl Into<String>,
        password: impl Into<String>,
    ) -> &mut Self {
        self.basic_auth = Some((username.into(), password.into()));
        self
    }

    // todo
}
