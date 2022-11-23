use std::{
    collections::HashMap,
    ops::{
        Deref,
        DerefMut,
    },
    time::Duration,
};

use anyhow::{
    bail,
    Error,
};
use muxado::{
    heartbeat::HeartbeatConfig,
    session::SessionBuilder,
    typed::{
        AcceptTypedStream,
        OpenTypedStream,
        TypedSession,
        TypedStream,
    },
};
use tokio::io::{
    AsyncRead,
    AsyncReadExt,
    AsyncWrite,
    AsyncWriteExt,
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
    rpc::RPCRequest,
};

pub struct RpcClient {
    open: Box<dyn OpenTypedStream + Send>,
}

pub struct IncomingStreams {
    accept: Box<dyn AcceptTypedStream + Send>,
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
    pub async fn connect<S, F>(io_stream: S, heartbeat: HeartbeatConfig<F>) -> Result<Self, Error>
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
    pub async fn accept(&mut self) -> Result<TunnelStream, Error> {
        self.incoming.accept().await
    }

    pub fn split(self) -> (RpcClient, IncomingStreams) {
        (self.client, self.incoming)
    }
}

impl RpcClient {
    async fn rpc<R: RPCRequest>(&mut self, req: R) -> Result<R::Response, Error> {
        let mut stream = self.open.open_typed(R::TYPE).await?;
        let s = serde_json::to_vec(&req)?;
        stream.write_all(&*s).await?;
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).await?;

        Ok(serde_json::from_slice(&*buf)?)
    }

    pub async fn auth(
        &mut self,
        id: impl Into<String>,
        extra: AuthExtra,
    ) -> Result<AuthResp, Error> {
        let id = id.into();
        let req = Auth {
            client_id: id.clone(),
            extra,
            version: vec![VERSION.into()],
        };

        let resp = self.rpc(req).await?;

        Ok(resp)
    }

    pub async fn listen(
        &mut self,
        protocol: impl Into<String>,
        opts: BindOpts,
        extra: BindExtra,
        id: impl Into<String>,
        forwards_to: impl Into<String>,
    ) -> Result<BindResp, Error> {
        let req = Bind {
            client_id: id.into(),
            proto: protocol.into(),
            forwards_to: forwards_to.into(),
            opts,
            extra,
        };

        self.rpc(req).await
    }

    #[allow(dead_code)]
    pub async fn listen_label(
        &mut self,
        labels: HashMap<String, String>,
        metadata: impl Into<String>,
        forwards_to: impl Into<String>,
    ) -> Result<StartTunnelWithLabelResp, Error> {
        let req = StartTunnelWithLabel {
            labels,
            metadata: metadata.into(),
            forwards_to: forwards_to.into(),
        };

        self.rpc(req).await
    }

    pub async fn unlisten(&mut self, id: impl Into<String>) -> Result<UnbindResp, Error> {
        self.rpc(Unbind {
            client_id: id.into(),
        })
        .await
    }
}

impl IncomingStreams {
    pub async fn accept(&mut self) -> Result<TunnelStream, Error> {
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
                t => bail!("invalid stream type: {}", t),
            }
        })
    }
}

pub struct TunnelStream {
    pub header: ProxyHeader,
    pub stream: TypedStream,
}
