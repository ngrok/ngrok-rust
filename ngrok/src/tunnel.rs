use std::{
    collections::HashMap,
    pin::Pin,
    sync::Arc,
    task::{
        Context,
        Poll,
    },
};

use futures::{
    Stream,
    TryStreamExt,
};
use tokio::{
    io::{
        AsyncRead,
        AsyncWrite,
    },
    sync::{
        mpsc::Receiver,
        Mutex,
    },
};

use crate::internals::{
    proto::{
        BindExtra,
        BindOpts,
        ProxyHeader,
    },
    raw_session::{
        RawSession,
        TunnelStream,
    },
};

pub struct Tunnel {
    pub(crate) id: String,
    pub(crate) config_proto: String,
    pub(crate) url: String,
    pub(crate) opts: Option<BindOpts>,
    pub(crate) token: String,
    pub(crate) bind_extra: BindExtra,
    pub(crate) labels: HashMap<String, String>,
    pub(crate) forwards_to: String,
    pub(crate) sess: Arc<Mutex<RawSession>>,
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

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub async fn close(&mut self) -> anyhow::Result<()> {
        self.sess.lock().await.unlisten(&self.id).await?;
        self.incoming.close();
        Ok(())
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
