use muxado::typed::StreamType;
use serde::{
    de::DeserializeOwned,
    Deserialize,
    Serialize,
};

pub trait RpcRequest: Serialize {
    type Response: DeserializeOwned;
    const TYPE: StreamType;
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "PascalCase")]
pub struct RemoteError {
    pub error: String,
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(untagged)]
pub enum RpcResult<T> {
    Ok(T),
    Err(RemoteError),
}

macro_rules! rpc_req {
    ($req:ty, $resp:ty, $typ:expr) => {
        impl $crate::internals::rpc::RpcRequest for $req {
            type Response = $resp;
            const TYPE: StreamType = $typ;
        }
    };
}
