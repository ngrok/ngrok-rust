//! Wrappers to add typing to muxado streams.

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
use tracing::debug;

use crate::{
    constrained::*,
    errors::Error,
    session::{
        Accept,
        OpenClose,
        Session,
    },
    stream::Stream,
};

constrained_num! {
    /// A muxado stream type.
    StreamType, u32, 0..=u32::MAX, clamp
}

/// Wrapper for a session capable of opening streams prefixed with a `u32` type
/// id.
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
    /// Wrap a session for use with typed streams.
    pub fn new(inner: S) -> Self {
        Typed { inner }
    }
}

/// A typed muxado stream.
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
    /// Get the type ID for this stream.
    pub fn typ(&self) -> StreamType {
        self.typ
    }
}

/// Typed analogue to the [Session] trait.
pub trait TypedSession: TypedAccept + TypedOpenClose {
    /// The component implementing [TypedAccept].
    type TypedAccept: TypedAccept;
    /// The component implementing [TypedOpen].
    type TypedOpen: TypedOpenClose;

    /// Split the typed session into open/accept components.
    fn split_typed(self) -> (Self::TypedOpen, Self::TypedAccept);
}

/// Typed analogue to the [Accept] trait.
#[async_trait]
pub trait TypedAccept {
    /// Accept a typed stream.
    ///
    /// Because typed streams are indistinguishable from untyped streams, if the
    /// remote isn't sending a type, then the first 4 bytes of data will be
    /// misinterpreted as the stream type.
    async fn accept_typed(&mut self) -> Result<TypedStream, Error>;
}

/// Typed analogue to the [Open] trait.
#[async_trait]
pub trait TypedOpenClose {
    /// Open a typed stream with the given type.
    async fn open_typed(&mut self, typ: StreamType) -> Result<TypedStream, Error>;
    /// Close the session by sending a GOAWAY
    async fn close(&mut self, error: Error, msg: String) -> Result<(), Error>;
}

#[async_trait]
impl<S> TypedAccept for Typed<S>
where
    S: Accept + Send,
{
    async fn accept_typed(&mut self) -> Result<TypedStream, Error> {
        let mut stream = self.accept().await.ok_or(Error::SessionClosed)?;

        let mut buf = [0u8; 4];

        stream
            .read_exact(&mut buf[..])
            .await
            .map_err(|_| Error::StreamClosed)?;

        let typ = StreamType::clamp((&buf[..]).get_u32());

        debug!(?typ, "read stream type");

        Ok(TypedStream { typ, inner: stream })
    }
}
#[async_trait]
impl<S> TypedOpenClose for Typed<S>
where
    S: OpenClose + Send,
{
    async fn open_typed(&mut self, typ: StreamType) -> Result<TypedStream, Error> {
        let mut stream = self.open().await?;

        let mut bytes = [0u8; 4];
        (&mut bytes[..]).put_u32(*typ);

        stream
            .write(&bytes[..])
            .await
            .map_err(|_| Error::StreamReset)?;

        Ok(TypedStream { inner: stream, typ })
    }

    async fn close(&mut self, error: Error, msg: String) -> Result<(), Error> {
        self.inner.close(error, msg).await
    }
}

impl<S> TypedSession for Typed<S>
where
    S: Session + Send,
    S::Accept: Send,
    S::OpenClose: Send,
{
    type TypedAccept = Typed<S::Accept>;
    type TypedOpen = Typed<S::OpenClose>;
    fn split_typed(self) -> (Self::TypedOpen, Self::TypedAccept) {
        let (open, accept) = self.inner.split();
        (Typed { inner: open }, Typed { inner: accept })
    }
}
