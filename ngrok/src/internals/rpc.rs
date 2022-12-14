use std::fmt::Debug;

use muxado::typed::StreamType;
use serde::{
    de::DeserializeOwned,
    Deserialize,
    Serialize,
};

use super::raw_session::RpcError;

pub trait RpcRequest: Serialize + Debug {
    type Response: DeserializeOwned + Debug;
    const TYPE: StreamType;
}

#[derive(Deserialize, Serialize, Debug)]
pub struct RpcResult<T> {
    #[serde(flatten)]
    ok: T,
    #[serde(rename = "Error")]
    error: String,
}

impl<T> From<RpcResult<T>> for Result<T, RpcError> {
    fn from(res: RpcResult<T>) -> Self {
        if res.error.is_empty() {
            Ok(res.ok)
        } else {
            Err(RpcError::Response(res.error))
        }
    }
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
