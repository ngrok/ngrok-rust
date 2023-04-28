use std::error::Error;

use async_trait::async_trait;
use muxado::{
    Accept,
    MuxadoAccept,
    MuxadoOpen,
    OpenClose,
};
use quinn::VarInt;
use tokio::{
    io::{
        AsyncRead,
        AsyncWrite,
    },
    sync::Mutex,
};

/// Alias for a Result that uses an Error trait object.
pub type DynResult<T> = Result<T, DynError>;
/// Alias for an AsyncWrite trait object.
pub type DynWrite = Box<dyn AsyncWrite + Unpin + Send + 'static>;
/// Alias for an AsyncRead trait object.
pub type DynRead = Box<dyn AsyncRead + Unpin + Send + 'static>;
/// Alias for an Error trait object.
pub type DynError = Box<dyn Error + Send + Sync + 'static>;

/// A stream multiplexer. This is ngrok's network transport primitive.
#[async_trait]
pub trait StreamMux: Send + Sync + 'static {
    /// Open a new stream.
    async fn open(&self) -> DynResult<(DynWrite, DynRead)>;
    /// Close the stream multiplexer session.
    async fn close(&self) -> DynResult<()>;
    /// Accept a new stream.
    async fn accept(&self) -> DynResult<(DynWrite, DynRead)>;
}

#[async_trait]
impl<T: StreamMux + ?Sized> StreamMux for Box<T> {
    async fn open(&self) -> DynResult<(DynWrite, DynRead)> {
        T::open(self).await
    }
    async fn close(&self) -> DynResult<()> {
        T::close(self).await
    }
    async fn accept(&self) -> DynResult<(DynWrite, DynRead)> {
        T::accept(self).await
    }
}

/// A stream multiplexer using the muxado protocol.
pub struct Muxado {
    opener: Mutex<MuxadoOpen>,
    accepter: Mutex<MuxadoAccept>,
}

impl Muxado {
    /// Start a new muxado session over a byte stream, usually a TLS connection.
    pub fn new<S>(stream: S) -> Self
    where
        S: AsyncRead + AsyncWrite + Send + 'static,
    {
        use muxado::Session;
        let sess = muxado::SessionBuilder::new(stream).start();
        let (open, accept) = sess.split();
        Self {
            opener: Mutex::new(open),
            accepter: Mutex::new(accept),
        }
    }
}

#[async_trait]
impl StreamMux for Muxado {
    async fn open(&self) -> DynResult<(DynWrite, DynRead)> {
        let mut lock = self.opener.lock().await;
        let stream = lock.open().await?;
        let (rd, wr) = tokio::io::split(stream);
        Ok((Box::new(wr), Box::new(rd)))
    }
    async fn close(&self) -> DynResult<()> {
        let mut lock = self.opener.lock().await;
        lock.close(muxado::Error::None, "".into()).await?;
        Ok(())
    }
    async fn accept(&self) -> DynResult<(DynWrite, DynRead)> {
        let mut lock = self.accepter.lock().await;
        let stream = lock.accept().await.ok_or(muxado::Error::SessionClosed)?;
        let (rd, wr) = tokio::io::split(stream);
        Ok((Box::new(wr), Box::new(rd)))
    }
}

#[async_trait]
impl StreamMux for quinn::Connection {
    async fn accept(&self) -> DynResult<(DynWrite, DynRead)> {
        let (wr, rd) = self.accept_bi().await?;
        Ok((Box::new(wr), Box::new(rd)))
    }
    async fn close(&self) -> DynResult<()> {
        self.close(VarInt::from_u32(0), &[]);
        Ok(())
    }
    async fn open(&self) -> DynResult<(DynWrite, DynRead)> {
        let (wr, rd) = self.open_bi().await?;
        Ok((Box::new(wr), Box::new(rd)))
    }
}
