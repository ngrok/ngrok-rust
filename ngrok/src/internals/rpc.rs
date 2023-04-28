use std::fmt::Debug;

use serde::{
    de::DeserializeOwned,
    Serialize,
};

pub trait RpcRequest: Serialize + Debug {
    type Response: DeserializeOwned + Debug;
    const TYPE: u32;
}

macro_rules! rpc_req {
    ($req:ty, $resp:ty, $typ:expr; $($t:tt)*) => {
        impl <$($t)*> $crate::internals::rpc::RpcRequest for $req
        {
            type Response = $resp;
            const TYPE: u32 = $typ;
        }
    };
    ($req:ty, $resp:ty, $typ:expr) => {
        impl $crate::internals::rpc::RpcRequest for $req {
            type Response = $resp;
            const TYPE: u32 = $typ;
        }
    };
}
