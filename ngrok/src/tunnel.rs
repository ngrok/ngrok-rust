use std::{
    collections::HashMap,
    net::SocketAddr,
    pin::Pin,
    task::{
        Context,
        Poll,
    },
};

use axum::extract::connect_info::Connected;
use futures::{
    Stream,
    TryStreamExt,
};
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
    internals::{
        proto::{
            BindExtra,
            BindOpts,
        },
        raw_session::RpcError,
    },
    Session,
};

pub struct Tunnel {
    pub(crate) id: String,
    pub(crate) proto: String,
    pub(crate) url: String,
    pub(crate) labels: HashMap<String, String>,
    pub(crate) forwards_to: String,
    pub(crate) session: Session,
    pub(crate) incoming: Receiver<Result<Conn, AcceptError>>,

    // TODO: remove these allows once we start using these, or the fields if we
    //       decide we don't need them.
    #[allow(dead_code)]
    pub(crate) opts: Option<BindOpts>,
    #[allow(dead_code)]
    pub(crate) token: String,
    #[allow(dead_code)]
    pub(crate) bind_extra: BindExtra,
}

pub struct Conn {
    pub(crate) remote_addr: SocketAddr,
    pub(crate) stream: TypedStream,
}

#[derive(Error, Debug, Clone, Copy)]
pub enum AcceptError {
    #[error("transport error")]
    Transport(#[from] MuxadoError),
}

impl Stream for Tunnel {
    type Item = Result<Conn, AcceptError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.incoming.poll_recv(cx)
    }
}

impl Accept for Tunnel {
    type Conn = Conn;
    type Error = AcceptError;

    fn poll_accept(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Self::Conn, Self::Error>>> {
        self.poll_next(cx)
    }
}

impl Tunnel {
    /// Accept an incomming connection on this tunnel.
    pub async fn accept(&mut self) -> Result<Option<Conn>, AcceptError> {
        self.try_next().await
    }

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
