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
    pub mod http;
    pub mod labeled;
    pub mod tcp;
    pub mod tls;
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
