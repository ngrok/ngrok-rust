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

/// A prelude of traits for working with ngrok types.
pub mod prelude {
    #[allow(deprecated)]
    #[doc(inline)]
    pub use crate::{
        config::{
            Action,
            ForwarderBuilder,
            HttpTunnelBuilder,
            InvalidPolicies,
            LabeledTunnelBuilder,
            OauthOptions,
            OidcOptions,
            Policies,
            Policy,
            ProxyProto,
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
}

#[cfg(all(test, feature = "online-tests"))]
mod online_tests;
