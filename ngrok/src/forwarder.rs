use std::{
    collections::HashMap,
    io,
};

use async_trait::async_trait;
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
    pub(crate) join: Option<JoinHandle<Result<(), io::Error>>>,
    pub(crate) inner: T,
}

impl<T> Forwarder<T> {
    /// Wait for the forwarding task to exit.
    pub fn join(&mut self) -> Option<JoinHandle<Result<(), io::Error>>> {
        self.join.take()
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
pub(crate) fn forward<T, P>(
    listener: T,
    info: T,
    get_proto: P,
    to_url: Url,
) -> Result<Forwarder<T>, RpcError>
where
    T: Tunnel + TunnelExt + Send + 'static,
    P: FnOnce(&T) -> &str + Send + 'static,
{
    let handle = tokio::spawn(async move {
        let mut listener = listener;
        let proto = get_proto(&listener);
        match (to_url.scheme(), proto) {
            (_, "http") => {
                listener
                    .forward_http(format!(
                        "{}:{}",
                        to_url.host().unwrap_or(url::Host::Domain("localhost")),
                        to_url.port().unwrap_or(80)
                    ))
                    .await
            }

            _ => {
                listener
                    .forward_tcp(format!(
                        "{}:{}",
                        to_url.host().unwrap_or(url::Host::Domain("localhost")),
                        to_url.port().unwrap_or(0)
                    ))
                    .await
            }
        }
    });

    Ok(Forwarder {
        join: Some(handle),
        inner: info,
    })
}
