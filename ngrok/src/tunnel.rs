use std::{
    pin::Pin,
    task::{
        Context,
        Poll,
    },
};

use futures::{
    Stream,
    StreamExt,
    TryStreamExt,
};
use tokio::{
    io::{
        AsyncRead,
        AsyncWrite,
    },
    sync::mpsc::{
        channel,
        Receiver,
    },
};

use crate::internals::{raw_session::TunnelStream, proto::ProxyHeader};

pub struct Tunnel {
    pub(crate) incoming: Receiver<anyhow::Result<Conn>>,
}

pub struct Conn {
    pub(crate) inner: TunnelStream,
}

impl Stream for Tunnel {
    type Item = Result<Conn, anyhow::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.incoming.poll_recv(cx)
    }
}

impl Tunnel {
    /// Accept an incomming connection on this tunnel.
    pub async fn accept(&mut self) -> anyhow::Result<Option<Conn>> {
        self.try_next().await
    }

}

impl Conn {
	pub fn header(&self) -> &ProxyHeader {
		&self.inner.header
	}
}

impl AsyncRead for Conn {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut *self.inner.stream).poll_read(cx, buf)
    }
}

impl AsyncWrite for Conn {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, std::io::Error>> {
        Pin::new(&mut *self.inner.stream).poll_write(cx, buf)
    }
    fn poll_flush(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        Pin::new(&mut *self.inner.stream).poll_flush(cx)
    }
    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        Pin::new(&mut *self.inner.stream).poll_shutdown(cx)
    }
}
