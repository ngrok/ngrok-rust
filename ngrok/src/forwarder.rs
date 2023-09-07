use std::{
    collections::HashMap,
    io,
};
#[cfg(feature = "hyper")]
use std::{
    error::Error as StdError,
    future::Future,
};

use async_trait::async_trait;
#[cfg(feature = "hyper")]
use hyper::{
    body::HttpBody,
    service::Service,
    Body,
    Request,
    Response,
};
use tokio::task::JoinHandle;
use url::Url;

#[allow(deprecated)]
use crate::prelude::TunnelExt;
use crate::{
    prelude::{
        LabelsInfo,
        ProtoInfo,
        TunnelCloser,
        TunnelInfo,
        UrlInfo,
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

#[async_trait]
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

impl<T> UrlInfo for Forwarder<T>
where
    T: UrlInfo,
{
    fn url(&self) -> &str {
        self.inner.url()
    }
}

impl<T> LabelsInfo for Forwarder<T>
where
    T: LabelsInfo,
{
    fn labels(&self) -> &HashMap<String, String> {
        self.inner.labels()
    }
}

impl<T> ProtoInfo for Forwarder<T>
where
    T: ProtoInfo,
{
    fn proto(&self) -> &str {
        self.inner.proto()
    }
}

#[allow(deprecated)]
pub(crate) fn forward<T>(mut listener: T, info: T, to_url: Url) -> Result<Forwarder<T>, RpcError>
where
    T: Tunnel + TunnelExt + Send + 'static,
{
    let handle = tokio::spawn(async move { listener.forward(to_url).await });

    Ok(Forwarder {
        join: handle,
        inner: info,
    })
}

#[cfg(feature = "hyper")]
pub(crate) fn serve_http<T, S, R, B, D, BE, E, SE, F, FE, SF>(
    listener: T,
    info: T,
    svc: S,
) -> Result<Forwarder<T>, RpcError>
where
    T: Tunnel + Send + 'static,
    for<'a> S: Service<&'a crate::Conn, Response = R, Error = E, Future = SF> + Send + 'static,
    R: Service<Request<Body>, Response = Response<B>, Error = SE, Future = F> + Send + 'static,
    B: HttpBody<Data = D, Error = BE> + Send + 'static,
    D: Send + 'static,
    BE: StdError + Send + Sync + 'static,
    E: StdError + Send + Sync + 'static,
    FE: StdError + Send + Sync + 'static,
    SE: StdError + Send + Sync + 'static,
    F: Future<Output = Result<Response<B>, FE>> + Send + 'static,
    SF: Future<Output = Result<R, E>> + Send + 'static,
{
    let server = hyper::Server::builder(listener).serve(svc);
    let handle = tokio::spawn(async move {
        server
            .await
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))
    });

    Ok(Forwarder {
        join: handle,
        inner: info,
    })
}
