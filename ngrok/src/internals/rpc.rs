use std::fmt::Debug;

use muxado::typed::StreamType;
use serde::{
    de::DeserializeOwned,
    Serialize,
};

pub trait RpcRequest: Serialize + Debug {
    type Response: DeserializeOwned + Debug;
    const TYPE: StreamType;
}

macro_rules! rpc_req {
    ($req:ty, $resp:ty, $typ:expr; $($t:tt)*) => {
        impl <$($t)*> $crate::internals::rpc::RpcRequest for $req
        {
            type Response = $resp;
            const TYPE: StreamType = $typ;
        }
    };
    ($req:ty, $resp:ty, $typ:expr) => {
        impl $crate::internals::rpc::RpcRequest for $req {
            type Response = $resp;
            const TYPE: StreamType = $typ;
        }
    };
}
