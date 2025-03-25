use std::{
    collections::HashMap,
    error,
    fmt,
    io,
    ops::{
        Deref,
        DerefMut,
    },
    str::FromStr,
    string::FromUtf8Error,
    sync::Arc,
};

use muxado::typed::StreamType;
use serde::{
    de::{
        DeserializeOwned,
        Visitor,
    },
    Deserialize,
    Serialize,
    Serializer,
};
use thiserror::Error;
use tokio::io::{
    AsyncRead,
    AsyncReadExt,
};
use tracing::debug;

pub const AUTH_REQ: StreamType = StreamType::clamp(0);
pub const BIND_REQ: StreamType = StreamType::clamp(1);
pub const UNBIND_REQ: StreamType = StreamType::clamp(2);
pub const PROXY_REQ: StreamType = StreamType::clamp(3);
pub const RESTART_REQ: StreamType = StreamType::clamp(4);
pub const STOP_REQ: StreamType = StreamType::clamp(5);
pub const UPDATE_REQ: StreamType = StreamType::clamp(6);
pub const BIND_LABELED_REQ: StreamType = StreamType::clamp(7);
pub const SRV_INFO_REQ: StreamType = StreamType::clamp(8);
pub const STOP_TUNNEL_REQ: StreamType = StreamType::clamp(9);

pub const VERSION: &[&str] = &["3", "2"]; // integers in priority order

/// An error that may have an ngrok error code.
/// All ngrok error codes are documented at https://ngrok.com/docs/errors
pub trait Error: error::Error {
    /// Return the ngrok error code, if one exists for this error.
    fn error_code(&self) -> Option<&str> {
        None
    }
    /// Return the error message minus the ngrok error code.
    /// If this error has no error code, this is equivalent to
    /// `format!("{error}")`.
    fn msg(&self) -> String {
        format!("{self}")
    }
}

impl<E> Error for Box<E>
where
    E: Error,
{
    fn error_code(&self) -> Option<&str> {
        <E as Error>::error_code(self)
    }
    fn msg(&self) -> String {
        <E as Error>::msg(self)
    }
}

impl<E> Error for Arc<E>
where
    E: Error,
{
    fn error_code(&self) -> Option<&str> {
        <E as Error>::error_code(self)
    }
    fn msg(&self) -> String {
        <E as Error>::msg(self)
    }
}

impl<E> Error for &E
where
    E: Error,
{
    fn error_code(&self) -> Option<&str> {
        <E as Error>::error_code(self)
    }
    fn msg(&self) -> String {
        <E as Error>::msg(self)
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct ErrResp {
    pub msg: String,
    pub error_code: Option<String>,
}

impl<'a> From<&'a str> for ErrResp {
    fn from(value: &'a str) -> Self {
        let mut error_code = None;
        let mut msg_lines = vec![];
        for line in value.lines().filter(|l| !l.is_empty()) {
            if line.starts_with("ERR_NGROK_") {
                error_code = Some(line.trim().into());
            } else {
                msg_lines.push(line);
            }
        }
        ErrResp {
            error_code,
            msg: msg_lines.join("\n"),
        }
    }
}

impl error::Error for ErrResp {}

const ERR_URL: &str = "https://ngrok.com/docs/errors";

impl fmt::Display for ErrResp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.msg.fmt(f)?;
        if let Some(code) = self.error_code.as_ref().map(|s| s.to_lowercase()) {
            write!(f, "\n\n{ERR_URL}/{code}")?;
        }
        Ok(())
    }
}

impl Error for ErrResp {
    fn error_code(&self) -> Option<&str> {
        self.error_code.as_deref()
    }
    fn msg(&self) -> String {
        self.msg.clone()
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "PascalCase")]
pub struct Auth {
    pub version: Vec<String>, // protocol versions supported, ordered by preference
    pub client_id: String,    // empty for new sessions
    pub extra: AuthExtra,     // clients may add whatever data the like to auth messages
}

#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize, Default)]
#[serde(transparent)]
pub struct SecretBytes(#[serde(with = "base64bytes")] Vec<u8>);

impl Deref for SecretBytes {
    type Target = Vec<u8>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for SecretBytes {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<'a> From<&'a [u8]> for SecretBytes {
    fn from(other: &'a [u8]) -> Self {
        SecretBytes(other.into())
    }
}

impl From<Vec<u8>> for SecretBytes {
    fn from(other: Vec<u8>) -> Self {
        SecretBytes(other)
    }
}

impl fmt::Display for SecretBytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "********")
    }
}

impl fmt::Debug for SecretBytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "********")
    }
}

#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize, Default)]
#[serde(transparent)]
pub struct SecretString(String);

impl Deref for SecretString {
    type Target = String;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for SecretString {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<'a> From<&'a str> for SecretString {
    fn from(other: &'a str) -> Self {
        SecretString(other.into())
    }
}

impl From<String> for SecretString {
    fn from(other: String) -> Self {
        SecretString(other)
    }
}

impl fmt::Display for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "********")
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "********")
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "PascalCase")]
pub struct AuthExtra {
    #[serde(rename = "OS")]
    pub os: String,
    pub arch: String,
    pub auth_token: SecretString,
    pub version: String,
    pub hostname: String,
    pub user_agent: String,
    pub metadata: String,
    pub cookie: SecretString,
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
    pub cookie: Option<SecretString>,
    pub account_name: Option<String>,
    pub session_duration: Option<i64>,
    pub plan_name: Option<String>,
    pub banner: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct Bind<T> {
    #[serde(rename = "Id")]
    pub client_id: String,
    pub proto: String,
    pub forwards_to: String,
    pub forwards_proto: String,
    pub opts: T,
    pub extra: BindExtra,
}

#[derive(Debug, Clone)]
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
    pub token: SecretString,
    #[serde(rename = "IPPolicyRef")]
    pub ip_policy_ref: String,
    pub metadata: String,
    pub bindings: Vec<String>,
    #[serde(rename = "PoolingEnabled")]
    pub pooling_enabled: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct BindResp<T> {
    #[serde(rename = "Id")]
    pub client_id: String,
    #[serde(rename = "URL")]
    pub url: String,
    pub proto: String,
    #[serde(rename = "Opts")]
    pub bind_opts: T,
    pub extra: BindRespExtra,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct BindRespExtra {
    pub token: SecretString,
}

rpc_req!(Bind<T>, BindResp<T>, BIND_REQ; T: std::fmt::Debug + Serialize + DeserializeOwned + Clone);

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct StartTunnelWithLabel {
    pub labels: HashMap<String, String>,
    pub forwards_to: String,
    pub forwards_proto: String,
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
#[non_exhaustive]
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

/// The edge type for an incomming connection.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum EdgeType {
    /// EdgeType Undefined
    Undefined,
    /// A TCP Edge
    Tcp,
    /// A TLS Edge
    Tls,
    /// A HTTPs Edge
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
    pub(crate) fn as_str(self) -> &'static str {
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

/// A request from the ngrok dashboard for the agent to stop.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, Default)]
#[serde(rename_all = "PascalCase")]
pub struct Stop {}

/// Common response structure for all remote commands originating from the ngrok
/// dashboard.
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "PascalCase")]
pub struct CommandResp {
    /// The error arising from command handling, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub type StopResp = CommandResp;

rpc_req!(Stop, StopResp, STOP_REQ);

/// A request from the ngrok dashboard for the agent to restart.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, Default)]
#[serde(rename_all = "PascalCase")]
pub struct Restart {}

pub type RestartResp = CommandResp;
rpc_req!(Restart, RestartResp, RESTART_REQ);

/// A request from the ngrok dashboard for the agent to update itself.
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "PascalCase")]
pub struct Update {
    /// The version that the agent is requested to update to.
    pub version: String,
    /// Whether or not updating to the same major version is sufficient.
    pub permit_major_version: bool,
}

/// A request from remote to stop a tunnel
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "PascalCase")]
pub struct StopTunnel {
    /// The id of the tunnel to stop
    #[serde(rename = "Id")]
    pub client_id: String,
    /// The message on why this tunnel was stopped
    pub message: String,
    /// An optional ngrok error code
    pub error_code: Option<String>,
}

pub type UpdateResp = CommandResp;
rpc_req!(Update, UpdateResp, UPDATE_REQ);

#[derive(Serialize, Deserialize, Debug, Clone, Copy, Default)]
#[serde(rename_all = "PascalCase")]
pub struct SrvInfo {}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "PascalCase")]
pub struct SrvInfoResp {
    pub region: String,
}

rpc_req!(SrvInfo, SrvInfoResp, SRV_INFO_REQ);

/// The version of [PROXY protocol](https://www.haproxy.org/download/1.8/doc/proxy-protocol.txt)
/// to use with this tunnel.
///
/// [ProxyProto::None] disables PROXY protocol support.
#[derive(Debug, Copy, Clone, Default, Eq, PartialEq)]
pub enum ProxyProto {
    /// No PROXY protocol
    #[default]
    None,
    /// PROXY protocol v1
    V1,
    /// PROXY protocol v2
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

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(untagged)]
pub enum PolicyWrapper {
    #[serde(serialize_with = "serialize_policy")]
    Policy(Policy),
    String(String),
}

impl From<String> for PolicyWrapper {
    fn from(value: String) -> Self {
        PolicyWrapper::String(value)
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "PascalCase")]
pub struct HttpEndpoint {
    pub hostname: String,
    pub auth: String,
    pub subdomain: String,
    pub host_header_rewrite: bool,
    pub local_url_scheme: Option<String>,
    pub proxy_proto: ProxyProto,

    pub compression: Option<Compression>,
    pub circuit_breaker: Option<CircuitBreaker>,
    #[serde(rename = "IPRestriction")]
    pub ip_restriction: Option<IpRestriction>,
    pub basic_auth: Option<BasicAuth>,
    #[serde(rename = "OAuth")]
    pub oauth: Option<Oauth>,
    #[serde(rename = "OIDC")]
    pub oidc: Option<Oidc>,
    pub webhook_verification: Option<WebhookVerification>,
    #[serde(rename = "MutualTLSCA")]
    pub mutual_tls_ca: Option<MutualTls>,
    #[serde(default)]
    pub request_headers: Option<Headers>,
    #[serde(default)]
    pub response_headers: Option<Headers>,
    #[serde(rename = "WebsocketTCPConverter")]
    pub websocket_tcp_converter: Option<WebsocketTcpConverter>,
    #[serde(rename = "UserAgentFilter")]
    pub user_agent_filter: Option<UserAgentFilter>,
    #[serde(rename = "TrafficPolicy")]
    pub traffic_policy: Option<PolicyWrapper>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Compression {}

fn is_default<T>(v: &T) -> bool
where
    T: PartialEq<T> + Default,
{
    T::default() == *v
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct CircuitBreaker {
    #[serde(default, skip_serializing_if = "is_default")]
    pub error_threshold: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BasicAuth {
    #[serde(default, skip_serializing_if = "is_default")]
    pub credentials: Vec<BasicAuthCredential>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub struct BasicAuthCredential {
    pub username: String,
    #[serde(default, skip_serializing_if = "is_default")]
    pub cleartext_password: String,
    #[serde(default, skip_serializing_if = "is_default")]
    #[serde(with = "base64bytes")]
    pub hashed_password: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpRestriction {
    #[serde(default, skip_serializing_if = "is_default")]
    pub allow_cidrs: Vec<String>,
    #[serde(default, skip_serializing_if = "is_default")]
    pub deny_cidrs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Oauth {
    pub provider: String,
    #[serde(default, skip_serializing_if = "is_default")]
    pub client_id: String,
    #[serde(default, skip_serializing_if = "is_default")]
    pub client_secret: SecretString,
    #[serde(default, skip_serializing_if = "is_default")]
    #[serde(with = "base64bytes")]
    pub sealed_client_secret: Vec<u8>,
    #[serde(default, skip_serializing_if = "is_default")]
    pub allow_emails: Vec<String>,
    #[serde(default, skip_serializing_if = "is_default")]
    pub allow_domains: Vec<String>,
    #[serde(default, skip_serializing_if = "is_default")]
    pub scopes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Oidc {
    pub issuer_url: String,
    #[serde(default, skip_serializing_if = "is_default")]
    pub client_id: String,
    #[serde(default, skip_serializing_if = "is_default")]
    pub client_secret: SecretString,
    #[serde(default, skip_serializing_if = "is_default")]
    #[serde(with = "base64bytes")]
    pub sealed_client_secret: Vec<u8>,
    #[serde(default, skip_serializing_if = "is_default")]
    pub allow_emails: Vec<String>,
    #[serde(default, skip_serializing_if = "is_default")]
    pub allow_domains: Vec<String>,
    #[serde(default, skip_serializing_if = "is_default")]
    pub scopes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookVerification {
    pub provider: String,
    #[serde(default, skip_serializing_if = "is_default")]
    pub secret: SecretString,
    #[serde(default, skip_serializing_if = "is_default")]
    #[serde(with = "base64bytes")]
    pub sealed_secret: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MutualTls {
    #[serde(default, skip_serializing_if = "is_default")]
    #[serde(with = "base64bytes")]
    // this is snake-case on the wire
    pub mutual_tls_ca: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Headers {
    #[serde(default, skip_serializing_if = "is_default")]
    pub add: Vec<String>,
    #[serde(default, skip_serializing_if = "is_default")]
    pub remove: Vec<String>,
    #[serde(default, skip_serializing_if = "is_default")]
    pub add_parsed: HashMap<String, String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct WebsocketTcpConverter {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserAgentFilter {
    #[serde(default, skip_serializing_if = "is_default")]
    pub allow: Vec<String>,
    #[serde(default, skip_serializing_if = "is_default")]
    pub deny: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "PascalCase")]
pub struct TcpEndpoint {
    pub addr: String,
    pub proxy_proto: ProxyProto,
    #[serde(rename = "IPRestriction")]
    pub ip_restriction: Option<IpRestriction>,
    #[serde(rename = "TrafficPolicy")]
    pub traffic_policy: Option<PolicyWrapper>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "PascalCase")]
pub struct TlsEndpoint {
    pub hostname: String,
    pub subdomain: String,
    pub proxy_proto: ProxyProto,
    #[serde(rename = "MutualTLSAtAgent")]
    pub mutual_tls_at_agent: bool,

    #[serde(rename = "MutualTLSAtEdge")]
    pub mutual_tls_at_edge: Option<MutualTls>,
    #[serde(rename = "TLSTermination")]
    pub tls_termination: Option<TlsTermination>,
    #[serde(rename = "IPRestriction")]
    pub ip_restriction: Option<IpRestriction>,
    #[serde(rename = "TrafficPolicy")]
    pub traffic_policy: Option<PolicyWrapper>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct TlsTermination {
    #[serde(default, with = "base64bytes", skip_serializing_if = "is_default")]
    pub cert: Vec<u8>,
    #[serde(skip_serializing_if = "is_default", default)]
    pub key: SecretBytes,
    #[serde(default, with = "base64bytes", skip_serializing_if = "is_default")]
    pub sealed_key: Vec<u8>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "PascalCase")]
pub struct LabelEndpoint {
    pub labels: HashMap<String, String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "PascalCase", default)]
pub struct Policy {
    pub inbound: Vec<Rule>,
    pub outbound: Vec<Rule>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "PascalCase", default)]
pub struct Rule {
    pub name: String,
    pub expressions: Vec<String>,
    pub actions: Vec<Action>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "PascalCase", default)]
pub struct Action {
    #[serde(rename = "Type")]
    pub type_: String,
    #[serde(default, with = "vec_to_json", skip_serializing_if = "is_default")]
    pub config: Vec<u8>,
}

// This function converts a Policy into a valid JSON string. This is used so legacy configurations will still work
// using the new string "TrafficPolicy" field.
fn serialize_policy<S: Serializer>(v: &Policy, s: S) -> Result<S::Ok, S::Error> {
    let abc = match serde_json::to_string(v) {
        Ok(t) => t,
        Err(_) => {
            return Err(serde::ser::Error::custom(
                "policy could not be converted to valid json",
            ))
        }
    };
    s.serialize_str(&abc)
}

// These are helpers to convert base64 strings to full, real json. The serialize helper also ensures that the resulting
// representation isn't a string-escaped string.
mod vec_to_json {
    use serde::{
        Deserialize,
        Deserializer,
        Serialize,
        Serializer,
    };

    pub fn serialize<S: Serializer>(v: &[u8], s: S) -> Result<S::Ok, S::Error> {
        let u: serde_json::Value = match serde_json::from_slice(v) {
            Ok(k) => k,
            Err(_) => return Err(serde::ser::Error::custom("Config is invalid JSON")),
        };

        u.serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = serde_json::Map::deserialize(d)?;
        let v = serde_json::to_vec(&s).unwrap();
        Ok(v)
    }
}

// These are helpers to facilitate the Vec<u8> <-> base64-encoded bytes
// representation that the Go messages use
mod base64bytes {
    use base64::prelude::*;
    use serde::{
        Deserialize,
        Deserializer,
        Serialize,
        Serializer,
    };

    pub fn serialize<S: Serializer>(v: &Vec<u8>, s: S) -> Result<S::Ok, S::Error> {
        BASE64_STANDARD.encode(v).serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(d)?;
        BASE64_STANDARD
            .decode(s.as_bytes())
            .map_err(serde::de::Error::custom)
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

    pub(crate) const POLICY_JSON: &str = r###"{"Inbound":[{"Name":"test_in","Expressions":["req.Method == 'PUT'"],"Actions":[{"Type":"deny"}]}],"Outbound":[{"Name":"test_out","Expressions":["res.StatusCode == '200'"],"Actions":[{"Type":"custom-response","Config":{"status_code":201}}]}]}"###;

    #[test]
    fn test_policy_proto_serde() {
        let policy: Policy = serde_json::from_str(POLICY_JSON).unwrap();

        // mainly just interested in checking outbound, as that has the
        // special vec serialization
        assert_eq!(1, policy.outbound.len());
        let outbound = &policy.outbound[0];
        assert_eq!(1, outbound.actions.len());
        let action = &outbound.actions[0];
        assert_eq!(r#"{"status_code":201}"#.as_bytes(), action.config);

        assert_eq!(serde_json::to_string(&policy).unwrap(), POLICY_JSON);
    }
}
