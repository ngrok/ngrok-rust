use std::env;

use muxado::{
    heartbeat::{
        Heartbeat,
        HeartbeatConfig,
    },
    session::*,
    typed::Typed,
};
use tokio::net::TcpStream;
use tracing_subscriber::{
    self,
    fmt::format::FmtSpan,
};

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    tracing_subscriber::fmt()
        .pretty()
        .with_span_events(FmtSpan::NEW | FmtSpan::CLOSE)
        .with_env_filter(env::var("RUST_LOG").unwrap_or("info".into()))
        .init();

    let conn = TcpStream::connect("localhost:1234").await?;

    let sess = SessionBuilder::new(conn).start();
    let typed = Typed::new(sess);
    let (_heartbeat, _heartbeat_ctl) = Heartbeat::start(
        typed,
        HeartbeatConfig {
            callback: Some(|d| {
                tracing::info!(?d, "got heartbeat");
            }),
            ..Default::default()
        },
    )
    .await?;

    let _: () = futures::future::pending().await;
    Ok(())
}
