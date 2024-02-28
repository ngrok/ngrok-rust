use std::{
    collections::HashMap,
    io,
};

use tokio::task::JoinHandle;
use url::Url;

use crate::{
    prelude::{
        EdgeInfo,
        EndpointInfo,
        TunnelCloser,
        TunnelInfo,
    },
    session::RpcError,
    Tunnel,
};

/// An ngrok forwarder.
///
/// Represents a tunnel that is being forwarded to a URL.
pub struct Forwarder<T> {
    pub(crate) join: JoinHandle<Result<(), io::Error>>,
    pub(crate) inner: T,
}

impl<T> Forwarder<T> {
    /// Wait for the forwarding task to exit.
    pub fn join(&mut self) -> &mut JoinHandle<Result<(), io::Error>> {
        &mut self.join
    }
}

impl<T> TunnelCloser for Forwarder<T>
where
    T: TunnelCloser + Send,
{
    async fn close(&mut self) -> Result<(), RpcError> {
        self.inner.close().await
    }
}

impl<T> TunnelInfo for Forwarder<T>
where
    T: TunnelInfo,
{
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

impl<T> EndpointInfo for Forwarder<T>
where
    T: EndpointInfo,
{
    fn proto(&self) -> &str {
        self.inner.proto()
    }

    fn url(&self) -> &str {
        self.inner.url()
    }
}

impl<T> EdgeInfo for Forwarder<T>
where
    T: EdgeInfo,
{
    fn labels(&self) -> &HashMap<String, String> {
        self.inner.labels()
    }
}

pub(crate) fn forward<T>(mut listener: T, info: T, to_url: Url) -> Result<Forwarder<T>, RpcError>
where
    T: Tunnel + Send + 'static,
    <T as Tunnel>::Conn: crate::tunnel_ext::ConnExt,
{
    let handle =
        tokio::spawn(async move { crate::tunnel_ext::forward_tunnel(&mut listener, to_url).await });

    Ok(Forwarder {
        join: handle,
        inner: info,
    })
}
