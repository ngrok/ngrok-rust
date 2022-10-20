use std::{
    fmt,
    ops::{
        Deref,
        DerefMut,
        RangeInclusive,
    },
};

use async_trait::async_trait;
use bytes::{
    Buf,
    BufMut,
};
use tokio::io::{
    AsyncReadExt,
    AsyncWriteExt,
};

use crate::{
    constrained::*,
    errors::ErrorType,
    session::Session,
    stream::Stream,
};

constrained_num!(StreamType, u32, 0..=u32::MAX);

#[derive(Clone)]
pub struct Typed<S> {
    inner: S,
}

impl<S> DerefMut for Typed<S> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl<S> Deref for Typed<S> {
    type Target = S;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<S> Typed<S>
where
    S: Session,
{
    pub fn new(inner: S) -> Self {
        Typed { inner }
    }

    pub async fn accept_typed(&mut self) -> Result<TypedStream, ErrorType> {
        let mut stream = self.accept().await.ok_or(ErrorType::SessionClosed)?;

        let mut buf = [0u8; 4];

        stream
            .read_exact(&mut buf[..])
            .await
            .map_err(|_| ErrorType::StreamClosed)?;

        let typ = StreamType::clamp((&buf[..]).get_u32());

        Ok(TypedStream { typ, inner: stream })
    }

    pub async fn open_typed(&mut self, typ: StreamType) -> Result<TypedStream, ErrorType> {
        let mut stream = self.open().await?;

        let mut bytes = [0u8; 4];
        (&mut bytes[..]).put_u32(*typ);

        stream
            .write(&bytes[..])
            .await
            .map_err(|_| ErrorType::StreamReset)?;

        Ok(TypedStream { inner: stream, typ })
    }
}

pub struct TypedStream {
    typ: StreamType,
    inner: Stream,
}

impl DerefMut for TypedStream {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl Deref for TypedStream {
    type Target = Stream;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl TypedStream {
    pub fn typ(&self) -> StreamType {
        self.typ
    }
}

#[async_trait]
pub trait TypedSession: Send {
    async fn accept_typed(&mut self) -> Result<TypedStream, ErrorType>;
    async fn open_typed(&mut self, typ: StreamType) -> Result<TypedStream, ErrorType>;
}

#[async_trait]
impl<S> TypedSession for Typed<S>
where
    S: Session,
{
    async fn accept_typed(&mut self) -> Result<TypedStream, ErrorType> {
        Self::accept_typed(self).await
    }
    async fn open_typed(&mut self, typ: StreamType) -> Result<TypedStream, ErrorType> {
        Self::open_typed(self, typ).await
    }
}
