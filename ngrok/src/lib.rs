//! The ngrok agent SDK.

#![warn(missing_docs)]

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

mod session;
mod tunnel;

pub use session::*;
pub use tunnel::*;
