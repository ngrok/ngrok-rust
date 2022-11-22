use std::{
    collections::HashMap,
    env,
    io,
    num::{ParseIntError, TryFromIntError},
    sync::Arc,
    time::Duration,
};

use async_rustls::{
    rustls,
    webpki,
};
use muxado::heartbeat::HeartbeatConfig;
use thiserror::Error;
use tokio::{
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
            AuthResp,
            BindOpts,
        },
        raw_session::RawSession,
    },
    Conn,
    Tunnel,
};

const CERT_BYTES: &[u8] = include_bytes!("../assets/ngrok.ca.crt");
const NOT_IMPLEMENTED: &str = "the agent has not defined a callback for this operation";

pub struct Session {
	#[allow(dead_code)]
    authresp: AuthResp,
    raw: Arc<Mutex<RawSession>>,
    tunnels: Arc<RwLock<HashMap<String, Sender<anyhow::Result<Conn>>>>>,
}

#[derive(Clone)]
pub struct SessionBuilder {
    authtoken: Option<String>,
    metadata: Option<String>,
    heartbeat_interval: Option<Duration>,
    heartbeat_tolerance: Option<Duration>,
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
    #[error("error configuring ngrok session: {0}")]
    SessionConfig(TryFromIntError),
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
            metadata: None,
            heartbeat_interval: None,
            heartbeat_tolerance: None,
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

    /// Set the heartbeat interval for the session.
    /// This value determines how often we send application level
    /// heartbeats to the server go check connection liveness.
    pub fn with_heartbeat_interval(&mut self, heartbeat_interval: Duration) -> &mut Self {
        self.heartbeat_interval = Some(heartbeat_interval.into());
        self
    }

    /// Set the heartbeat tolerance for the session.
    /// If the session's heartbeats are outside of their interval by this duration,
    /// the server will assume the session is dead and close it.
    pub fn with_heartbeat_tolerance(&mut self, heartbeat_tolerance: Duration) -> &mut Self {
        self.heartbeat_tolerance = Some(heartbeat_tolerance.into());
        self
    }

    /// Use the provided opaque metadata string for this session.
    /// Viewable from the ngrok dashboard or API.
    pub fn with_metadata(&mut self, metadata: impl Into<String>) -> &mut Self {
        self.metadata = Some(metadata.into());
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

        let mut heartbeat_config = HeartbeatConfig::<fn(Duration)>::default();
        if let Some(interval) = self.heartbeat_interval {
            heartbeat_config.interval = interval;
        }
        if let Some(tolerance) = self.heartbeat_tolerance {
            heartbeat_config.tolerance = tolerance;
        }
        // convert these while we have ownership
        let heartbeat_interval = i64::try_from(heartbeat_config.interval.as_nanos())
        .map_err(ConnectError::SessionConfig)?;
        let heartbeat_tolerance = i64::try_from(heartbeat_config.tolerance.as_nanos())
        .map_err(ConnectError::SessionConfig)?;

        let mut raw = RawSession::connect(
            tls_conn.compat(),
            heartbeat_config,
        )
        .await
        .map_err(ConnectError::Session)?;

        // list of possibilities: https://doc.rust-lang.org/std/env/consts/constant.OS.html
        let os = match env::consts::OS {
            "macos" => "darwin",
            _ => env::consts::OS
        };

        let resp = raw
            .auth(
                "",
                AuthExtra {
                    version: env!("CARGO_PKG_VERSION").into(),
                    auth_token: self.authtoken.clone().unwrap_or_default(),
                    metadata: self.metadata.clone().unwrap_or_default(),
                    os: os.into(),
                    arch: std::env::consts::ARCH.into(),
                    heartbeat_interval: heartbeat_interval,
                    heartbeat_tolerance: heartbeat_tolerance,
                    restart_unsupported_error: Some(NOT_IMPLEMENTED.into()),
                    stop_unsupported_error: Some(NOT_IMPLEMENTED.into()),
                    update_unsupported_error: Some(NOT_IMPLEMENTED.into()),
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

        Ok(Session {
            authresp: resp,
            raw,
            tunnels,
        })
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
                "rust",
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
