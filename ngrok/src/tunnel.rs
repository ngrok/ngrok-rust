use std::{
    collections::HashMap,
    net::SocketAddr,
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
use muxado::{
    typed::TypedStream,
    Error as MuxadoError,
};
use thiserror::Error;
use tokio::{
    io::{
        AsyncRead,
        AsyncWrite,
    },
    sync::mpsc::Receiver,
};

use crate::{
    config::{
        HttpTunnelBuilder,
        LabeledTunnelBuilder,
        TcpTunnelBuilder,
        TlsTunnelBuilder,
    },
    internals::{
        proto::{
            EdgeType,
            ProxyHeader,
        },
        raw_session::RpcError,
    },
    session::ConnectError,
    Session,
};

/// Errors arising when accepting a [Conn] from an ngrok tunnel.
#[derive(Error, Debug, Clone)]
pub enum AcceptError {
    /// An error occurred in the underlying transport protocol.
    #[error("transport error")]
    Transport(#[from] MuxadoError),
    /// An error arose during reconnect
    #[error("reconnect error")]
    Reconnect(Arc<ConnectError>),
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
    pub(crate) incoming: Option<Receiver<Result<Conn, AcceptError>>>,

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
        tokio::spawn(async move { sess.close_tunnel(&id).await });
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
            Stream<Item = Result<Conn, AcceptError>>
            + TunnelInfo
            + TunnelCloser
            $($hyper_bound)*
            + Unpin
            + Send
            + 'static
        {}

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
tunnel_trait!(+ Accept<Conn = Conn, Error = AcceptError>);

/// An ngrok tunnel that supports getting the URL it was started for.
pub trait UrlInfo {
    /// Returns the tunnel endpoint's URL.
    fn url(&self) -> &str;
}

/// An ngrok tunnel that supports getting the protocol it uses at the ngrok edge.
pub trait ProtoInfo {
    /// Returns the protocol of the tunnel's endpoint.
    fn proto(&self) -> &str;
}

/// An ngrok tunnel that supports getting the labels it was started with.
pub trait LabelsInfo {
    /// Returns the labels that the tunnel was started with.
    fn labels(&self) -> &HashMap<String, String>;
}

/// A connection from an ngrok tunnel.
///
/// This implements [AsyncRead]/[AsyncWrite], as well as providing access to the
/// address from which the connection to the ngrok edge originated.
pub struct Conn {
    pub(crate) header: ProxyHeader,
    pub(crate) remote_addr: SocketAddr,
    pub(crate) stream: TypedStream,
}

impl Conn {
    #[allow(dead_code)]
    fn edge_type(&self) -> EdgeType {
        self.header.edge_type
    }

    #[allow(dead_code)]
    fn proto(&self) -> &str {
        &self.header.proto
    }
}

impl Stream for TunnelInner {
    type Item = Result<Conn, AcceptError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.incoming
            .as_mut()
            .expect("tunnel inner lacks a receiver")
            .poll_recv(cx)
    }
}

#[cfg(feature = "hyper")]
impl Accept for TunnelInner {
    type Conn = Conn;
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
    pub(crate) fn split_listener(mut self) -> (TunnelInner, TunnelInner) {
        (
            TunnelInner {
                info: self.info.clone(),
                incoming: self.incoming.take(),
                session: self.session.clone(),
            },
            TunnelInner {
                info: self.info.clone(),
                incoming: None,
                session: self.session.clone(),
            },
        )
    }
}

impl Conn {
    /// Returns the client address that initiated the connection to the ngrok
    /// edge.
    pub fn remote_addr(&self) -> SocketAddr {
        self.remote_addr
    }
}

impl AsyncRead for Conn {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut *self.stream).poll_read(cx, buf)
    }
}

impl AsyncWrite for Conn {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, std::io::Error>> {
        Pin::new(&mut *self.stream).poll_write(cx, buf)
    }
    fn poll_flush(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        Pin::new(&mut *self.stream).poll_flush(cx)
    }
    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        Pin::new(&mut *self.stream).poll_shutdown(cx)
    }
}

// Support for axum's connection info trait.
#[cfg(feature = "axum")]
use axum::extract::connect_info::Connected;
#[cfg_attr(docsrs, doc(cfg(feature = "axum")))]
#[cfg(feature = "axum")]
impl Connected<&Conn> for SocketAddr {
    fn connect_info(target: &Conn) -> Self {
        target.remote_addr
    }
}

macro_rules! make_tunnel_type {
    ($(#[$outer:meta])* $wrapper:ident, $builder:tt, $($m:tt),*) => {
        $(#[$outer])*
        pub struct $wrapper {
            pub(crate) inner: TunnelInner,
        }

        impl $wrapper {
            /// Split this tunnel type into two parts - both of which have all
            /// tunnel information, but only the former can be used as a
            /// listener. Attempts to accept connections on the later will fail.
            pub(crate) fn split_listener(self) -> ($wrapper, $wrapper) {
                let (listener, info) = self.inner.split_listener();
                (
                    $wrapper {
                        inner: listener,
                    },
                    $wrapper {
                        inner: info,
                    }
                )
            }
        }

        impl Tunnel for $wrapper { }

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
            type Item = Result<Conn, AcceptError>;

            fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
                Pin::new(&mut self.inner).poll_next(cx)
            }
        }

        #[cfg(feature = "hyper")]
        #[cfg_attr(all(feature = "hyper", docsrs), doc(cfg(feature = "hyper")))]
        impl Accept for $wrapper {
            type Conn = Conn;
            type Error = AcceptError;

            fn poll_accept(
                mut self: Pin<&mut Self>,
                cx: &mut Context<'_>,
            ) -> Poll<Option<Result<Self::Conn, Self::Error>>> {
                Pin::new(&mut self.inner).poll_accept(cx)
            }
        }
    };
    (url; $wrapper:ty) => {
        impl UrlInfo for $wrapper {
            fn url(&self) -> &str {
                self.inner.url()
            }
        }
    };
    (proto; $wrapper:ty) => {
        impl ProtoInfo for $wrapper {
            fn proto(&self) -> &str {
                self.inner.proto()
            }
        }
    };
    (labels; $wrapper:ty) => {
        impl LabelsInfo for $wrapper {
            fn labels(&self) -> &HashMap<String, String> {
                self.inner.labels()
            }
        }
    };
}

make_tunnel_type! {
    /// An ngrok tunnel for an HTTP endpoint.
    HttpTunnel, HttpTunnelBuilder, url, proto
}
make_tunnel_type! {
    /// An ngrok tunnel for a TCP endpoint.
    TcpTunnel, TcpTunnelBuilder, url, proto
}
make_tunnel_type! {
    /// An ngrok tunnel for a TLS endpoint.
    TlsTunnel, TlsTunnelBuilder, url, proto
}
make_tunnel_type! {
    /// A labeled ngrok tunnel.
    LabeledTunnel, LabeledTunnelBuilder, labels, proto
}
