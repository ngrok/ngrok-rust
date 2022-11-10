use std::{
    collections::HashMap,
    time::Duration,
};

use anyhow::{Error, bail};
use muxado::{
    heartbeat::HeartbeatConfig,
    session::SessionBuilder,
    typed::{
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
        RESTART_REQ,
        STOP_REQ,
        UPDATE_REQ,
        VERSION, PROXY_REQ,
    },
    rpc::RPCRequest,
};

pub struct RawSession {
    id: String,
    inner: Box<dyn TypedSession>,
}

impl RawSession {
    pub async fn connect<S, F>(io_stream: S, heartbeat: HeartbeatConfig<F>) -> Result<Self, Error>
    where
        S: AsyncRead + AsyncWrite + Send + 'static,
        F: FnMut(Duration) + Send + 'static,
    {
        let mux_sess = SessionBuilder::new(io_stream).start();

        let typed = muxado::typed::Typed::new(mux_sess);
        let mut heartbeat = muxado::heartbeat::Heartbeat::new(typed, heartbeat);
        heartbeat.start().await?;

        let sess = RawSession {
            id: String::new(),
            inner: Box::new(heartbeat),
        };

        Ok(sess)
    }

    async fn rpc<R: RPCRequest>(&mut self, req: R) -> Result<R::Response, Error> {
        let mut stream = self.inner.open_typed(R::TYPE).await?;
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
        if resp.client_id.is_some() && resp.client_id != Some(id) {
            self.id = resp.client_id.as_ref().unwrap().clone();
        }

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

    pub async fn accept(&mut self) -> Result<TunnelStream, Error> {
        Ok(loop {
            let mut stream = self.inner.accept_typed().await?;

            match stream.typ() {
                RESTART_REQ => {}
                STOP_REQ => {}
                UPDATE_REQ => {}
                PROXY_REQ => {
                    let header = ProxyHeader::read_from_stream(&mut *stream).await?;

                    break TunnelStream { header, stream };
                },
                t => bail!("invalid stream type: {}", t),
            }
        })
    }
}

pub struct TunnelStream {
    pub header: ProxyHeader,
    pub stream: TypedStream,
}
