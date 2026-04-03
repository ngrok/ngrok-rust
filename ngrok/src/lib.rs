#![doc = include_str!("../README.md")]
#![warn(missing_docs)]
#![cfg_attr(docsrs, feature(doc_cfg))]

mod internals {
    #[macro_use]
    pub mod rpc;
    pub mod proto;
    pub mod raw_session;
}

/// Tunnel and endpoint configuration types.
pub mod config {
    #[macro_use]
    mod common;
    pub use common::*;

    mod headers;
    mod http;
    pub use self::http::*;
    mod labeled;
    pub use labeled::*;
    mod oauth;
    pub use oauth::*;
    mod oidc;
    pub use policies::*;
    mod policies;
    pub use oidc::*;
    mod tcp;
    pub use tcp::*;
    mod tls;
    pub use tls::*;
    mod webhook_verification;
}

mod proxy_proto;

/// Types for working with the ngrok session.
pub mod session;
/// Types for working with ngrok tunnels.
pub mod tunnel;

/// Types for working with ngrok connections.
pub mod conn;

/// Types for working with connection forwarders.
pub mod forwarder;
mod tunnel_ext;

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

// ===== v1 re-exports (kept for backward compatibility) =====
#[doc(inline)]
pub use conn::{
    Conn,
    EdgeConn,
    EndpointConn,
};
#[doc(inline)]
pub use internals::proto::Error;
#[doc(inline)]
pub use session::Session;
#[doc(inline)]
pub use tunnel::Tunnel;

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

/// A prelude of traits for working with ngrok types.
pub mod prelude {
    #[allow(deprecated)]
    #[doc(inline)]
    pub use crate::{
        config::{
            Action,
            ForwarderBuilder,
            HttpTunnelBuilder,
            InvalidPolicy,
            LabeledTunnelBuilder,
            OauthOptions,
            OidcOptions,
            Policy,
            ProxyProto,
            Rule,
            Scheme,
            TcpTunnelBuilder,
            TlsTunnelBuilder,
            TunnelBuilder,
        },
        conn::{
            Conn,
            ConnInfo,
            EdgeConnInfo,
            EndpointConnInfo,
        },
        internals::proto::EdgeType,
        internals::proto::Error,
        tunnel::{
            EdgeInfo,
            EndpointInfo,
            Tunnel,
            TunnelCloser,
            TunnelInfo,
        },
        tunnel_ext::TunnelExt,
    };

    // v2 API re-exports in prelude
    pub use crate::{
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
