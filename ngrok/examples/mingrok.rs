use anyhow::Error;
use ngrok::Upstream;
use tracing::info;

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::fmt()
        .pretty()
        .with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into()))
        .init();

    let forwards_to = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("missing forwarding address"))?;

    let mut fwd = ngrok::forward(Upstream::new(&forwards_to))
        .start()
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))?;

    info!(url = fwd.url(), %forwards_to, "started forwarder");

    match fwd.join().await {
        Ok(Ok(())) => info!("forwarder exited cleanly"),
        Ok(Err(e)) => info!("forwarder error: {}", e),
        Err(e) => info!("forwarder task panicked: {}", e),
    }

    Ok(())
}
