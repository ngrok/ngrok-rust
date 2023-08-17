use std::{
    collections::HashMap,
    fmt::Debug,
    future::Future,
    io,
    ops::{
        Deref,
        DerefMut,
    },
    sync::Arc,
};

use async_trait::async_trait;
use muxado::{
    heartbeat::{
        HeartbeatConfig,
        HeartbeatCtl,
    },
    typed::{
        StreamType,
        TypedAccept,
        TypedOpenClose,
        TypedSession,
        TypedStream,
    },
    Error as MuxadoError,
    SessionBuilder,
};
use serde::{
    de::DeserializeOwned,
    Deserialize,
};
use thiserror::Error;
use tokio::{
    io::{
        AsyncRead,
        AsyncReadExt,
        AsyncWrite,
        AsyncWriteExt,
    },
    runtime::Handle,
};
use tokio_util::either::Either;
use tracing::{
    debug,
    instrument,
    warn,
};

use super::{
    proto::{
        Auth,
        AuthExtra,
        AuthResp,
        Bind,
        BindExtra,
        BindOpts,
        BindResp,
        CommandResp,
        ErrResp,
        NgrokError,
        ProxyHeader,
        ReadHeaderError,
        Restart,
        StartTunnelWithLabel,
        StartTunnelWithLabelResp,
        Stop,
        Unbind,
        UnbindResp,
        Update,
        PROXY_REQ,
        RESTART_REQ,
        STOP_REQ,
        UPDATE_REQ,
        VERSION,
    },
    rpc::RpcRequest,
};

/// Errors arising from tunneling protocol RPC calls.
#[derive(Error, Debug)]
#[non_exhaustive]
pub enum RpcError {
    /// Failed to open a new stream to start the RPC call.
    #[error("failed to open muxado stream")]
    Open(#[source] MuxadoError),
    /// Some non-Open transport error occurred
    #[error("transport error")]
    Transport(#[source] MuxadoError),
    /// Failed to send the request over the stream.
    #[error("error sending rpc request")]
    Send(#[source] io::Error),
    /// Failed to read the RPC response from the stream.
    #[error("error reading rpc response")]
    Receive(#[source] io::Error),
    /// The RPC response was invalid.
    #[error("failed to deserialize rpc response")]
    InvalidResponse(#[from] serde_json::Error),
    /// There was an error in the RPC response.
    #[error("rpc error response:\n{0}")]
    Response(ErrResp),
}

impl NgrokError for RpcError {
    fn error_code(&self) -> Option<&str> {
        match self {
            RpcError::Response(resp) => resp.error_code(),
            _ => None,
        }
    }

    fn msg(&self) -> String {
        match self {
            RpcError::Response(resp) => resp.msg(),
            _ => format!("{self}"),
        }
    }
}

#[derive(Error, Debug)]
#[non_exhaustive]
pub enum StartSessionError {
    #[error("failed to start heartbeat task")]
    StartHeartbeat(#[from] io::Error),
}

#[derive(Error, Debug)]
#[non_exhaustive]
pub enum AcceptError {
    #[error("transport error when accepting connection")]
    Transport(#[from] MuxadoError),
    #[error(transparent)]
    Header(#[from] ReadHeaderError),
    #[error("invalid stream type: {0}")]
    InvalidType(StreamType),
}

pub struct RpcClient {
    // This is held so that the heartbeat task doesn't get shutdown. Eventually
    // we may use it to request heartbeats via the `Session`.
    #[allow(dead_code)]
    heartbeat: HeartbeatCtl,
    open: Box<dyn TypedOpenClose + Send>,
}

pub struct IncomingStreams {
    runtime: Handle,
    handlers: CommandHandlers,
    accept: Box<dyn TypedAccept + Send>,
}

pub struct RawSession {
    client: RpcClient,
    incoming: IncomingStreams,
}

impl Deref for RawSession {
    type Target = RpcClient;
    fn deref(&self) -> &Self::Target {
        &self.client
    }
}

impl DerefMut for RawSession {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.client
    }
}

/// Trait for a type that can handle a command from the ngrok dashboard.
#[async_trait]
pub trait CommandHandler<T>: Send + Sync + 'static {
    /// Handle the remote command.
    async fn handle_command(&self, req: T) -> Result<(), String>;
}

#[async_trait]
impl<R, T, F> CommandHandler<R> for T
where
    R: Send + 'static,
    T: Fn(R) -> F + Send + Sync + 'static,
    F: Future<Output = Result<(), String>> + Send,
{
    async fn handle_command(&self, req: R) -> Result<(), String> {
        self(req).await
    }
}

#[derive(Default, Clone)]
pub struct CommandHandlers {
    pub on_restart: Option<Arc<dyn CommandHandler<Restart>>>,
    pub on_update: Option<Arc<dyn CommandHandler<Update>>>,
    pub on_stop: Option<Arc<dyn CommandHandler<Stop>>>,
}

impl RawSession {
    pub async fn start<S, H>(
        io_stream: S,
        heartbeat: HeartbeatConfig,
        handlers: H,
    ) -> Result<Self, StartSessionError>
    where
        S: AsyncRead + AsyncWrite + Send + 'static,
        H: Into<Option<CommandHandlers>>,
    {
        let mux_sess = SessionBuilder::new(io_stream).start();

        let handlers = handlers.into().unwrap_or_default();

        let typed = muxado::typed::Typed::new(mux_sess);
        let (heartbeat, hbctl) = muxado::heartbeat::Heartbeat::start(typed, heartbeat).await?;
        let (open, accept) = heartbeat.split_typed();

        let runtime = Handle::current();

        let sess = RawSession {
            client: RpcClient {
                heartbeat: hbctl,
                open: Box::new(open),
            },
            incoming: IncomingStreams {
                runtime,
                handlers,
                accept: Box::new(accept),
            },
        };

        Ok(sess)
    }

    #[allow(dead_code)]
    pub async fn accept(&mut self) -> Result<TunnelStream, AcceptError> {
        self.incoming.accept().await
    }

    pub fn split(self) -> (RpcClient, IncomingStreams) {
        (self.client, self.incoming)
    }
}

impl RpcClient {
    #[instrument(level = "debug", skip(self))]
    async fn rpc<R: RpcRequest>(&mut self, req: R) -> Result<R::Response, RpcError> {
        let mut stream = self
            .open
            .open_typed(R::TYPE)
            .await
            .map_err(RpcError::Open)?;
        let s = serde_json::to_string(&req)
            // This should never happen, since we control the request types and
            // know that they will always serialize correctly. Just in case
            // though, call them "Send" errors.
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))
            .map_err(RpcError::Send)?;

        stream
            .write_all(s.as_bytes())
            .await
            .map_err(RpcError::Send)?;

        let mut buf = Vec::new();
        stream
            .read_to_end(&mut buf)
            .await
            .map_err(RpcError::Receive)?;

        #[derive(Debug, Deserialize)]
        struct ErrResp {
            #[serde(rename = "Error")]
            error: String,
        }

        let ok_resp = serde_json::from_slice::<R::Response>(&buf);
        let err_resp = serde_json::from_slice::<ErrResp>(&buf);

        if let Ok(err) = err_resp {
            if !err.error.is_empty() {
                debug!(?err, "decoded rpc error response");
                return Err(RpcError::Response(err.error.as_str().into()));
            }
        }

        debug!(resp = ?ok_resp, "decoded rpc response");

        Ok(ok_resp?)
    }

    /// Close the raw ngrok session with a "None" muxado error.
    pub async fn close(&mut self) -> Result<(), RpcError> {
        self.open
            .close(MuxadoError::None, "".into())
            .await
            .map_err(RpcError::Transport)?;
        Ok(())
    }

    #[instrument(level = "debug", skip(self))]
    pub async fn auth(
        &mut self,
        id: impl Into<String> + Debug,
        extra: AuthExtra,
    ) -> Result<AuthResp, RpcError> {
        let id = id.into();
        let req = Auth {
            client_id: id.clone(),
            extra,
            version: vec![VERSION.into()],
        };

        let resp = self.rpc(req).await?;

        Ok(resp)
    }

    #[instrument(level = "debug", skip(self))]
    pub async fn listen(
        &mut self,
        protocol: impl Into<String> + Debug,
        opts: BindOpts,
        extra: BindExtra,
        id: impl Into<String> + Debug,
        forwards_to: impl Into<String> + Debug,
    ) -> Result<BindResp<BindOpts>, RpcError> {
        // Sorry, this is awful. Serde untagged unions are pretty fraught and
        // hard to debug, so we're using this macro to specialize this call
        // based on the enum variant. It drops down to the type wrapped in the
        // enum for the actual request/response, and then re-wraps it on the way
        // back out in the same variant.
        // It's probably an artifact of the go -> rust translation, and could be
        // fixed with enough refactoring and rearchitecting. But it works well
        // enough for now and is pretty localized.
        macro_rules! match_variant {
            ($v:expr, $($var:tt),*) => {
                match opts {
                    $(BindOpts::$var (opts) => {
                        let req = Bind {
                            client_id: id.into(),
                            proto: protocol.into(),
                            forwards_to: forwards_to.into(),
                            opts,
                            extra,
                        };

                        let resp = self.rpc(req).await?;
                        BindResp {
                            bind_opts: BindOpts::$var(resp.bind_opts),
                            client_id: resp.client_id,
                            url: resp.url,
                            extra: resp.extra,
                            proto: resp.proto,
                        }
                    })*
                }
            };
        }
        Ok(match_variant!(opts, Http, Tcp, Tls))
    }

    #[instrument(level = "debug", skip(self))]
    pub async fn listen_label(
        &mut self,
        labels: HashMap<String, String>,
        metadata: impl Into<String> + Debug,
        forwards_to: impl Into<String> + Debug,
    ) -> Result<StartTunnelWithLabelResp, RpcError> {
        let req = StartTunnelWithLabel {
            labels,
            metadata: metadata.into(),
            forwards_to: forwards_to.into(),
        };

        self.rpc(req).await
    }

    #[instrument(level = "debug", skip(self))]
    pub async fn unlisten(
        &mut self,
        id: impl Into<String> + Debug,
    ) -> Result<UnbindResp, RpcError> {
        self.rpc(Unbind {
            client_id: id.into(),
        })
        .await
    }
}

pub const NOT_IMPLEMENTED: &str = "the agent has not defined a callback for this operation";

async fn handle_req<T>(
    handler: Option<Arc<dyn CommandHandler<T>>>,
    mut stream: TypedStream,
) -> Result<(), Either<io::Error, serde_json::Error>>
where
    T: DeserializeOwned + Debug + 'static,
{
    let res = async {
        debug!("reading request from stream");
        let mut buf = vec![];
        let req = serde_json::from_value(loop {
            let mut tmp = vec![0u8; 256];
            let bytes = stream.read(&mut tmp).await.map_err(Either::Left)?;
            buf.extend_from_slice(&tmp[..bytes]);

            if let Ok(obj) = serde_json::from_slice::<serde_json::Value>(&buf) {
                break obj;
            }
        })
        .map_err(Either::Right)?;
        debug!(?req, "read request from stream");

        let resp = if let Some(handler) = handler {
            debug!("running command handler");
            handler.handle_command(req).await.err()
        } else {
            Some(NOT_IMPLEMENTED.into())
        };

        debug!(?resp, "writing response to stream");

        let resp_json = serde_json::to_vec(&CommandResp { error: resp }).map_err(Either::Right)?;

        stream
            .write_all(resp_json.as_slice())
            .await
            .map_err(Either::Left)?;

        Ok(())
    }
    .await;

    if let Err(e) = &res {
        warn!(?e, "error when handling dashboard command");
    }

    res
}

impl IncomingStreams {
    pub async fn accept(&mut self) -> Result<TunnelStream, AcceptError> {
        Ok(loop {
            let mut stream = self.accept.accept_typed().await?;

            match stream.typ() {
                RESTART_REQ => {
                    self.runtime
                        .spawn(handle_req(self.handlers.on_restart.clone(), stream));
                }
                UPDATE_REQ => {
                    self.runtime
                        .spawn(handle_req(self.handlers.on_update.clone(), stream));
                }
                STOP_REQ => {
                    self.runtime
                        .spawn(handle_req(self.handlers.on_stop.clone(), stream));
                }
                PROXY_REQ => {
                    let header = ProxyHeader::read_from_stream(&mut *stream).await?;

                    break TunnelStream { header, stream };
                }
                t => return Err(AcceptError::InvalidType(t)),
            }
        })
    }
}

pub struct TunnelStream {
    pub header: ProxyHeader,
    pub stream: TypedStream,
}
