use std::{
    collections::HashMap,
    fmt::Debug,
    io,
    ops::{
        Deref,
        DerefMut,
    },
    time::Duration,
};

use muxado::{
    heartbeat::HeartbeatConfig,
    typed::{
        StreamType,
        TypedAccept,
        TypedOpen,
        TypedSession,
        TypedStream,
    },
    Error as MuxadoError,
    SessionBuilder,
};
use thiserror::Error;
use tokio::io::{
    AsyncRead,
    AsyncReadExt,
    AsyncWrite,
    AsyncWriteExt,
};
use tracing::{
    debug,
    instrument,
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
        ProxyHeader,
        ReadHeaderError,
        StartTunnelWithLabel,
        StartTunnelWithLabelResp,
        Unbind,
        UnbindResp,
        PROXY_REQ,
        RESTART_REQ,
        STOP_REQ,
        UPDATE_REQ,
        VERSION,
    },
    rpc::{
        RpcRequest,
        RpcResult,
    },
};

/// Errors arising from tunneling protocol RPC calls.
#[derive(Error, Debug)]
pub enum RpcError {
    /// Failed to open a new stream to start the RPC call.
    #[error("failed to open muxado stream")]
    Open(#[from] MuxadoError),
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
    #[error("rpc error response: {0}")]
    Response(String),
}

#[derive(Error, Debug)]
pub enum StartSessionError {
    #[error("failed to start heartbeat task")]
    StartHeartbeat(#[from] io::Error),
}

#[derive(Error, Debug)]
pub enum AcceptError {
    #[error("transport error when accepting connection")]
    Transport(#[from] MuxadoError),
    #[error(transparent)]
    Header(#[from] ReadHeaderError),
    #[error("invalid stream type: {0}")]
    InvalidType(StreamType),
}

pub struct RpcClient {
    open: Box<dyn TypedOpen + Send>,
}

pub struct IncomingStreams {
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

impl RawSession {
    pub async fn start<S, F>(
        io_stream: S,
        heartbeat: HeartbeatConfig<F>,
    ) -> Result<Self, StartSessionError>
    where
        S: AsyncRead + AsyncWrite + Send + 'static,
        F: FnMut(Duration) + Send + 'static,
    {
        let mux_sess = SessionBuilder::new(io_stream).start();

        let typed = muxado::typed::Typed::new(mux_sess);
        let (heartbeat, _) = muxado::heartbeat::Heartbeat::start(typed, heartbeat).await?;
        let (open, accept) = heartbeat.split_typed();

        let sess = RawSession {
            client: RpcClient {
                open: Box::new(open),
            },
            incoming: IncomingStreams {
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
        let mut stream = self.open.open_typed(R::TYPE).await?;
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

        debug!(
            resp_string = String::from_utf8_lossy(&buf).as_ref(),
            "read rpc response"
        );

        let resp: RpcResult<R::Response> = serde_json::from_slice(&buf)?;

        debug!(?resp, "decoded rpc response");

        resp.into()
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

impl IncomingStreams {
    pub async fn accept(&mut self) -> Result<TunnelStream, AcceptError> {
        Ok(loop {
            let mut stream = self.accept.accept_typed().await?;

            match stream.typ() {
                RESTART_REQ => {}
                STOP_REQ => {}
                UPDATE_REQ => {}
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
