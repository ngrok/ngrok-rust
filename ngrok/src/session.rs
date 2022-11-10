use std::{
    collections::HashMap,
    env,
    io,
    net::SocketAddr,
    num::ParseIntError,
    ops::{
        Deref,
        DerefMut,
    },
    sync::Arc,
    time::Duration,
};

use async_rustls::{
    rustls,
    webpki,
};
use futures::{
    future::poll_fn,
    pin_mut,
    Future,
    FutureExt,
};
use muxado::heartbeat::HeartbeatConfig;
use thiserror::Error;
use tokio::{
    net::ToSocketAddrs,
    sync::{
        mpsc::{
            channel,
            Sender,
        },
        Mutex,
        RwLock,
    },
    time::timeout,
};
use tokio_util::compat::{
    FuturesAsyncReadCompatExt,
    TokioAsyncReadCompatExt,
};

use crate::{
    internals::{
        proto::{
            AuthExtra,
            BindOpts,
        },
        raw_session::RawSession,
    },
    Conn,
    Tunnel,
};

const CERT_BYTES: &[u8] = include_bytes!("../assets/ngrok.ca.crt");

pub struct Session {
    raw: Arc<Mutex<RawSession>>,
    tunnels: Arc<RwLock<HashMap<String, Sender<anyhow::Result<Conn>>>>>,
}

#[derive(Clone)]
pub struct SessionBuilder {
    authtoken: Option<String>,
    server_addr: (String, u16),
    tls_config: rustls::ClientConfig,
}

#[derive(Error, Debug)]
pub enum ConnectError {
    #[error("error establishing tcp connection: {0}")]
    Connect(io::Error),
    #[error("tls handshake error: {0}")]
    Tls(io::Error),
    #[error("error establishing ngrok session: {0}")]
    Session(anyhow::Error),
    #[error("error authenticating ngrok session: {0}")]
    Auth(anyhow::Error),
}

impl Default for SessionBuilder {
    fn default() -> Self {
        let mut root_store = rustls::RootCertStore::empty();
        let mut cert_pem = io::Cursor::new(CERT_BYTES);
        root_store
            .add_pem_file(&mut cert_pem)
            .expect("a valid ngrok root cert");

        let mut tls_config = rustls::ClientConfig::new();
        tls_config.root_store = root_store;

        SessionBuilder {
            authtoken: None,
            server_addr: ("tunnel.ngrok.com".into(), 443),
            tls_config,
        }
    }
}

#[derive(Debug, Error)]
#[error("invalid server address: {0}")]
pub struct InvalidAddrError(ParseIntError);

impl SessionBuilder {
    /// Authenticate the ngrok session with the given authtoken.
    pub fn with_authtoken(&mut self, authtoken: impl Into<String>) -> &mut Self {
        self.authtoken = Some(authtoken.into());
        self
    }

    /// Authenticate using the authtoken in the `NGROK_AUTHTOKEN` environment
    /// variable.
    pub fn with_authtoken_from_env(&mut self) -> &mut Self {
        self.authtoken = env::var("NGROK_AUTHTOKEN").ok();
        self
    }

    /// Connect to the provided ngrok server address.
    pub fn with_server_addr(
        &mut self,
        addr: impl AsRef<str>,
    ) -> Result<&mut Self, InvalidAddrError> {
        let addr = addr.as_ref();
        let mut split = addr.split(':');
        let host = split.next().unwrap().into();
        let port = split
            .next()
            .map(str::parse::<u16>)
            .transpose()
            .map_err(InvalidAddrError)?;
        self.server_addr = (host, port.unwrap_or(443));
        Ok(self)
    }

    /// Use the provided tls config when connecting to the ngrok server.
    pub fn with_tls_config(&mut self, config: rustls::ClientConfig) -> &mut Self {
        self.tls_config = config;
        self
    }

    /// Attempt to establish an ngrok session using the current configuration.
    pub async fn connect(&self) -> Result<Session, ConnectError> {
        let conn = tokio::net::TcpStream::connect(&self.server_addr)
            .await
            .map_err(ConnectError::Connect)?
            .compat();

        let tls_conn = async_rustls::TlsConnector::from(Arc::new(self.tls_config.clone()))
            .connect(
                webpki::DNSNameRef::try_from_ascii(self.server_addr.0.as_bytes()).unwrap(),
                conn,
            )
            .await
            .map_err(ConnectError::Tls)?;

        let mut raw = RawSession::connect(
            tls_conn.compat(),
            HeartbeatConfig::<fn(Duration)>::default(),
        )
        .await
        .map_err(ConnectError::Session)?;

        let resp = raw
            .auth(
                "",
                AuthExtra {
                    version: env!("CARGO_PKG_VERSION").into(),
                    auth_token: self.authtoken.clone().unwrap_or_default(),
                    client_type: "library/official/rust".into(),
                    ..Default::default()
                },
            )
            .await
            .map_err(ConnectError::Auth)?;

        let tunnels: Arc<RwLock<HashMap<String, Sender<Result<Conn, anyhow::Error>>>>> =
            Arc::new(Default::default());
        let raw = Arc::new(Mutex::new(raw));

        tokio::spawn({
            let tunnels = Arc::clone(&tunnels);
            let raw = Arc::clone(&raw);
            async move {
                loop {
                    let mut raw = raw.lock().await;
                    let conn = timeout(Duration::from_millis(10), raw.accept()).await;

                    match conn {
                        Ok(Ok(conn)) => {
                            let id = conn.header.id.clone();
                            let guard = RwLock::read(&tunnels).await;
                            let res = if let Some(ch) = guard.get(&id) {
                                ch.send(Ok(Conn { inner: conn })).await
                            } else {
                                Ok(())
                            };
                            drop(guard);
                            if res.is_err() {
                                RwLock::write(&tunnels).await.remove(&id);
                            }
                        }
                        Ok(Err(_e)) => break,
                        _ => continue, // timeout, yield to rpcs
                    }
                }
            }
        });

        Ok(Session { raw, tunnels })
    }
}

impl Session {
    pub fn new() -> SessionBuilder {
        SessionBuilder::default()
    }

    pub async fn start_tunnel(&self) -> anyhow::Result<Tunnel> {
        let resp = self
            .raw
            .lock()
            .await
            .listen(
                "tcp",
                BindOpts::TCPEndpoint(Default::default()),
                Default::default(),
                "",
                "nothing",
            )
            .await?;

        let (tx, rx) = channel(64);

        let mut tunnels = self.tunnels.write().await;
        tunnels.insert(resp.client_id.clone(), tx);

        Ok(Tunnel {
            sess: self.raw.clone(),
            info: resp,
            incoming: rx,
        })
    }

    pub async fn close_tunnel(&self, id: impl AsRef<str>) -> anyhow::Result<()> {
        let id = id.as_ref();
        self.raw.lock().await.unlisten(id).await?;
        self.tunnels.write().await.remove(id);
        Ok(())
    }
}
