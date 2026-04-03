#![doc = include_str!("../README.md")]
#![warn(missing_docs)]
#![cfg_attr(docsrs, feature(doc_cfg))]

#[allow(dead_code)]
mod internals {
    #[macro_use]
    pub mod rpc;
    pub mod proto;
    pub mod raw_session;
}

/// Tunnel and endpoint configuration types (internal).
///
/// The v2 API uses [`EndpointOptions`] and [`EndpointListenBuilder`] instead of
/// per-protocol builders. These types are kept for internal use.
#[allow(dead_code)]
pub(crate) mod config {
    #[macro_use]
    mod common;
    pub(crate) use common::*;

    mod headers;
    mod http;
    pub(crate) use self::http::*;
    mod labeled;
    pub(crate) use labeled::*;
    mod oauth;
    #[cfg(test)]
    pub(crate) use oauth::*;
    mod oidc;
    pub(crate) use policies::*;
    mod policies;
    mod tcp;
    pub(crate) use tcp::*;
    mod tls;
    pub(crate) use tls::*;
    mod webhook_verification;
}

mod proxy_proto;

/// Types for working with the ngrok session (internal).
#[allow(dead_code)]
pub(crate) mod session;
/// Types for working with ngrok tunnels (internal).
#[allow(dead_code)]
pub(crate) mod tunnel;

/// Types for working with ngrok connections.
pub mod conn;

/// Types for working with connection forwarders (internal).
#[allow(dead_code)]
pub(crate) mod forwarder;
#[allow(dead_code)]
pub(crate) mod tunnel_ext;

// ===== v2 API modules =====
/// Agent configuration and management (v2 API).
pub mod agent;
/// Endpoint types — listeners and forwarders (v2 API).
pub mod endpoint;
/// Endpoint configuration builders (v2 API).
pub mod endpoint_builder;
/// Upstream configuration (v2 API).
pub mod upstream;
/// Agent events (v2 API).
pub mod event;
/// RPC handler types (v2 API).
pub mod rpc_handler;
/// Default agent and top-level convenience functions (v2 API).
pub mod default_agent;

/// FFI support types (v2 API).
#[cfg(feature = "ffi")]
#[cfg_attr(docsrs, doc(cfg(feature = "ffi")))]
pub mod ffi;

// ===== Connection types (kept public) =====
#[doc(inline)]
pub use conn::{
    Conn,
    ConnInfo,
    EndpointConn,
    EndpointConnInfo,
};

// ===== v2 re-exports =====
#[doc(inline)]
pub use agent::{
    Agent,
    AgentBuilder,
    AgentSession,
};
#[doc(inline)]
pub use default_agent::{
    default_agent,
    forward,
    listen,
};
#[doc(inline)]
pub use endpoint::{
    Endpoint,
    EndpointForwarder,
    EndpointListener,
};
#[doc(inline)]
pub use endpoint_builder::{
    EndpointForwardBuilder,
    EndpointListenBuilder,
    EndpointOptions,
};
#[doc(inline)]
pub use event::Event;
#[doc(inline)]
pub use rpc_handler::{
    RpcRequest,
    RESTART_AGENT_METHOD,
    STOP_AGENT_METHOD,
    UPDATE_AGENT_METHOD,
};
#[doc(inline)]
pub use upstream::Upstream;

/// A prelude of traits and types for working with ngrok.
pub mod prelude {
    pub use crate::{
        conn::{
            Conn,
            ConnInfo,
            EndpointConnInfo,
        },
        Agent,
        AgentBuilder,
        Endpoint,
        EndpointForwarder,
        EndpointListener,
        EndpointOptions,
        Upstream,
        Event,
        default_agent,
        listen,
        forward,
    };
}

#[cfg(all(test, feature = "online-tests"))]
mod online_tests;
