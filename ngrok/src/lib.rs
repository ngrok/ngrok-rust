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
    // TODO: remove this once all of the config structs are fully fleshed out
    //       and tested.
    #![allow(dead_code)]

    #[macro_use]
    mod common;
    pub use common::*;

    mod headers;
    mod http;
    pub use http::*;
    mod labeled;
    pub use labeled::*;
    mod oauth;
    pub use oauth::*;
    mod oidc;
    pub use oidc::*;
    mod tcp;
    pub use tcp::*;
    mod tls;
    pub use tls::*;
    mod webhook_verification;
}

/// Types for working with the ngrok session.
pub mod session;
/// Types for working with ngrok tunnels.
pub mod tunnel;

mod tunnel_ext;

#[doc(inline)]
pub use session::Session;
#[doc(inline)]
pub use tunnel::{
    Conn,
    Tunnel,
};

/// A prelude of traits for working with ngrok types.
pub mod prelude {
    #[doc(inline)]
    pub use crate::{
        config::TunnelBuilder,
        internals::proto::NgrokError,
        tunnel::{
            LabelsTunnel,
            ProtoTunnel,
            Tunnel,
            UrlTunnel,
        },
        tunnel_ext::TunnelExt,
    };
}

#[cfg(all(test, feature = "online-tests"))]
mod online_tests;
