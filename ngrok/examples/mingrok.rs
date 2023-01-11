use anyhow::Error;
use ngrok::prelude::*;
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

    let mut tun = ngrok::Session::builder()
        .authtoken_from_env()
        .connect()
        .await?
        .http_endpoint()
        .forwards_to(&forwards_to)
        .listen()
        .await?;

    info!(url = tun.url(), forwards_to, "started tunnel");

    let fut = if forwards_to.contains('/') {
        tun.forward_unix(forwards_to)
    } else {
        tun.forward_http(forwards_to)
    };
    Ok(fut.await?)
}
