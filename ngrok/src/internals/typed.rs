use std::sync::Arc;

use async_trait::async_trait;
use bytes::{
    Buf,
    BufMut,
};
use tokio::io::{
    AsyncReadExt,
    AsyncWriteExt,
};

use super::multiplexer::{
    DynRead,
    DynResult,
    DynWrite,
    StreamMux,
};

/// Wrapper for a session capable of opening streams prefixed with a `u32` type
/// id.
#[derive(Clone)]
pub struct Typed<S> {
    pub inner: S,
}

#[async_trait]
pub trait TypedMux: Send + Sync + 'static {
    async fn open_typed(&self, typ: u32) -> DynResult<(DynWrite, DynRead)>;
    async fn accept_typed(&self) -> DynResult<(u32, DynWrite, DynRead)>;
    async fn close(&self) -> DynResult<()>;
}

pub type TypedMuxHandle = Arc<dyn TypedMux>;

impl<S> Typed<S>
where
    S: StreamMux + Send + Sync + 'static,
{
    pub async fn open_typed(&self, typ: u32) -> DynResult<(DynWrite, DynRead)> {
        let (mut wr, rd) = self.inner.open().await?;

        let mut bytes = [0u8; 4];
        (&mut bytes[..]).put_u32(typ);

        wr.write_all(&bytes[..]).await?;

        Ok((wr, rd))
    }
    pub async fn accept_typed(&self) -> DynResult<(u32, DynWrite, DynRead)> {
        let (wr, mut rd) = self.inner.accept().await?;

        let mut buf = [0u8; 4];

        rd.read_exact(&mut buf[..]).await?;

        let typ = (&buf[..]).get_u32();

        tracing::debug!(?typ, "read stream type");

        Ok((typ, wr, rd))
    }
    pub async fn close(&self) -> DynResult<()> {
        self.inner.close().await?;
        Ok(())
    }
}
