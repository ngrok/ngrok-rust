use std::{
    pin::Pin,
    task::{
        Context,
        Poll,
    },
};

use async_trait::async_trait;
use futures::Stream;
use tokio::task::JoinHandle;

use crate::{
    conn::EndpointConn,
    internals::raw_session::RpcError,
    tunnel::{
        AcceptError,
        TunnelInner,
    },
};

/// Common interface for all ngrok endpoints.
///
/// Provides access to endpoint metadata such as ID, URL, and protocol.
/// This trait replaces the v1 `TunnelInfo` and `EndpointInfo` traits.
#[async_trait]
pub trait Endpoint: Send + Sync {
    /// Returns the endpoint's unique ID.
    fn id(&self) -> &str;
    /// Returns the endpoint's URL as a string.
    fn url(&self) -> &str;
    /// Returns the protocol of the endpoint (e.g. "https", "tcp", "tls").
    fn protocol(&self) -> &str;
    /// Returns the opaque metadata string for this endpoint.
    fn metadata(&self) -> &str;
    /// Returns the traffic policy for this endpoint (if set).
    fn traffic_policy(&self) -> &str;
    /// Close the endpoint.
    async fn close(&mut self) -> Result<(), RpcError>;
}

/// An endpoint that accepts incoming connections.
///
/// This is the v2 replacement for the protocol-specific tunnel types
/// (`HttpTunnel`, `TcpTunnel`, `TlsTunnel`). It implements
/// [`futures::Stream`] yielding [`Conn`](crate::conn::ConnInner) values
/// and provides the [`Endpoint`] trait for metadata access.
///
/// # Examples
///
/// ```no_run
/// use futures::TryStreamExt;
///
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let mut listener = ngrok::listen().start().await?;
/// println!("Listening on: {}", listener.url());
///
/// // Accept connections as a stream
/// // (actual usage with typed connections will depend on your protocol)
/// # Ok(())
/// # }
/// ```
pub struct EndpointListener {
    pub(crate) inner: TunnelInner,
}

impl EndpointListener {
    /// Create an EndpointListener from a TunnelInner.
    pub(crate) fn from_tunnel(inner: TunnelInner) -> Self {
        EndpointListener { inner }
    }

    /// Returns the endpoint's URL.
    pub fn url(&self) -> &str {
        self.inner.url()
    }

    /// Returns the endpoint's unique ID.
    pub fn id(&self) -> &str {
        self.inner.id()
    }

    /// Returns the protocol of the endpoint.
    pub fn proto(&self) -> &str {
        self.inner.proto()
    }

    /// Returns the metadata for this endpoint.
    pub fn metadata(&self) -> &str {
        self.inner.metadata()
    }

    /// Returns the forwards_to metadata.
    pub fn forwards_to(&self) -> &str {
        self.inner.forwards_to()
    }
}

#[async_trait]
impl Endpoint for EndpointListener {
    fn id(&self) -> &str {
        self.inner.id()
    }
    fn url(&self) -> &str {
        self.inner.url()
    }
    fn protocol(&self) -> &str {
        self.inner.proto()
    }
    fn metadata(&self) -> &str {
        self.inner.metadata()
    }
    fn traffic_policy(&self) -> &str {
        // Traffic policy is not stored on the tunnel; return empty
        ""
    }
    async fn close(&mut self) -> Result<(), RpcError> {
        self.inner.close().await
    }
}

impl Stream for EndpointListener {
    type Item = Result<EndpointConn, AcceptError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.inner)
            .poll_next(cx)
            .map(|o| o.map(|r| r.map(|c| EndpointConn { inner: c })))
    }
}

impl Unpin for EndpointListener {}

/// An endpoint that forwards traffic to an upstream service.
///
/// This is the v2 replacement for `Forwarder<T>`. It wraps an endpoint
/// that is actively forwarding connections to a configured upstream URL.
///
/// # Examples
///
/// ```no_run
/// use ngrok::Upstream;
///
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let mut fwd = ngrok::forward(Upstream::new("localhost:8080"))
///     .url("https://app.ngrok.app")
///     .start()
///     .await?;
///
/// println!("Forwarding {} -> localhost:8080", fwd.url());
///
/// // Wait for the forwarder to finish
/// fwd.join().await??;
/// # Ok(())
/// # }
/// ```
pub struct EndpointForwarder {
    pub(crate) inner: EndpointListener,
    pub(crate) join: JoinHandle<Result<(), Box<dyn std::error::Error + Send + Sync>>>,
}

impl EndpointForwarder {
    /// Returns the endpoint's URL.
    pub fn url(&self) -> &str {
        self.inner.url()
    }

    /// Returns the endpoint's unique ID.
    pub fn id(&self) -> &str {
        self.inner.id()
    }

    /// Returns the protocol of the endpoint.
    pub fn proto(&self) -> &str {
        self.inner.proto()
    }

    /// Returns the metadata for this endpoint.
    pub fn metadata(&self) -> &str {
        self.inner.metadata()
    }

    /// Returns the forwards_to metadata.
    pub fn forwards_to(&self) -> &str {
        self.inner.forwards_to()
    }

    /// Wait for the forwarding task to complete.
    pub async fn join(
        &mut self,
    ) -> Result<Result<(), Box<dyn std::error::Error + Send + Sync>>, tokio::task::JoinError> {
        (&mut self.join).await
    }
}

#[async_trait]
impl Endpoint for EndpointForwarder {
    fn id(&self) -> &str {
        self.inner.id()
    }
    fn url(&self) -> &str {
        self.inner.url()
    }
    fn protocol(&self) -> &str {
        self.inner.proto()
    }
    fn metadata(&self) -> &str {
        self.inner.metadata()
    }
    fn traffic_policy(&self) -> &str {
        ""
    }
    async fn close(&mut self) -> Result<(), RpcError> {
        self.inner.close().await
    }
}
