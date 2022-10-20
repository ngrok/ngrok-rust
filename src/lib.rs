pub mod mw {
    include!(concat!(env!("OUT_DIR"), "/agent.rs"));
}

#[macro_use]
pub mod rpc;
pub mod proto;