use std::sync::Arc;

use futures::TryStreamExt;
use ngrok::{
    Session,
    Tunnel, HTTPEndpoint,
};
use tokio::io::{
    self,
    AsyncBufReadExt,
    AsyncWriteExt,
    BufReader,
};
use tracing::info;
use tracing_subscriber::fmt::format::FmtSpan;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .pretty()
        .with_span_events(FmtSpan::ENTER)
        .with_env_filter(std::env::var("RUST_LOG").unwrap_or_default())
        .init();

    let sess = Arc::new(Session::new()
        .with_authtoken_from_env()
        .with_metadata("Online in One Line")
        .connect()
        .await?);

    let tunnel = sess.start_tunnel(HTTPEndpoint::default()
        .with_metadata("Understand it so thoroughly that you merge with it")
        ).await?;

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

                    if buf.eq("\r\n".into()) {
                        info!("writing");
                        tx.write_all("HTTP/1.1 200 OK\r\n\r\n<html><body>hi</body></html>\r\n\r\n".as_bytes()).await?;
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
