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
use muxado::heartbeat::HeartbeatConfig;
use ngrok::{
    internals::{
        proto::{
            AuthExtra,
            BindOpts,
        },
        raw_session::RawSession,
    },
    Session,
};
use tokio::io::{
    self,
    AsyncBufReadExt,
    AsyncReadExt,
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

    let mut sess = Session::new().with_authtoken_from_env().connect().await?;

    let mut tunnel = sess
        .start_tunnel()
        .await?;

    loop {
        let stream = if let Some(stream) = tunnel.accept().await? {
            stream
        } else {
            break;
        };

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
                println!("read line: {}", buf);
                tx.write_all(buf.as_bytes()).await?;
                println!("echoed line");
                tx.flush().await?;
                println!("flushed");
            }

            Result::<(), anyhow::Error>::Ok(())
        });
    }

    Ok(())
}
