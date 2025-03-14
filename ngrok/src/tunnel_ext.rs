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

use async_trait::async_trait;
#[cfg(feature = "axum")]
use axum_core::response::Response;
use bitflags::bitflags;
use futures::stream::TryStreamExt;
use futures_rustls::rustls::{
    self,
    pki_types,
    ClientConfig,
};
#[cfg(feature = "hyper")]
use hyper::{
    server::conn::http1,
    service::service_fn,
    StatusCode,
};
use once_cell::sync::Lazy;
use proxy_protocol::ProxyHeader;
use rustls::crypto::ring as provider;
#[cfg(feature = "hyper")]
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
                self.inner.info.verify_upstream_tls,
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
            let verify_upstream_tls = self.inner.info.verify_upstream_tls;

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
                verify_upstream_tls,
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

fn tls_config(
    app_protocol: Option<String>,
    verify_upstream_tls: bool,
) -> Result<Arc<ClientConfig>, &'static io::Error> {
    // A hashmap of tls client configs for different configurations.
    // There won't need to be a lot of variation among these, and we'll want to
    // reuse them as much as we can, which is why we initialize them all once
    // and then pull out the one we need.
    // Disabling the lint because this is a local static that doesn't escape the
    // enclosing context. It fine.
    #[allow(clippy::type_complexity)]
    static CONFIGS: Lazy<Result<HashMap<u8, Arc<ClientConfig>>, &'static io::Error>> =
        Lazy::new(|| {
            std::ops::Range {
                start: 0,
                end: TlsFlags::FLAG_MAX.bits() + 1,
            }
            .map(|p| {
                let http2 = (p & TlsFlags::FLAG_HTTP2.bits()) != 0;
                let verify_upstream_tls = (p & TlsFlags::FLAG_verify_upstream_tls.bits()) != 0;
                let mut config = crate::session::host_certs_tls_config()?;
                if !verify_upstream_tls {
                    config.dangerous().set_certificate_verifier(Arc::new(
                        danger::NoCertificateVerification::new(provider::default_provider()),
                    ));
                }

                if http2 {
                    config
                        .alpn_protocols
                        .extend(["h2", "http/1.1"].iter().map(|s| s.as_bytes().to_vec()));
                }
                Ok((p, Arc::new(config)))
            })
            .collect()
        });

    let configs: &HashMap<u8, Arc<ClientConfig>> = CONFIGS.as_ref().map_err(|e| *e)?;
    let mut key = 0;
    if Some("http2").eq(&app_protocol.as_deref()) {
        key |= TlsFlags::FLAG_HTTP2.bits();
    }
    if verify_upstream_tls {
        key |= TlsFlags::FLAG_verify_upstream_tls.bits();
    }

    Ok(configs
        .get(&key)
        .or_else(|| configs.get(&0))
        .unwrap()
        .clone())
}

bitflags! {
    struct TlsFlags: u8 {
        const FLAG_HTTP2       = 0b01;
        const FLAG_verify_upstream_tls       = 0b10;
        const FLAG_MAX     = Self::FLAG_HTTP2.bits()
                           | Self::FLAG_verify_upstream_tls.bits();
    }
}

// Establish the connection to forward the tunnel stream to.
// Takes the tunnel and connection to make additional decisions on how to wrap
// the forwarded connection, i.e. reordering tls termination and proxyproto.
// Note: this additional wrapping logic currently unimplemented.
async fn connect(
    tunnel_tls: bool,
    verify_upstream_tls: bool,
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
            futures_rustls::TlsConnector::from(
                tls_config(app_protocol, verify_upstream_tls).map_err(|e| e.kind())?,
            )
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
    conn: impl hyper::rt::Read + hyper::rt::Write + Unpin + Send + 'static,
) -> JoinHandle<()> {
    tokio::spawn(
        async move {
            let service = service_fn(move |_req| {
                debug!("serving bad gateway error");
                let mut resp = Response::new(format!("failed to dial backend: {err}"));
                *resp.status_mut() = StatusCode::BAD_GATEWAY;
                futures::future::ok::<_, Infallible>(resp)
            });

            let res = http1::Builder::new()
                .keep_alive(false)
                .serve_connection(conn, service)
                .await;
            debug!(?res, "connection closed");
        }
        .in_current_span(),
    )
}

// https://github.com/rustls/rustls/blob/main/examples/src/bin/tlsclient-mio.rs#L334
mod danger {
    use futures_rustls::rustls;
    use rustls::{
        client::danger::HandshakeSignatureValid,
        crypto::{
            verify_tls12_signature,
            verify_tls13_signature,
            CryptoProvider,
        },
        DigitallySignedStruct,
    };

    use super::pki_types::{
        CertificateDer,
        ServerName,
        UnixTime,
    };

    #[derive(Debug)]
    pub struct NoCertificateVerification(CryptoProvider);

    impl NoCertificateVerification {
        pub fn new(provider: CryptoProvider) -> Self {
            Self(provider)
        }
    }

    impl rustls::client::danger::ServerCertVerifier for NoCertificateVerification {
        fn verify_server_cert(
            &self,
            _end_entity: &CertificateDer<'_>,
            _intermediates: &[CertificateDer<'_>],
            _server_name: &ServerName<'_>,
            _ocsp: &[u8],
            _now: UnixTime,
        ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
            Ok(rustls::client::danger::ServerCertVerified::assertion())
        }

        fn verify_tls12_signature(
            &self,
            message: &[u8],
            cert: &CertificateDer<'_>,
            dss: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, rustls::Error> {
            verify_tls12_signature(
                message,
                cert,
                dss,
                &self.0.signature_verification_algorithms,
            )
        }

        fn verify_tls13_signature(
            &self,
            message: &[u8],
            cert: &CertificateDer<'_>,
            dss: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, rustls::Error> {
            verify_tls13_signature(
                message,
                cert,
                dss,
                &self.0.signature_verification_algorithms,
            )
        }

        fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
            self.0.signature_verification_algorithms.supported_schemes()
        }
    }
}
