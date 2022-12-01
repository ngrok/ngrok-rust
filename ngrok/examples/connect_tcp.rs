use std::sync::Arc;

use futures::TryStreamExt;
use ngrok::{
    common::ProxyProto,
    Session,
    TCPEndpoint,
    Tunnel,
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

    let sess = Arc::new(
        Session::builder()
            .with_authtoken_from_env()
            .with_metadata("Online in One Line")
            .connect()
            .await?,
    );

    let tunnel = sess
        .start_tunnel(
            TCPEndpoint::default()
                .with_allow_cidr_string("0.0.0.0/0")
                .with_deny_cidr_string("10.1.1.1/32")
                .with_proxy_proto(ProxyProto::None)
                .with_metadata("Understand it so thoroughly that you merge with it")
                // .with_remote_addr("<n>.tcp.ngrok.io:<p>")
                .with_forwards_to("moo"),
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

            let sess = sess.clone();
            let id: String = tunnel.id().into();

            tokio::spawn(async move {
                info!("accepted connection: {:?}", stream.remote_addr());
                let (rx, mut tx) = io::split(stream);

                let mut lines = BufReader::new(rx);

                loop {
                    let mut buf = String::new();
                    let len = lines.read_line(&mut buf).await?;
                    if len == 0 {
                        break;
                    }

                    if buf.contains("bye!") {
                        info!("unbind requested");
                        tx.write_all("later!".as_bytes()).await?;
                        sess.close_tunnel(id).await?;
                        return Ok(());
                    } else if buf.contains("another!") {
                        info!("another requested");
                        let new_tunnel = sess.start_tunnel(TCPEndpoint::default()).await?;
                        tx.write_all(new_tunnel.url().as_bytes()).await?;
                        handle_tunnel(new_tunnel, sess.clone());
                    } else {
                        info!("read line: {}", buf);
                        tx.write_all(buf.as_bytes()).await?;
                        info!("echoed line");
                    }
                    tx.flush().await?;
                    info!("flushed");
                }

                Result::<(), anyhow::Error>::Ok(())
            });
        }
        anyhow::Result::<()>::Ok(())
    });
}
