/// An RPC request from the ngrok service.
///
/// RPC requests are dispatched to the handler registered with
/// [`AgentBuilder::rpc_handler`](crate::AgentBuilder::rpc_handler).
pub trait RpcRequest: Send + Sync {
    /// Returns the method name of the RPC request (e.g. "StopAgent").
    fn method(&self) -> &str;
}

/// Method constant for the "stop agent" RPC.
pub const STOP_AGENT_METHOD: &str = "StopAgent";
/// Method constant for the "restart agent" RPC.
pub const RESTART_AGENT_METHOD: &str = "RestartAgent";
/// Method constant for the "update agent" RPC.
pub const UPDATE_AGENT_METHOD: &str = "UpdateAgent";
