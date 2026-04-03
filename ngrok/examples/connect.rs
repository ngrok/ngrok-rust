use futures::TryStreamExt;
use ngrok::prelude::*;
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

    let agent = ngrok::Agent::builder()
        .authtoken_from_env()
        .metadata("Online in One Line")
        .build()?;

    let listener = agent
        .listen()
        .url("tcp://0.tcp.ngrok.io:0")
        .metadata("example tunnel metadata from rust")
        .start()
        .await?;

    handle_listener(listener, agent.clone());

    futures::future::pending().await
}

fn handle_listener(mut listener: ngrok::EndpointListener, agent: ngrok::Agent) {
    info!("bound new listener: {}", listener.url());
    tokio::spawn(async move {
        loop {
            let stream = if let Some(stream) = listener.try_next().await.map_err(|e| anyhow::anyhow!("{}", e))? {
                stream
            } else {
                info!("listener closed!");
                break;
            };

            let agent = agent.clone();

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
                        info!("close requested");
                        tx.write_all("later!".as_bytes()).await?;
                        return Ok(());
                    } else if buf.contains("another!") {
                        info!("another requested");
                        let new_listener = agent
                            .listen()
                            .url("tcp://0.tcp.ngrok.io:0")
                            .start()
                            .await
                            .map_err(|e| anyhow::anyhow!("{}", e))?;
                        tx.write_all(new_listener.url().as_bytes()).await?;
                        handle_listener(new_listener, agent.clone());
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
