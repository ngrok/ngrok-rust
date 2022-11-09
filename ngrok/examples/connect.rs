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
    proto::AuthExtra,
    raw_session::RawSession,
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

    let mut config = rustls::ClientConfig::new();

    config
        .dangerous()
        .set_certificate_verifier(Arc::new(NoVerify));

    let conn = tokio::net::TcpStream::connect("tunnel.ngrok.com:443")
        .await?
        .compat();

    let tls_conn = async_rustls::TlsConnector::from(Arc::new(config))
        .connect(
            webpki::DNSNameRef::try_from_ascii("tunnel.ngrok.com".as_bytes()).unwrap(),
            conn,
        )
        .await?;

    let mut sess = RawSession::connect(
        tls_conn.compat(),
        HeartbeatConfig::<fn(Duration)>::default(),
    )
    .await?;

    let resp = sess
        .auth(
            "1234",
            AuthExtra {
                version: "3.0.0".into(),
                auth_token: env::var("NGROK_AUTHTOKEN").unwrap_or_default(),
                ..Default::default()
            },
        )
        .await?;

    println!("{:#?}", resp);

    let resp = sess
        .listen(
            "tcp",
            ngrok::proto::BindOpts::TCPEndpoint(Default::default()),
            Default::default(),
            "1234",
            "nothing",
        )
        .await?;

    println!("{:#?}", resp);

    loop {
        let mut stream = sess.accept().await?;

        tokio::spawn(async move {
            println!("accepted stream: {:?}", stream.header);

            let (rx, mut tx) = io::split(&mut *stream.stream);

            let mut lines = BufReader::new(rx);

            loop {
                let mut buf = String::new();
                let len = lines.read_line(&mut buf).await?;
                if len == 0 {
                    break;
                }
                tx.write_all(buf.as_bytes()).await?;
                tx.flush().await?;
            }

            Result::<(), anyhow::Error>::Ok(())
        });
    }

    Ok(())
}
