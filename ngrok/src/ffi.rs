//! FFI support types for the ngrok agent SDK.
//!
//! This module provides concrete, type-erased types suitable for use across
//! FFI boundaries. It is gated behind the `ffi` feature flag.

use crate::{
    agent::Agent,
    endpoint::{
        EndpointForwarder,
        EndpointListener,
    },
    internals::raw_session::RpcError,
    upstream::Upstream,
};

/// Concrete, FFI-safe Agent — all generics erased.
pub struct FfiAgent {
    inner: Agent,
}

impl FfiAgent {
    /// Create a new FfiAgent wrapping an Agent.
    pub fn new(agent: Agent) -> Self {
        FfiAgent { inner: agent }
    }

    /// Connect the agent to the ngrok service.
    pub async fn connect(&self) -> Result<(), FfiError> {
        self.inner.connect().await.map_err(FfiError::from)
    }

    /// Disconnect the agent from the ngrok service.
    pub async fn disconnect(&self) -> Result<(), FfiError> {
        self.inner.disconnect().await.map_err(FfiError::from)
    }

    /// Listen on an endpoint with the given options.
    pub async fn listen(
        &self,
        opts: FfiEndpointOptions,
    ) -> Result<EndpointListener, FfiError> {
        let mut builder = self.inner.listen();
        if let Some(ref url) = opts.url {
            builder = builder.url(url);
        }
        if let Some(ref tp) = opts.traffic_policy {
            builder = builder.traffic_policy(tp);
        }
        if let Some(ref meta) = opts.metadata {
            builder = builder.metadata(meta);
        }
        if let Some(ref desc) = opts.description {
            builder = builder.description(desc);
        }
        if !opts.bindings.is_empty() {
            builder = builder.bindings(opts.bindings.iter().map(|s| s.as_str()));
        }
        if let Some(pooling) = opts.pooling_enabled {
            builder = builder.pooling_enabled(pooling);
        }
        builder.start().await.map_err(FfiError::from)
    }

    /// Forward to an upstream with the given options.
    pub async fn forward(
        &self,
        upstream: FfiUpstream,
        opts: FfiEndpointOptions,
    ) -> Result<EndpointForwarder, FfiError> {
        let mut us = Upstream::new(upstream.addr);
        if let Some(ref proto) = upstream.protocol {
            us = us.protocol(proto);
        }
        if !upstream.verify_upstream_tls {
            us = us.verify_upstream_tls(false);
        }

        let mut builder = self.inner.forward(us);
        if let Some(ref url) = opts.url {
            builder = builder.url(url);
        }
        if let Some(ref tp) = opts.traffic_policy {
            builder = builder.traffic_policy(tp);
        }
        if let Some(ref meta) = opts.metadata {
            builder = builder.metadata(meta);
        }
        if let Some(ref desc) = opts.description {
            builder = builder.description(desc);
        }
        if !opts.bindings.is_empty() {
            builder = builder.bindings(opts.bindings.iter().map(|s| s.as_str()));
        }
        if let Some(pooling) = opts.pooling_enabled {
            builder = builder.pooling_enabled(pooling);
        }
        builder.start().await.map_err(FfiError::from)
    }
}

/// Concrete EndpointOptions with no type parameters.
#[derive(Clone, Debug, Default)]
pub struct FfiEndpointOptions {
    /// The endpoint URL (protocol inferred from scheme).
    pub url: Option<String>,
    /// Traffic policy as YAML/JSON string.
    pub traffic_policy: Option<String>,
    /// Opaque metadata.
    pub metadata: Option<String>,
    /// Human-readable description.
    pub description: Option<String>,
    /// Binding configuration.
    pub bindings: Vec<String>,
    /// Whether endpoint pooling is enabled.
    pub pooling_enabled: Option<bool>,
}

/// Concrete Upstream with no type parameters.
#[derive(Clone, Debug)]
pub struct FfiUpstream {
    /// The upstream address (host:port or URL).
    pub addr: String,
    /// The application protocol.
    pub protocol: Option<String>,
    /// Whether to verify upstream TLS certificates.
    pub verify_upstream_tls: bool,
}

impl Default for FfiUpstream {
    fn default() -> Self {
        FfiUpstream {
            addr: String::new(),
            protocol: None,
            verify_upstream_tls: true,
        }
    }
}

/// Error type that serializes cleanly across FFI boundaries.
#[derive(Debug, Clone)]
pub struct FfiError {
    /// The ngrok error code, if any.
    pub code: Option<String>,
    /// The error message.
    pub message: String,
}

impl std::fmt::Display for FfiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(ref code) = self.code {
            write!(f, "[{}] {}", code, self.message)
        } else {
            write!(f, "{}", self.message)
        }
    }
}

impl std::error::Error for FfiError {}

impl From<RpcError> for FfiError {
    fn from(err: RpcError) -> Self {
        use crate::internals::proto::Error;
        FfiError {
            code: err.error_code().map(|s| s.to_string()),
            message: err.msg(),
        }
    }
}
