use std::{
    collections::HashMap,
    pin::Pin,
    sync::Arc,
    task::{
        Context,
        Poll,
    },
};

use async_trait::async_trait;
use futures::Stream;
#[cfg(feature = "hyper")]
use hyper::server::accept::Accept;
use muxado::Error as MuxadoError;
use thiserror::Error;
use tokio::sync::mpsc::Receiver;

use crate::{
    config::{
        HttpTunnelBuilder,
        LabeledTunnelBuilder,
        TcpTunnelBuilder,
        TlsTunnelBuilder,
    },
    conn::{
        ConnInner,
        EdgeConn,
        EndpointConn,
    },
    internals::raw_session::RpcError,
    session::ConnectError,
    Session,
};

/// Errors arising when accepting a [Conn] from an ngrok tunnel.
#[derive(Error, Debug, Clone)]
#[non_exhaustive]
pub enum AcceptError {
    /// An error occurred in the underlying transport protocol.
    #[error("transport error")]
    Transport(#[from] MuxadoError),
    /// An error arose during reconnect
    #[error("reconnect error")]
    Reconnect(#[from] Arc<ConnectError>),
    /// The listener was closed.
    #[error("listener closed: {message}{}", error_code.clone().map(|s| format!(", {s}")).unwrap_or_else(String::new))]
    ListenerClosed {
        /// The error message.
        message: String,
        /// The error code, if any.
        error_code: Option<String>,
    },
}

#[derive(Clone)]
pub(crate) struct TunnelInnerInfo {
    pub(crate) id: String,
    pub(crate) proto: String,
    pub(crate) url: String,
    pub(crate) labels: HashMap<String, String>,
    pub(crate) forwards_to: String,
    pub(crate) metadata: String,
}

pub(crate) struct TunnelInner {
    pub(crate) info: TunnelInnerInfo,
    pub(crate) incoming: Option<Receiver<Result<ConnInner, AcceptError>>>,

    // Note: this session field is also used to detect tunnel liveness for the
    // purposes of shutting down the accept loop. If it's ever removed, an
    // awaitdrop::Ref field needs to be added that's derived from the one
    // belonging to the session.
    pub(crate) session: Session,
}

impl Drop for TunnelInner {
    fn drop(&mut self) {
        let id = self.id().to_string();
        let sess = self.session.clone();
        let rt = sess.runtime();
        rt.spawn(async move { sess.close_tunnel(&id).await });
    }
}

// This codgen indirect is required to make the hyper "Accept" trait bound
// dependent on the hyper feature. You can't put a #[cfg] on a single bound, so
// we're putting the whole trait def in a macro. Gross, but gets the job done.
macro_rules! tunnel_trait {
    ($($hyper_bound:tt)*) => {
        /// An ngrok tunnel.
        ///
        /// ngrok [Tunnel]s act like TCP listeners and can be used as a
        /// [futures::stream::TryStream] of [Conn]ections from endpoints created on the ngrok
        /// service.
        pub trait Tunnel:
            Stream<Item = Result<<Self as Tunnel>::Conn, AcceptError>>
            + TunnelInfo
            + TunnelCloser
            $($hyper_bound)*
            + Unpin
            + Send
            + 'static
        {
            /// The type of connection associated with this tunnel type.
            /// Agent-initiated http, tls, and tcp tunnels all produce
            /// `EndpointConn`s, while labeled tunnels produce `EdgeConn`s.
            type Conn: crate::Conn;
        }

        /// Information associated with an ngrok tunnel.
        pub trait TunnelInfo {
            /// Returns a tunnel's unique ID.
            fn id(&self) -> &str;
            /// Returns a human-readable string presented in the ngrok dashboard
            /// and the Tunnels API. Use the [HttpTunnelBuilder::forwards_to],
            /// [TcpTunnelBuilder::forwards_to], etc. to set this value
            /// explicitly.
            fn forwards_to(&self) -> &str;
            /// Returns the arbitrary metadata string for this tunnel.
            fn metadata(&self) -> &str;
        }

        /// An ngrok tunnel closer.
        #[async_trait]
        pub trait TunnelCloser {
            /// Close the tunnel.
            ///
            /// This is an RPC call that must be `.await`ed.
            /// It is equivalent to calling `Session::close_tunnel` with this
            /// tunnel's ID.
            ///
            /// If the tunnel is dropped, a task will be spawned to close it
            /// asynchronously.
            async fn close(&mut self) -> Result<(), RpcError>;
        }
    }
}

#[cfg(not(feature = "hyper"))]
tunnel_trait!();

#[cfg(feature = "hyper")]
tunnel_trait!(+ Accept<Conn = <Self as Tunnel>::Conn, Error = AcceptError>);

/// An ngrok tunnel backing a simple endpoint.
/// Most agent-configured tunnels fall into this category, with the exception of
/// labeled tunnels.
pub trait EndpointInfo {
    /// Returns the tunnel endpoint's URL.
    fn url(&self) -> &str;

    /// Returns the protocol of the tunnel's endpoint.
    fn proto(&self) -> &str;
}

/// An ngrok tunnel backing an edge.
/// Since labels may be dynamically defined via the dashboard or API, the url
/// and protocol for the tunnel is not knowable ahead of time.
pub trait EdgeInfo {
    /// Returns the labels that the tunnel was started with.
    fn labels(&self) -> &HashMap<String, String>;
}

impl Stream for TunnelInner {
    type Item = Result<ConnInner, AcceptError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.incoming
            .as_mut()
            .expect("tunnel inner lacks a receiver")
            .poll_recv(cx)
    }
}

#[cfg(feature = "hyper")]
impl Accept for TunnelInner {
    type Conn = ConnInner;
    type Error = AcceptError;

    fn poll_accept(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Self::Conn, Self::Error>>> {
        self.poll_next(cx)
    }
}

impl TunnelInner {
    /// Get this tunnel's ID as returned by the ngrok server.
    pub fn id(&self) -> &str {
        &self.info.id
    }

    /// Get the URL for this tunnel.
    /// Labeled tunnels will return an empty string.
    pub fn url(&self) -> &str {
        &self.info.url
    }

    /// Close the tunnel.
    /// This is an RPC call and needs to be `.await`ed.
    pub async fn close(&mut self) -> Result<(), RpcError> {
        self.session.close_tunnel(self.id()).await?;
        if let Some(r) = self.incoming.as_mut() {
            r.close()
        }
        Ok(())
    }

    /// Get the protocol that this tunnel uses.
    pub fn proto(&self) -> &str {
        &self.info.proto
    }

    /// Get the labels this tunnel was started with.
    /// The returned [`HashMap`] will be empty for non-labeled tunnels.
    pub fn labels(&self) -> &HashMap<String, String> {
        &self.info.labels
    }

    /// Get the address that this tunnel says it forwards to.
    pub fn forwards_to(&self) -> &str {
        &self.info.forwards_to
    }

    /// Get the user-supplied metadata for this tunnel.
    pub fn metadata(&self) -> &str {
        &self.info.metadata
    }

    /// Split the tunnel into two parts - the first contains the listener and
    /// all tunnel information, and the second contains *only* the information.
    pub(crate) fn make_info(&self) -> TunnelInner {
        TunnelInner {
            info: self.info.clone(),
            incoming: None,
            session: self.session.clone(),
        }
    }
}

macro_rules! make_tunnel_type {
    ($(#[$outer:meta])* $wrapper:ident, $builder:tt, $conn:tt, $($m:tt),*) => {
        $(#[$outer])*
        pub struct $wrapper {
            pub(crate) inner: TunnelInner,
        }

        impl $wrapper {
            /// Split this tunnel type into two parts - both of which have all
            /// tunnel information, but only the former can be used as a
            /// listener. Attempts to accept connections on the later will fail.
            pub(crate) fn make_info(&self) -> $wrapper {
                $wrapper {
                    inner: self.inner.make_info(),
                }
            }
        }

        impl Tunnel for $wrapper {
            type Conn = $conn;
        }

        impl TunnelInfo for $wrapper {
            fn id(&self) -> &str {
                self.inner.id()
            }

            fn forwards_to(&self) -> &str {
                self.inner.forwards_to()
            }

            fn metadata(&self) -> &str {
                self.inner.metadata()
            }
        }

        #[async_trait]
        impl TunnelCloser for $wrapper {
            async fn close(&mut self) -> Result<(), RpcError> {
                self.inner.close().await
            }
        }

        impl $wrapper {
            /// Create a builder for this tunnel type.
            pub fn builder(session: Session) -> $builder {
                $builder::from(session)
            }
        }

        $(
            make_tunnel_type!($m; $wrapper);
        )*

        impl Stream for $wrapper {
            type Item = Result<$conn, AcceptError>;

            fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
                Pin::new(&mut self.inner).poll_next(cx).map(|o| o.map(|r| r.map(|c| $conn { inner: c })))
            }
        }

        #[cfg(feature = "hyper")]
        #[cfg_attr(all(feature = "hyper", docsrs), doc(cfg(feature = "hyper")))]
        impl Accept for $wrapper {
            type Conn = $conn;
            type Error = AcceptError;

            fn poll_accept(
                mut self: Pin<&mut Self>,
                cx: &mut Context<'_>,
            ) -> Poll<Option<Result<Self::Conn, Self::Error>>> {
                Pin::new(&mut self.inner).poll_accept(cx).map(|o| o.map(|r| r.map(|c| $conn { inner: c })))
            }
        }
    };
    (endpoint; $wrapper:ty) => {
        impl EndpointInfo for $wrapper {
            fn url(&self) -> &str {
                self.inner.url()
            }
            fn proto(&self) -> &str {
                self.inner.proto()
            }
        }
    };
    (edge; $wrapper:ty) => {
        impl EdgeInfo for $wrapper {
            fn labels(&self) -> &HashMap<String, String> {
                self.inner.labels()
            }
        }
    };
}

make_tunnel_type! {
    /// An ngrok tunnel for an HTTP endpoint.
    HttpTunnel, HttpTunnelBuilder, EndpointConn, endpoint
}
make_tunnel_type! {
    /// An ngrok tunnel for a TCP endpoint.
    TcpTunnel, TcpTunnelBuilder, EndpointConn, endpoint
}
make_tunnel_type! {
    /// An ngrok tunnel for a TLS endpoint.
    TlsTunnel, TlsTunnelBuilder, EndpointConn, endpoint
}
make_tunnel_type! {
    /// A labeled ngrok tunnel.
    LabeledTunnel, LabeledTunnelBuilder, EdgeConn, edge
}
