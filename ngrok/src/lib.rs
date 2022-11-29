pub mod mw {
    include!(concat!(env!("OUT_DIR"), "/agent.rs"));
}

mod internals {
    #[macro_use]
    pub mod rpc;
    pub mod proto;
    pub mod raw_session;
}

mod config {
    // TODO: remove this once all of the config structs are fully fleshed out
    //       and tested.
    #![allow(dead_code)]

    pub mod common;
    pub mod headers;
    pub mod http;
    pub mod labeled;
    pub mod oauth;
    pub mod oidc;
    pub mod tcp;
    pub mod tls;
    pub mod webhook_verification;
}

mod session;
mod tunnel;

pub use config::{
    http::*,
    labeled::*,
    tcp::*,
    tls::*,
    *,
};
pub use session::*;
pub use tunnel::*;
