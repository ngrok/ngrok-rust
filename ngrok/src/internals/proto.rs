use std::{
    collections::HashMap,
    io,
    str::FromStr,
    string::FromUtf8Error,
};

use muxado::typed::StreamType;
use serde::{
    de::Visitor,
    Deserialize,
    Serialize,
};
use thiserror::Error;
use tokio::io::{
    AsyncRead,
    AsyncReadExt,
};
use tracing::debug;

use crate::mw::{
    HttpMiddleware,
    TcpMiddleware,
    TlsMiddleware,
};

pub const AUTH_REQ: StreamType = StreamType::clamp(0);
pub const BIND_REQ: StreamType = StreamType::clamp(1);
pub const UNBIND_REQ: StreamType = StreamType::clamp(2);
pub const PROXY_REQ: StreamType = StreamType::clamp(3);
pub const RESTART_REQ: StreamType = StreamType::clamp(4);
pub const STOP_REQ: StreamType = StreamType::clamp(5);
pub const UPDATE_REQ: StreamType = StreamType::clamp(6);
pub const BIND_LABELED_REQ: StreamType = StreamType::clamp(7);
pub const SRV_INFO_REQ: StreamType = StreamType::clamp(8);

pub const VERSION: &str = "2";

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "PascalCase")]
pub struct Auth {
    pub version: Vec<String>, // protocol versions supported, ordered by preference
    pub client_id: String,    // empty for new sessions
    pub extra: AuthExtra,     // clients may add whatever data the like to auth messages
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "PascalCase")]
pub struct AuthExtra {
    #[serde(rename = "OS")]
    pub os: String,
    pub arch: String,
    pub auth_token: String,
    pub version: String,
    pub hostname: String,
    pub user_agent: String,
    pub metadata: String,
    pub cookie: String,
    pub heartbeat_interval: i64,
    pub heartbeat_tolerance: i64,

    // for each remote operation, these variables define whether the ngrok
    // client is capable of executing that operation. each capability
    // is transmitted as a pointer to String, with the following meanings:
    //
    // null ->               operation disallow beause the ngrok agent version is too old.
    //                       this is true because older clients will never set this value
    //
    // "" (empty String)  -> the operation is supported
    //
    // non-empty String   -> the operation is not supported and this value is the  user-facing
    //                       error message describing why it is not supported
    pub update_unsupported_error: Option<String>,
    pub stop_unsupported_error: Option<String>,
    pub restart_unsupported_error: Option<String>,

    pub proxy_type: String,
    #[serde(rename = "MutualTLS")]
    pub mutual_tls: bool,
    pub service_run: bool,
    pub config_version: String,
    pub custom_interface: bool,
    #[serde(rename = "CustomCAs")]
    pub custom_cas: bool,

    pub client_type: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct AuthResp {
    pub version: String,
    pub client_id: String,
    #[serde(default)]
    pub extra: AuthRespExtra,
}

rpc_req!(Auth, AuthResp, AUTH_REQ);

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "PascalCase")]
pub struct AuthRespExtra {
    pub version: Option<String>,
    pub region: Option<String>,
    pub cookie: Option<String>,
    pub account_name: Option<String>,
    pub session_duration: Option<i64>,
    pub plan_name: Option<String>,
    pub banner: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct Bind {
    #[serde(rename = "Id")]
    pub client_id: String,
    pub proto: String,
    pub forwards_to: String,
    pub opts: BindOpts,
    pub extra: BindExtra,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "PascalCase")]
#[serde(untagged)]
// allowing this since these aren't persistent values.
#[allow(clippy::large_enum_variant)]
pub enum BindOpts {
    Http(HttpEndpoint),
    Tcp(TcpEndpoint),
    Tls(TlsEndpoint),
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "PascalCase")]
pub struct BindExtra {
    pub token: String,
    #[serde(rename = "IPPolicyRef")]
    pub ip_policy_ref: String,
    pub metadata: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct BindResp {
    #[serde(rename = "Id")]
    pub client_id: String,
    #[serde(rename = "URL")]
    pub url: String,
    pub proto: String,
    pub bind_opts: Option<BindOpts>,
    pub extra: BindRespExtra,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct BindRespExtra {
    pub token: String,
}

rpc_req!(Bind, BindResp, BIND_REQ);

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct StartTunnelWithLabel {
    pub labels: HashMap<String, String>,
    pub forwards_to: String,
    pub metadata: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct StartTunnelWithLabelResp {
    pub id: String,
}

rpc_req!(
    StartTunnelWithLabel,
    StartTunnelWithLabelResp,
    BIND_LABELED_REQ
);

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct Unbind {
    #[serde(rename = "Id")]
    pub client_id: String,
    // extra: not sure what this field actually contains
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct UnbindResp {
    // extra: not sure what this field actually contains
}

rpc_req!(Unbind, UnbindResp, UNBIND_REQ);

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct ProxyHeader {
    pub id: String,
    pub client_addr: String,
    pub proto: String,
    pub edge_type: EdgeType,
    #[serde(rename = "PassthroughTLS")]
    pub passthrough_tls: bool,
}

#[derive(Error, Debug)]
pub enum ReadHeaderError {
    #[error("error reading proxy header")]
    Io(#[from] io::Error),
    #[error("invalid utf-8 in proxy header")]
    InvalidUtf8(#[from] FromUtf8Error),
    #[error("invalid proxy header json")]
    InvalidHeader(#[from] serde_json::Error),
}

impl ProxyHeader {
    pub async fn read_from_stream(
        mut stream: impl AsyncRead + Unpin,
    ) -> Result<Self, ReadHeaderError> {
        let size = stream.read_i64_le().await?;
        let mut buf = vec![0u8; size as usize];

        stream.read_exact(&mut buf).await?;

        let header = String::from_utf8(buf)?;

        debug!(?header, "read header");

        Ok(serde_json::from_str(&header)?)
    }
}

#[derive(Copy, Clone, Debug)]
pub enum EdgeType {
    Undefined,
    Tcp,
    Tls,
    Https,
}

impl FromStr for EdgeType {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "1" => EdgeType::Tcp,
            "2" => EdgeType::Tls,
            "3" => EdgeType::Https,
            _ => EdgeType::Undefined,
        })
    }
}

impl EdgeType {
    pub fn as_str(self) -> &'static str {
        match self {
            EdgeType::Undefined => "0",
            EdgeType::Tcp => "1",
            EdgeType::Tls => "2",
            EdgeType::Https => "3",
        }
    }
}

impl Serialize for EdgeType {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

struct EdgeTypeVisitor;

impl<'de> Visitor<'de> for EdgeTypeVisitor {
    type Value = EdgeType;
    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str(r#""0", "1", "2", or "3""#)
    }

    fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(EdgeType::from_str(v).unwrap())
    }
}

impl<'de> Deserialize<'de> for EdgeType {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_str(EdgeTypeVisitor)
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, Default)]
#[serde(rename_all = "PascalCase")]
pub struct Stop;

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "PascalCase")]
pub struct StopResp {}

rpc_req!(Stop, StopResp, STOP_REQ);

#[derive(Serialize, Deserialize, Debug, Clone, Copy, Default)]
#[serde(rename_all = "PascalCase")]
pub struct Restart;

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "PascalCase")]
pub struct RestartResp {}

rpc_req!(Restart, RestartResp, RESTART_REQ);

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "PascalCase")]
pub struct Update {
    pub version: String,
    pub permit_major_version: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "PascalCase")]
pub struct UpdateResp {}

rpc_req!(Update, UpdateResp, UPDATE_REQ);

#[derive(Serialize, Deserialize, Debug, Clone, Copy, Default)]
#[serde(rename_all = "PascalCase")]
pub struct SrvInfo;

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "PascalCase")]
pub struct SrvInfoResp {
    pub region: String,
}

rpc_req!(SrvInfo, SrvInfoResp, SRV_INFO_REQ);

#[derive(Debug, Copy, Clone)]
pub enum ProxyProto {
    None,
    V1,
    V2,
}

impl From<ProxyProto> for i64 {
    fn from(other: ProxyProto) -> Self {
        use ProxyProto::*;
        match other {
            None => 0,
            V1 => 1,
            V2 => 2,
        }
    }
}

impl From<i64> for ProxyProto {
    fn from(other: i64) -> Self {
        use ProxyProto::*;
        match other {
            1 => V1,
            2 => V2,
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Error)]
#[error("invalid proxyproto string: {}", .0)]
pub struct InvalidProxyProtoString(String);

impl FromStr for ProxyProto {
    type Err = InvalidProxyProtoString;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        use ProxyProto::*;
        Ok(match s {
            "" => None,
            "1" => V1,
            "2" => V2,
            _ => return Err(InvalidProxyProtoString(s.into())),
        })
    }
}

impl Serialize for ProxyProto {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_i64(i64::from(*self))
    }
}

struct ProxyProtoVisitor;

impl<'de> Visitor<'de> for ProxyProtoVisitor {
    type Value = ProxyProto;
    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str("0, 1, or 2")
    }

    fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(ProxyProto::from(v))
    }

    fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(ProxyProto::from(v as i64))
    }
}

impl<'de> Deserialize<'de> for ProxyProto {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_i64(ProxyProtoVisitor)
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct HttpEndpoint {
    pub hostname: String,
    pub auth: String,
    pub subdomain: String,
    pub host_header_rewrite: bool,
    pub local_url_scheme: String,
    pub proxy_proto: ProxyProto,

    // must always be true, only here for serialization purposes.
    proto_middleware: bool,

    // Uses the Go byte slice json representation, which is base64
    #[serde(rename = "middleware_bytes")]
    #[serde(with = "base64proto")]
    pub middleware: HttpMiddleware,
}

impl Default for HttpEndpoint {
    fn default() -> Self {
        HttpEndpoint {
            hostname: Default::default(),
            auth: Default::default(),
            subdomain: Default::default(),
            host_header_rewrite: false,
            local_url_scheme: Default::default(),
            proxy_proto: ProxyProto::None,
            proto_middleware: true,
            middleware: Default::default(),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct TcpEndpoint {
    pub addr: String,
    pub proxy_proto: ProxyProto,

    proto_middleware: bool,

    #[serde(rename = "middleware_bytes")]
    #[serde(with = "base64proto")]
    pub middleware: TcpMiddleware,
}

impl Default for TcpEndpoint {
    fn default() -> Self {
        TcpEndpoint {
            addr: Default::default(),
            proxy_proto: ProxyProto::None,
            proto_middleware: true,
            middleware: Default::default(),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct TlsEndpoint {
    pub hostname: String,
    pub subdomain: String,
    pub proxy_proto: ProxyProto,
    pub mutual_tls_at_agent: bool,

    proto_middleware: bool,

    #[serde(rename = "middleware_bytes")]
    #[serde(with = "base64proto")]
    pub middleware: TlsMiddleware,
}

impl Default for TlsEndpoint {
    fn default() -> Self {
        TlsEndpoint {
            hostname: Default::default(),
            subdomain: Default::default(),
            proxy_proto: ProxyProto::None,
            mutual_tls_at_agent: false,
            proto_middleware: true,
            middleware: Default::default(),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "PascalCase")]
pub struct LabelEndpoint {
    pub labels: HashMap<String, String>,
}

// These are helpers to facilitate the struct <-> base64-encoded protobuf
// representation that the Go messages use
mod base64proto {
    use prost::Message;
    use serde::{
        Deserialize,
        Deserializer,
        Serialize,
        Serializer,
    };

    pub fn serialize<M: Message, S: Serializer>(v: &M, s: S) -> Result<S::Ok, S::Error> {
        let bytes = v.encode_to_vec();
        let base64 = base64::encode(bytes);
        String::serialize(&base64, s)
    }

    pub fn deserialize<'de, M: Message + Default, D: Deserializer<'de>>(
        d: D,
    ) -> Result<M, D::Error> {
        let base64 = String::deserialize(d)?;
        let bytes = base64::decode(base64.as_bytes()).map_err(serde::de::Error::custom)?;
        M::decode(bytes.as_slice()).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod test {

    use super::*;

    #[test]
    fn test_proxy_proto_serde() {
        let input = "2";

        let p: ProxyProto = serde_json::from_str(input).unwrap();

        assert!(matches!(p, ProxyProto::V2));

        assert_eq!(serde_json::to_string(&p).unwrap(), "2");
    }
}
