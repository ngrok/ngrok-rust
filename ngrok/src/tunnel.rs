use std::{
    collections::HashMap,
    net::SocketAddr,
    pin::Pin,
    task::{
        Context,
        Poll,
    },
};

use async_trait::async_trait;
use axum::extract::connect_info::Connected;
use futures::Stream;
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
            BindExtra,
            BindOpts,
        },
        raw_session::RpcError,
    },
    Session,
};

/// Errors arising when accepting a [Conn] from an ngrok tunnel.
#[derive(Error, Debug, Clone, Copy)]
pub enum AcceptError {
    /// An error occurred in the underlying transport protocol.
    #[error("transport error")]
    Transport(#[from] MuxadoError),
}

pub(crate) struct TunnelInner {
    pub(crate) id: String,
    pub(crate) proto: String,
    pub(crate) url: String,
    pub(crate) labels: HashMap<String, String>,
    pub(crate) forwards_to: String,
    pub(crate) session: Session,
    pub(crate) incoming: Receiver<Result<Conn, AcceptError>>,
}

/// An ngrok tunnel.
///
/// This acts like a TCP listener and can be used as a [Stream] of [Result]<[Conn], [AcceptError]>.
#[async_trait]
pub trait Tunnel:
    Stream<Item = Result<Conn, AcceptError>>
    + Accept<Conn = Conn, Error = AcceptError>
    + Unpin
    + Send
    + 'static
{
    /// The ID of this tunnel, assigned by the remote server.
    fn id(&self) -> &str;
    /// Get the forwards_to metadata for this tunnel.
    fn forwards_to(&self) -> &str;
    /// Close the tunnel.
    ///
    /// This is an RPC call that must be `.await`ed.
    async fn close(&mut self) -> Result<(), RpcError>;
}

/// An ngrok tunnel that supports getting the URL it was started for.
pub trait UrlTunnel: Tunnel {
    /// The URL that this tunnel backs.
    fn url(&self) -> &str;
}

/// An ngrok tunnel that supports getting the protocol it uses at the ngrok edge.
pub trait ProtoTunnel: Tunnel {
    /// The protocol of the endpoint that this tunnel backs.
    fn proto(&self) -> &str;
}

/// An ngrok tunnel that supports getting the labels it was started with.
pub trait LabelsTunnel: Tunnel {
    /// The labels this tunnel was started with.
    fn labels(&self) -> &HashMap<String, String>;
}

/// A connection from an ngrok tunnel.
///
/// This implements [AsyncRead]/[AsyncWrite], as well as providing access to the
/// address from which the connection to the ngrok edge originated.
pub struct Conn {
    pub(crate) remote_addr: SocketAddr,
    pub(crate) stream: TypedStream,
}

impl Stream for TunnelInner {
    type Item = Result<Conn, AcceptError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.incoming.poll_recv(cx)
    }
}

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
        &self.id
    }

    /// Get the URL for this tunnel.
    /// Labeled tunnels will return an empty string.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Close the tunnel.
    /// This is an RPC call and needs to be `.await`ed.
    pub async fn close(&mut self) -> Result<(), RpcError> {
        self.session.close_tunnel(&self.id).await?;
        self.incoming.close();
        Ok(())
    }

    /// Get the protocol that this tunnel uses.
    pub fn proto(&self) -> &str {
        &self.proto
    }

    /// Get the labels this tunnel was started with.
    /// The returned [`HashMap`] will be empty for non-labeled tunnels.
    pub fn labels(&self) -> &HashMap<String, String> {
        &self.labels
    }

    /// Get the address that this tunnel says it forwards to.
    pub fn forwards_to(&self) -> &str {
        &self.forwards_to
    }
}

impl Conn {
    /// Get the client address that initiated the connection to the ngrok edge.
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

        #[async_trait]
        impl Tunnel for $wrapper {
            fn id(&self) -> &str {
                self.inner.id()
            }

            async fn close(&mut self) -> Result<(), RpcError> {
                self.inner.close().await
            }

            fn forwards_to(&self) -> &str {
                self.inner.forwards_to()
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
        impl UrlTunnel for $wrapper {
            fn url(&self) -> &str {
                self.inner.url()
            }
        }
    };
    (proto; $wrapper:ty) => {
        impl ProtoTunnel for $wrapper {
            fn proto(&self) -> &str {
                self.inner.proto()
            }
        }
    };
    (labels; $wrapper:ty) => {
        impl LabelsTunnel for $wrapper {
            fn labels(&self) -> &HashMap<String, String> {
                self.inner.labels()
            }
        }
    };
}

make_tunnel_type! {
    /// An ngrok tunnel backing an HTTP endpoint.
    HttpTunnel, HttpTunnelBuilder, url, proto
}
make_tunnel_type! {
    /// An ngrok tunnel backing a TCP endpoint.
    TcpTunnel, TcpTunnelBuilder, url, proto
}
make_tunnel_type! {
    /// An ngrok tunnel bcking a TLS endpoint.
    TlsTunnel, TlsTunnelBuilder, url, proto
}
make_tunnel_type! {
    /// A labeled ngrok tunnel.
    LabeledTunnel, LabeledTunnelBuilder, labels
}
