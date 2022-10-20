use muxado::typed::StreamType;
use serde::{
    de::DeserializeOwned,
    Serialize,
};

pub trait RPCRequest: Serialize {
    type Response: DeserializeOwned;
    const TYPE: StreamType;
}

macro_rules! rpc_req {
    ($req:ty, $resp:ty, $typ:expr) => {
        impl $crate::rpc::RPCRequest for $req {
            type Response = $resp;
            const TYPE: StreamType = $typ;
        }
    };
}
