#[cfg(not(target_os = "windows"))]
use std::borrow::Cow;
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
    Instrument,
    Span,
};
use url::Url;
#[cfg(target_os = "windows")]
use windows_sys::Win32::Foundation::ERROR_PIPE_BUSY;

use crate::{
    prelude::*,
    session::IoStream,
    EdgeConn,
    EndpointConn,
};

#[allow(deprecated)]
#[async_trait]
impl<T> TunnelExt for T
where
    T: Tunnel + Send,
    <T as Tunnel>::Conn: ConnExt,
{
    async fn forward(&mut self, url: Url) -> Result<(), io::Error> {
        forward_tunnel(self, url).await
    }
}

/// Extension methods auto-implemented for all tunnel types
#[async_trait]
#[deprecated = "superceded by the `listen_and_forward` builder method"]
pub trait TunnelExt: Tunnel + Send {
    /// Forward incoming tunnel connections to the provided url based on its
    /// scheme.
    /// This currently supports http, https, tls, and tcp on all platforms, unix
    /// sockets on unix platforms, and named pipes on Windows via the "pipe"
    /// scheme.
    ///
    /// Unix socket URLs can be formatted as `unix://path/to/socket` or
    /// `unix:path/to/socket` for relative paths or as `unix:///path/to/socket` or
    /// `unix:/path/to/socket` for absolute paths.
    ///
    /// Windows named pipe URLs can be formatted as `pipe:mypipename` or
    /// `pipe://host/mypipename`. If no host is provided, as with
    /// `pipe:///mypipename` or `pipe:/mypipename`, the leading slash will be
    /// preserved.
    async fn forward(&mut self, url: Url) -> Result<(), io::Error>;
}

#[async_trait]
pub(crate) trait ConnExt {
    async fn forward_to(mut self, url: &Url) -> Result<JoinHandle<()>, io::Error>;
}

#[tracing::instrument(skip_all, fields(tunnel_id = tun.id(), url = %url))]
pub(crate) async fn forward_tunnel<T>(tun: &mut T, url: Url) -> Result<(), io::Error>
where
    T: Tunnel + 'static + ?Sized,
    <T as Tunnel>::Conn: ConnExt,
{
    loop {
        let tunnel_conn = if let Some(conn) = tun
            .try_next()
            .await
            .map_err(|err| io::Error::new(io::ErrorKind::NotConnected, err))?
        {
            conn
        } else {
            return Ok(());
        };

        tunnel_conn.forward_to(&url).await?;
    }
}

#[async_trait]
impl ConnExt for EdgeConn {
    async fn forward_to(self, url: &Url) -> Result<JoinHandle<()>, io::Error> {
        let upstream = match connect(
            self.edge_type() == EdgeType::Tls && self.passthrough_tls(),
            false,
            url,
        )
        .await
        {
            Ok(conn) => conn,
            Err(e) => {
                #[cfg(feature = "hyper")]
                if self.edge_type() == EdgeType::Https {
                    serve_gateway_error(format!("{e}"), self);
                }
                return Err(e);
            }
        };

        Ok(join_streams(self, upstream))
    }
}

#[async_trait]
impl ConnExt for EndpointConn {
    async fn forward_to(self, url: &Url) -> Result<JoinHandle<()>, io::Error> {
        let upstream = match connect(
            self.proto() == "tls" && self.inner.info.header.passthrough_tls,
            false,
            url,
        )
        .await
        {
            Ok(conn) => conn,
            Err(e) => {
                #[cfg(feature = "hyper")]
                match self.proto() {
                    "http" | "https" => {
                        serve_gateway_error(format!("{e}"), self);
                    }
                    _ => {}
                }
                return Err(e);
            }
        };

        Ok(join_streams(self, upstream))
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
async fn connect(
    tunnel_tls: bool,
    _tunnel_proxyproto: bool,
    url: &Url,
) -> Result<Box<dyn IoStream>, io::Error> {
    let host = url.host_str().unwrap_or("localhost");
    let mut backend_tls: bool = false;
    let mut conn: Box<dyn IoStream> = match url.scheme() {
        "tcp" => {
            let port = url.port().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("missing port for tcp forwarding url {url}"),
                )
            })?;
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

            backend_tls = true;
            Box::new(conn)
        }

        #[cfg(not(target_os = "windows"))]
        "unix" => {
            //
            let mut addr = Cow::Borrowed(url.path());
            if let Some(host) = url.host_str() {
                // note: if host exists, there should always be a leading / in
                // the path, but we should consider it a relative path.
                addr = Cow::Owned(format!("{host}{addr}"));
            }
            Box::new(UnixStream::connect(&*addr).await?)
        }

        #[cfg(target_os = "windows")]
        "pipe" => {
            let mut pipe_name = url.path();
            if url.host_str().is_some() {
                pipe_name = pipe_name.strip_prefix('/').unwrap_or(pipe_name);
            }
            if pipe_name.is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("missing pipe name in forwarding url {url}"),
                ));
            }
            let host = url
                .host_str()
                // Consider localhost to mean "." for the pipe name
                .map(|h| if h == "localhost" { "." } else { h })
                .unwrap_or(".");
            // Finally, assemble the full name.
            let addr = format!("\\\\{host}\\pipe\\{pipe_name}");
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
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("unrecognized scheme in forwarding url: {url}"),
            ))
        }
    };

    if backend_tls && !tunnel_tls {
        let domain = rustls::ServerName::try_from(host)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        conn = Box::new(
            async_rustls::TlsConnector::from(tls_config().map_err(|e| e.kind())?)
                .connect(domain, conn.compat())
                .await?
                .compat(),
        )
    }

    // TODO: proxyproto, header rewrites?

    Ok(conn)
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
