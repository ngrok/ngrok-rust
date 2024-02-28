#[cfg(not(target_os = "windows"))]
use std::borrow::Cow;
#[cfg(target_os = "windows")]
use std::time::Duration;
use std::{
    collections::HashMap,
    io,
    sync::Arc,
};
#[cfg(feature = "hyper")]
use std::{
    convert::Infallible,
    fmt,
};

use futures::stream::TryStreamExt;
use futures_rustls::rustls::{
    pki_types,
    ClientConfig,
    RootCertStore,
};
#[cfg(feature = "hyper")]
use hyper::{
    server::conn::Http,
    service::service_fn,
    Body,
    Response,
    StatusCode,
};
use once_cell::sync::Lazy;
use proxy_protocol::ProxyHeader;
#[cfg(feature = "hyper")]
use tokio::io::{
    AsyncRead,
    AsyncWrite,
};
#[cfg(target_os = "windows")]
use tokio::net::windows::named_pipe::ClientOptions;
#[cfg(not(target_os = "windows"))]
use tokio::net::UnixStream;
#[cfg(target_os = "windows")]
use tokio::time;
use tokio::{
    io::copy_bidirectional,
    net::TcpStream,
    task::JoinHandle,
};
use tokio_util::compat::{
    FuturesAsyncReadCompatExt,
    TokioAsyncReadCompatExt,
};
#[cfg(feature = "hyper")]
use tracing::debug;
use tracing::{
    field,
    warn,
    Instrument,
    Span,
};
use url::Url;
#[cfg(target_os = "windows")]
use windows_sys::Win32::Foundation::ERROR_PIPE_BUSY;

use crate::{
    prelude::*,
    proxy_proto,
    session::IoStream,
    EdgeConn,
    EndpointConn,
};

#[allow(deprecated)]
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

pub(crate) trait ConnExt {
    fn forward_to(self, url: &Url) -> JoinHandle<io::Result<()>>;
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

        tunnel_conn.forward_to(&url);
    }
}

impl ConnExt for EdgeConn {
    fn forward_to(mut self, url: &Url) -> JoinHandle<io::Result<()>> {
        let url = url.clone();
        tokio::spawn(async move {
            let mut upstream = match connect(
                self.edge_type() == EdgeType::Tls && self.passthrough_tls(),
                self.inner.info.app_protocol.clone(),
                None, // Edges don't support proxyproto (afaik)
                &url,
            )
            .await
            {
                Ok(conn) => conn,
                Err(error) => {
                    #[cfg(feature = "hyper")]
                    if self.edge_type() == EdgeType::Https {
                        serve_gateway_error(format!("{error}"), self);
                    }
                    warn!(%error, "error connecting to upstream");
                    return Err(error);
                }
            };

            copy_bidirectional(&mut self, &mut upstream).await?;
            Ok(())
        })
    }
}

impl ConnExt for EndpointConn {
    fn forward_to(self, url: &Url) -> JoinHandle<Result<(), io::Error>> {
        let url = url.clone();
        tokio::spawn(async move {
            let proxy_proto = self.inner.info.proxy_proto;
            let proto_tls = self.proto() == "tls";
            #[cfg(feature = "hyper")]
            let proto_http = matches!(self.proto(), "http" | "https");
            let passthrough_tls = self.inner.info.passthrough_tls();
            let app_protocol = self.inner.info.app_protocol.clone();

            let (mut stream, proxy_header) = match proxy_proto {
                ProxyProto::None => (crate::proxy_proto::Stream::disabled(self), None),
                _ => {
                    let mut stream = crate::proxy_proto::Stream::incoming(self);
                    let header = stream
                        .proxy_header()
                        .await?
                        .map_err(|e| {
                            io::Error::new(
                                io::ErrorKind::InvalidData,
                                format!("invalid proxy-protocol header: {}", e),
                            )
                        })?
                        .cloned();
                    (stream, header)
                }
            };

            let mut upstream = match connect(
                proto_tls && passthrough_tls,
                app_protocol,
                proxy_header,
                &url,
            )
            .await
            {
                Ok(conn) => conn,
                Err(error) => {
                    #[cfg(feature = "hyper")]
                    if proto_http {
                        serve_gateway_error(format!("{error}"), stream);
                    }
                    warn!(%error, "error connecting to upstream");
                    return Err(error);
                }
            };

            copy_bidirectional(&mut stream, &mut upstream).await?;
            Ok(())
        })
    }
}

fn tls_config(app_protocol: Option<String>) -> Result<Arc<ClientConfig>, &'static io::Error> {
    // The root certificate store, lazily loaded once.
    static ROOT_STORE: Lazy<Result<RootCertStore, io::Error>> = Lazy::new(|| {
        let der_certs = rustls_native_certs::load_native_certs()?
            .into_iter()
            .collect::<Vec<_>>();
        let mut root_store = RootCertStore::empty();
        root_store.add_parsable_certificates(der_certs);
        Ok(root_store)
    });
    // A hashmap of tls client configs for different configurations.
    // There won't need to be a lot of variation among these, and we'll want to
    // reuse them as much as we can, which is why we initialize them all once
    // and then pull out the one we need.
    // Disabling the lint because this is a local static that doesn't escape the
    // enclosing context. It fine.
    #[allow(clippy::type_complexity)]
    static CONFIGS: Lazy<Result<HashMap<Option<String>, Arc<ClientConfig>>, &'static io::Error>> =
        Lazy::new(|| {
            let root_store = ROOT_STORE.as_ref()?;
            Ok([None, Some("http2".to_string())]
                .into_iter()
                .map(|p| {
                    let mut config = ClientConfig::builder()
                        .with_root_certificates(root_store.clone())
                        .with_no_client_auth();
                    if let Some("http2") = p.as_deref() {
                        config
                            .alpn_protocols
                            .extend(["h2", "http/1.1"].iter().map(|s| s.as_bytes().to_vec()));
                    }
                    (p, Arc::new(config))
                })
                .collect())
        });

    let configs: &HashMap<Option<String>, Arc<ClientConfig>> = CONFIGS.as_ref().map_err(|e| *e)?;

    Ok(configs
        .get(&app_protocol)
        .or_else(|| configs.get(&None))
        .unwrap()
        .clone())
}

// Establish the connection to forward the tunnel stream to.
// Takes the tunnel and connection to make additional decisions on how to wrap
// the forwarded connection, i.e. reordering tls termination and proxyproto.
// Note: this additional wrapping logic currently unimplemented.
async fn connect(
    tunnel_tls: bool,
    app_protocol: Option<String>,
    proxy_proto_header: Option<ProxyHeader>,
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

    // We have to write the proxy header _before_ tls termination
    if let Some(header) = proxy_proto_header {
        conn = Box::new(
            proxy_proto::Stream::outgoing(conn, header)
                .expect("re-serializing proxy header should always succeed"),
        )
    };

    if backend_tls && !tunnel_tls {
        let domain = pki_types::ServerName::try_from(host)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?
            .to_owned();
        conn = Box::new(
            futures_rustls::TlsConnector::from(tls_config(app_protocol).map_err(|e| e.kind())?)
                .connect(domain, conn.compat())
                .await?
                .compat(),
        )
    }

    // TODO: header rewrites?

    Ok(conn)
}

async fn connect_tcp(host: &str, port: u16) -> Result<TcpStream, io::Error> {
    let conn = TcpStream::connect(&format!("{}:{}", host, port)).await?;
    if let Ok(addr) = conn.peer_addr() {
        Span::current().record("forward_addr", field::display(addr));
    }
    Ok(conn)
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
