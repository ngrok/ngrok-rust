use std::time::Duration;

/// Events emitted by the [`Agent`](crate::Agent).
///
/// These events inform the application about agent lifecycle changes
/// and can be received via an event handler registered with
/// [`AgentBuilder::event_handler`](crate::AgentBuilder::event_handler).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum Event {
    /// The agent successfully connected to the ngrok service.
    AgentConnectSucceeded,
    /// The agent disconnected from the ngrok service.
    AgentDisconnected {
        /// The error that caused the disconnection, if any.
        error: Option<String>,
    },
    /// A heartbeat response was received from the ngrok service.
    AgentHeartbeatReceived {
        /// The round-trip latency of the heartbeat.
        latency: Duration,
    },
}
