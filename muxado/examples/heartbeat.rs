use std::{
    env,
    sync::Arc,
    time::Duration,
};

use futures::future::{
    self,
    BoxFuture,
};
use muxado::{
    heartbeat::{
        Heartbeat,
        HeartbeatConfig,
        HeartbeatHandler,
    },
    typed::Typed,
    *,
};
use tokio::net::TcpStream;
use tracing_subscriber::fmt::format::FmtSpan;

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
            // Either approach to providing a handler works
            // handler: Some(Arc::new(HHandler)),
            handler: Some(Arc::new(|lat| async move {
                tracing::info!(?lat, "got heartbeat");
                Result::<(), BoxError>::Ok(())
            })),
            ..Default::default()
        },
    )
    .await?;

    futures::future::pending::<()>().await;
    Ok(())
}

struct HHandler;

impl HeartbeatHandler for HHandler {
    fn handle_heartbeat(&self, lat: Option<Duration>) -> BoxFuture<Result<(), BoxError>> {
        if let Some(lat) = lat {
            tracing::info!(?lat, "got heartbeat");
        }
        Box::pin(future::ok(()))
    }
}
