use std::time::Duration;

use crate::proto::{self, Auth, AuthExtra, AuthResp, Bind, BindExtra, BindOpts, BindResp, VERSION};
use crate::rpc::RPCRequest;

use anyhow::Error;
use muxado::heartbeat::{Heartbeat, HeartbeatConfig};
use muxado::session::SessionBuilder;
use muxado::typed::{StreamType, TypedSession};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

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

        println!("buf: {:#?}", String::from_utf8_lossy(&buf));
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
}
