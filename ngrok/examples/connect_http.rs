use std::sync::Arc;

use futures::TryStreamExt;
use ngrok::{
    common::ProxyProto,
    oauth::OauthOptions,
    oidc::OidcOptions,
    HTTPEndpoint,
    Session,
    Tunnel,
};
use tokio::io::{
    self,
    AsyncBufReadExt,
    AsyncWriteExt,
    BufReader,
};
use tracing::{
    debug,
    info,
};
use tracing_subscriber::fmt::format::FmtSpan;

// const CA_CERT: &[u8] = include_bytes!("ca.crt");

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .pretty()
        .with_span_events(FmtSpan::ENTER)
        .with_env_filter(std::env::var("RUST_LOG").unwrap_or_default())
        .init();

    let sess = Arc::new(
        Session::builder()
            .with_authtoken_from_env()
            .with_metadata("Online in One Line")
            .connect()
            .await?,
    );

    let tunnel = sess
        .start_tunnel(
            HTTPEndpoint::default()
                .with_allow_cidr_string("0.0.0.0/0")
                .with_deny_cidr_string("10.1.1.1/32")
                .with_proxy_proto(ProxyProto::None)
                .with_metadata("Understand it so thoroughly that you merge with it")
                .with_scheme(ngrok::Scheme::HTTPS)
                // .with_domain("<somedomain>.ngrok.io")
                // .with_mutual_tlsca(CA_CERT.into())
                .with_compression()
                // .with_websocket_tcp_conversion()
                .with_circuit_breaker(0.5)
                .with_request_header("X-Req-Yup", "true")
                .with_response_header("X-Res-Yup", "true")
                .with_remove_request_header("X-Req-Nope")
                .with_remove_response_header("X-Res-Nope")
                // .with_oauth(OauthOptions::new("google"))
                // .with_oauth(
                //     OauthOptions::new("google")
                //         .with_allow_email("<user>@<domain>>")
                //         .with_allow_domain("<domain>>")
                //         //.with_scope("<scope>"),
                // )
                // .with_oidc(OidcOptions::new("<url>", "<id>", "<secret>"))
                // .with_oidc(
                //     OidcOptions::new("<url>", "<id>>", "<secret>")
                //         .with_allow_email("<user>@<domain>")
                //         .with_allow_domain("<domain>")
                //         .with_scope("<scope>"),
                // )
                // .with_webhook_verification("twilio", "asdf"),
                .with_basic_auth("ngrok", "online1line"),
        )
        .await?;

    handle_tunnel(tunnel, sess);

    futures::future::pending().await
}

fn handle_tunnel(mut tunnel: Tunnel, sess: Arc<Session>) {
    info!("bound new tunnel: {}", tunnel.url());
    tokio::spawn(async move {
        loop {
            let stream = if let Some(stream) = tunnel.try_next().await? {
                stream
            } else {
                info!("tunnel closed!");
                break;
            };

            let _sess = sess.clone();
            let _id: String = tunnel.id().into();

            tokio::spawn(async move {
                info!("accepted connection: {:?}", stream.header());
                let (rx, mut tx) = io::split(stream);

                let mut lines = BufReader::new(rx);

                loop {
                    let mut buf = String::new();
                    let len = lines.read_line(&mut buf).await?;
                    if len == 0 {
                        break;
                    }

                    debug!("received: {}", buf);

                    if buf.eq("\r\n") {
                        info!("writing");
                        tx.write_all(
                            "HTTP/1.1 200 OK\r\n\r\n<html><body>ngrok-rs</body></html>\r\n\r\n"
                                .as_bytes(),
                        )
                        .await?;
                        info!("done writing");
                        tx.flush().await?;
                        info!("connection shutdown");
                        tx.shutdown().await?;
                        break;
                    }
                }

                Result::<(), anyhow::Error>::Ok(())
            });
        }
        anyhow::Result::<()>::Ok(())
    });
}
