use std::{
    env,
    os,
    sync::Arc,
    time::Duration,
};

use async_rustls::{
    rustls::{
        self,
        ServerCertVerified,
    },
    webpki,
};
use futures::{
    Stream,
    TryStreamExt,
};
use muxado::heartbeat::HeartbeatConfig;
use ngrok::{
    internals::{
        proto::{
            AuthExtra,
            BindOpts,
        },
        raw_session::RawSession,
    },
    Conn,
    Session,
    Tunnel,
};
use tokio::io::{
    self,
    AsyncBufReadExt,
    AsyncRead,
    AsyncReadExt,
    AsyncWrite,
    AsyncWriteExt,
    BufReader,
};
use tokio_util::compat::{
    FuturesAsyncReadCompatExt,
    TokioAsyncReadCompatExt,
};
use tracing_subscriber::fmt::format::FmtSpan;

struct NoVerify;

impl rustls::ServerCertVerifier for NoVerify {
    fn verify_server_cert(
        &self,
        _roots: &rustls::RootCertStore,
        _presented_certs: &[rustls::Certificate],
        _dns_name: webpki::DNSNameRef<'_>,
        _ocsp_response: &[u8],
    ) -> Result<ServerCertVerified, rustls::TLSError> {
        Ok(ServerCertVerified::assertion())
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .pretty()
        .with_span_events(FmtSpan::ENTER)
        .with_env_filter(std::env::var("RUST_LOG").unwrap_or_default())
        .init();

    let sess = Arc::new(Session::new().with_authtoken_from_env().connect().await?);

    let tunnel = sess.start_tunnel().await?;

    handle_tunnel(tunnel, sess);

    futures::future::pending().await
}

fn handle_tunnel(mut tunnel: Tunnel, sess: Arc<Session>) {
    tokio::spawn(async move {
        loop {
            let stream = if let Some(stream) = tunnel.try_next().await? {
                stream
            } else {
                break;
            };

            let sess = sess.clone();
            let id: String = tunnel.id().into();

            tokio::spawn(async move {
                println!("accepted connection: {:?}", stream.header());
                let (rx, mut tx) = io::split(stream);

                let mut lines = BufReader::new(rx);

                loop {
                    let mut buf = String::new();
                    let len = lines.read_line(&mut buf).await?;
                    if len == 0 {
                        break;
                    }

                    if buf.contains("bye!") {
                        println!("unbind requested");
                        tx.write_all("later!".as_bytes()).await?;
                        sess.close_tunnel(id).await?;
                        return Ok(());
                    } else if buf.contains("another!") {
                        println!("another requested");
                        let new_tunnel = sess.start_tunnel().await?;
                        tx.write_all(new_tunnel.url().as_bytes()).await?;
                        handle_tunnel(new_tunnel, sess.clone());
                    } else {
                        println!("read line: {}", buf);
                        tx.write_all(buf.as_bytes()).await?;
                        println!("echoed line");
                    }
                    tx.flush().await?;
                    println!("flushed");
                }

                Result::<(), anyhow::Error>::Ok(())
            });
        }
        anyhow::Result::<()>::Ok(())
    });
}
