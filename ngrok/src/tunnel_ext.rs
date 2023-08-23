#[cfg(target_os = "windows")]
use std::time::Duration;
#[cfg(feature = "hyper")]
use std::{
    convert::Infallible,
    fmt,
};
use std::{
    io,
    sync::Arc,
};

use async_rustls::rustls::{
    self,
    ClientConfig,
    RootCertStore,
};
use async_trait::async_trait;
use futures::stream::TryStreamExt;
#[cfg(feature = "hyper")]
use hyper::{
    server::conn::Http,
    service::service_fn,
    Body,
    Response,
    StatusCode,
};
use once_cell::sync::Lazy;
#[cfg(target_os = "windows")]
use tokio::net::windows::named_pipe::ClientOptions;
#[cfg(not(target_os = "windows"))]
use tokio::net::UnixStream;
#[cfg(target_os = "windows")]
use tokio::time;
use tokio::{
    io::{
        copy_bidirectional,
        AsyncRead,
        AsyncWrite,
    },
    net::TcpStream,
    task::JoinHandle,
};
use tokio_util::compat::{
    FuturesAsyncReadCompatExt,
    TokioAsyncReadCompatExt,
};
use tracing::{
    debug,
    field,
    info_span,
    warn,
    Instrument,
    Span,
};
use url::Url;
#[cfg(target_os = "windows")]
use windows_sys::Win32::Foundation::ERROR_PIPE_BUSY;

use crate::{
    prelude::*,
    session::IoStream,
    Conn,
};

impl<T> TunnelExt for T where T: Tunnel + Send {}

/// Extension methods auto-implemented for all tunnel types
#[async_trait]
pub trait TunnelExt: Tunnel + Send {
    /// Forward incoming tunnel connections to the provided url based on its
    /// scheme.
    /// This currently supports http, https, tls, and tcp on all platforms, unix
    /// sockets on unix platforms, and named pipes on Windows via the "pipe"
    /// scheme.
    #[tracing::instrument(skip_all, fields(tunnel_id = self.id(), url = %url))]
    async fn forward(&mut self, url: Url) -> Result<(), io::Error> {
        loop {
            let tunnel_conn = if let Some(conn) = self
                .try_next()
                .await
                .map_err(|err| io::Error::new(io::ErrorKind::NotConnected, err))?
            {
                conn
            } else {
                return Ok(());
            };

            let span = info_span!(
                "forward_one",
                remote_addr = %tunnel_conn.remote_addr(),
                forward_addr = field::Empty
            );

            debug!(parent: &span, "accepted tunnel connection");

            let local_conn = match connect(self, &tunnel_conn, &url)
                .instrument(span.clone())
                .await
            {
                Ok(conn) => conn,
                Err(error) => {
                    warn!(%error, "error establishing local connection");

                    span.in_scope(|| on_err(self, error, tunnel_conn));

                    continue;
                }
            };

            debug!(parent: &span, "established local connection, joining streams");

            span.in_scope(|| join_streams(tunnel_conn, local_conn));
        }
    }
}

fn on_err<T: Tunnel + Send + ?Sized>(tunnel: &T, err: io::Error, conn: Conn) {
    match tunnel.proto() {
        #[cfg(feature = "hyper")]
        "http" | "https" => drop(serve_gateway_error(err, conn)),
        _ => {}
    }
}

fn tls_config() -> Result<Arc<ClientConfig>, &'static io::Error> {
    static CONFIG: Lazy<Result<Arc<ClientConfig>, io::Error>> = Lazy::new(|| {
        let der_certs = rustls_native_certs::load_native_certs()?
            .into_iter()
            .map(|c| c.0)
            .collect::<Vec<_>>();
        let der_certs = der_certs.as_slice();
        let mut root_store = RootCertStore::empty();
        root_store.add_parsable_certificates(der_certs);
        let config = ClientConfig::builder()
            .with_safe_defaults()
            .with_root_certificates(root_store)
            .with_no_client_auth();
        Ok(Arc::new(config))
    });

    Ok(CONFIG.as_ref()?.clone())
}

// Establish the connection to forward the tunnel stream to.
// Takes the tunnel and connection to make additional decisions on how to wrap
// the forwarded connection, i.e. reordering tls termination and proxyproto.
// Note: this additional wrapping logic currently unimplemented.
async fn connect<T: Tunnel + Send + ?Sized>(
    _tunnel: &mut T,
    _conn: &Conn,
    url: &Url,
) -> Result<Box<dyn IoStream>, io::Error> {
    let host = url.host_str().unwrap_or("localhost");
    Ok(match url.scheme() {
        "tcp" => {
            let port = url.port().ok_or(io::ErrorKind::InvalidInput)?;
            let conn = connect_tcp(host, port).in_current_span().await?;
            Box::new(conn)
        }

        "http" => {
            let port = url.port().unwrap_or(80);
            let conn = connect_tcp(host, port).in_current_span().await?;
            Box::new(conn)
        }

        "https" | "tls" => {
            let port = url.port().unwrap_or(443);
            let conn = connect_tcp(host, port).in_current_span().await?;

            // TODO: if the tunnel uses proxyproto, wrap conn here before terminating tls

            let domain = rustls::ServerName::try_from(host)
                .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
            Box::new(
                async_rustls::TlsConnector::from(tls_config().map_err(|e| e.kind())?)
                    .connect(domain, conn.compat())
                    .await?
                    .compat(),
            )
        }

        #[cfg(not(target_os = "windows"))]
        "unix" => Box::new(
            UnixStream::connect(&format!(
                "{}/{}",
                url.host_str().unwrap_or_default(),
                url.path()
            ))
            .await?,
        ),

        #[cfg(target_os = "windows")]
        "pipe" => {
            let addr = format!(
                "\\\\{}\\{}",
                url.host_str().unwrap_or("."),
                url.path().replace("/", "\\")
            );
            // loop behavior copied from docs
            // https://docs.rs/tokio/latest/tokio/net/windows/named_pipe/struct.NamedPipeClient.html
            let local_conn = loop {
                match ClientOptions::new().open(&addr) {
                    Ok(client) => break client,
                    Err(error) if error.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) => (),
                    Err(error) => return Err(error),
                }

                time::sleep(Duration::from_millis(50)).await;
            };
            Box::new(local_conn)
        }
        _ => return Err(io::ErrorKind::InvalidInput.into()),
    })
}

async fn connect_tcp(host: &str, port: u16) -> Result<TcpStream, io::Error> {
    let conn = TcpStream::connect(&format!("{}:{}", host, port)).await?;
    if let Ok(addr) = conn.peer_addr() {
        Span::current().record("forward_addr", field::display(addr));
    }
    Ok(conn)
}

fn join_streams(
    mut left: impl AsyncRead + AsyncWrite + Unpin + Send + 'static,
    mut right: impl AsyncRead + AsyncWrite + Unpin + Send + 'static,
) -> JoinHandle<()> {
    tokio::spawn(
        async move {
            match copy_bidirectional(&mut left, &mut right).await {
                Ok((l_bytes, r_bytes)) => debug!("joined streams closed, bytes from tunnel: {l_bytes}, bytes from local: {r_bytes}"),
                Err(e) => debug!("joined streams error: {e}"),
            };
        }
        .in_current_span(),
    )
}

#[cfg(feature = "hyper")]
#[allow(dead_code)]
fn serve_gateway_error(
    err: impl fmt::Display + Send + 'static,
    conn: impl AsyncRead + AsyncWrite + Unpin + Send + 'static,
) -> JoinHandle<()> {
    tokio::spawn(
        async move {
            let res = Http::new()
                .http1_only(true)
                .http1_keep_alive(false)
                .serve_connection(
                    conn,
                    service_fn(move |_req| {
                        debug!("serving bad gateway error");
                        let mut resp =
                            Response::new(Body::from(format!("failed to dial backend: {err}")));
                        *resp.status_mut() = StatusCode::BAD_GATEWAY;
                        futures::future::ok::<_, Infallible>(resp)
                    }),
                )
                .await;
            debug!(?res, "connection closed");
        }
        .in_current_span(),
    )
}
