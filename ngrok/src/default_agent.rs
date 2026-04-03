use once_cell::sync::Lazy;
use tokio::sync::Mutex;

use crate::{
    agent::Agent,
    endpoint_builder::{
        EndpointForwardBuilder,
        EndpointListenBuilder,
    },
    upstream::Upstream,
};

static DEFAULT_AGENT: Lazy<Mutex<Option<Agent>>> = Lazy::new(|| Mutex::new(None));

/// Returns the global default agent.
///
/// If no default agent has been set, one is initialized with the
/// `NGROK_AUTHTOKEN` environment variable. The agent will connect lazily
/// on the first `listen()` or `forward()` call.
///
/// # Examples
///
/// ```no_run
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let agent = ngrok::default_agent();
/// let listener = agent.listen().start().await?;
/// # Ok(())
/// # }
/// ```
pub fn default_agent() -> Agent {
    // We can't use async here, so we try a non-blocking approach
    // The agent is lazily initialized but build() is sync
    let guard = DEFAULT_AGENT.blocking_lock();
    if let Some(ref agent) = *guard {
        return agent.clone();
    }
    drop(guard);

    let agent = Agent::builder()
        .authtoken_from_env()
        .build()
        .expect("failed to build default agent");

    let mut guard = DEFAULT_AGENT.blocking_lock();
    if guard.is_none() {
        *guard = Some(agent.clone());
    }
    guard.as_ref().unwrap().clone()
}

/// Returns the global default agent (async version).
///
/// Same as [`default_agent()`] but safe to call from async contexts.
pub async fn default_agent_async() -> Agent {
    let guard = DEFAULT_AGENT.lock().await;
    if let Some(ref agent) = *guard {
        return agent.clone();
    }
    drop(guard);

    let agent = Agent::builder()
        .authtoken_from_env()
        .build()
        .expect("failed to build default agent");

    let mut guard = DEFAULT_AGENT.lock().await;
    if guard.is_none() {
        *guard = Some(agent.clone());
    }
    guard.as_ref().unwrap().clone()
}

/// Replaces the global default agent.
///
/// Use this for test isolation or when running multiple agent configurations.
pub async fn set_default_agent(agent: Agent) {
    let mut guard = DEFAULT_AGENT.lock().await;
    *guard = Some(agent);
}

/// Resets the global default agent to uninitialized state.
pub async fn reset_default_agent() {
    let mut guard = DEFAULT_AGENT.lock().await;
    *guard = None;
}

/// Top-level convenience: listen using the default agent.
///
/// Creates an [`EndpointListenBuilder`] using the global default agent.
/// Configure the endpoint with the fluent API, then call `.start()`.
///
/// # Examples
///
/// ```no_run
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// // Random HTTPS endpoint
/// let listener = ngrok::listen().start().await?;
///
/// // With a specific domain
/// let listener = ngrok::listen()
///     .url("https://app.ngrok.app")
///     .start()
///     .await?;
///
/// // TCP endpoint
/// let listener = ngrok::listen()
///     .url("tcp://1.tcp.ngrok.io:12345")
///     .start()
///     .await?;
/// # Ok(())
/// # }
/// ```
pub fn listen() -> EndpointListenBuilder {
    let agent = default_agent();
    agent.listen()
}

/// Top-level convenience: forward using the default agent.
///
/// Creates an [`EndpointForwardBuilder`] using the global default agent.
///
/// # Examples
///
/// ```no_run
/// use ngrok::Upstream;
///
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let fwd = ngrok::forward(Upstream::new("localhost:8080"))
///     .url("https://app.ngrok.app")
///     .start()
///     .await?;
/// # Ok(())
/// # }
/// ```
pub fn forward(upstream: Upstream) -> EndpointForwardBuilder {
    let agent = default_agent();
    agent.forward(upstream)
}
