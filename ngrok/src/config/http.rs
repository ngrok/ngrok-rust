use std::collections::HashMap;

use crate::{common::{CommonOpts, TunnelConfig, private, FORWARDS_TO}, internals::proto::{BindExtra, BindOpts, self}};

#[derive(Clone, Eq, PartialEq)]
pub enum Scheme {
    HTTP,
    HTTPS,
}

pub struct HTTPEndpoint {
    common_opts: CommonOpts,
    scheme: Option<Scheme>,
    hostname: Option<String>,
    basic_auth: Option<(String, String)>
}

impl TunnelConfig for HTTPEndpoint {}

impl Default for HTTPEndpoint {
    fn default() -> Self {
        HTTPEndpoint {
            common_opts: CommonOpts::default(),
            scheme: None,
            hostname: None,
            basic_auth: None,
        }
    }
}

impl private::TunnelConfigPrivate for HTTPEndpoint {
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
    fn proto(&self) -> String {
        let scheme = self.scheme.clone().unwrap_or(Scheme::HTTPS);
        if scheme == Scheme::HTTP {
            return "http".into();
        }
        "https".into()
    }
    fn opts(&self) -> Option<BindOpts> {
        let http_endpoint = proto::HTTPEndpoint::default();
        // todo: fill out all the options here? or use the proto as our storage?
        Some(BindOpts::HTTPEndpoint(http_endpoint))
    }
    fn labels(&self) -> HashMap<String,String> {
        return HashMap::new();
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
	pub fn with_basic_auth(&mut self, username: impl Into<String>, password: impl Into<String>) -> &mut Self {
        self.basic_auth = Some((username.into(), password.into()));
        self
    }

	// todo
}