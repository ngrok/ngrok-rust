use std::{
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
use muxado::heartbeat::HeartbeatConfig;
use thiserror::Error;
use tokio::net::ToSocketAddrs;
use tokio_util::compat::{
    FuturesAsyncReadCompatExt,
    TokioAsyncReadCompatExt,
};

use crate::internals::raw_session::RawSession;

const CERT_BYTES: &[u8] = include_bytes!("../assets/ngrok.ca.crt");

pub struct Session {
    raw: RawSession,
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
    pub async fn connect(self) -> Result<Session, ConnectError> {
        let conn = tokio::net::TcpStream::connect(&self.server_addr)
            .await
            .map_err(ConnectError::Connect)?
            .compat();

        let tls_conn = async_rustls::TlsConnector::from(Arc::new(self.tls_config))
            .connect(
                webpki::DNSNameRef::try_from_ascii(self.server_addr.0.as_bytes()).unwrap(),
                conn,
            )
            .await
            .map_err(ConnectError::Tls)?;

        let raw = RawSession::connect(
            tls_conn.compat(),
            HeartbeatConfig::<fn(Duration)>::default(),
        )
        .await
        .map_err(ConnectError::Session)?;

        Ok(Session { raw })
    }
}

impl Session {
    pub fn new() -> SessionBuilder {
        SessionBuilder::default()
    }
}

impl Deref for Session {
    type Target = RawSession;
    fn deref(&self) -> &Self::Target {
        &self.raw
    }
}

impl DerefMut for Session {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.raw
    }
}
