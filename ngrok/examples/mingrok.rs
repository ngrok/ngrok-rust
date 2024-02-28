use std::sync::{
    Arc,
    Mutex,
};

use anyhow::Error;
use futures::{
    prelude::*,
    select,
};
use ngrok::prelude::*;
use tokio::sync::oneshot;
use tracing::info;
use url::Url;

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::fmt()
        .pretty()
        .with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into()))
        .init();

    let forwards_to = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("missing forwarding address"))
        .and_then(|s| Ok(Url::parse(&s)?))?;

    loop {
        let (stop_tx, stop_rx) = oneshot::channel();
        let stop_tx = Arc::new(Mutex::new(Some(stop_tx)));

        let (restart_tx, restart_rx) = oneshot::channel();
        let restart_tx = Arc::new(Mutex::new(Some(restart_tx)));

        let mut fwd = ngrok::Session::builder()
            .authtoken_from_env()
            .handle_stop_command(move |req| {
                let stop_tx = stop_tx.clone();
                async move {
                    info!(?req, "received stop command");
                    let _ = stop_tx.lock().unwrap().take().unwrap().send(());
                    Ok(())
                }
            })
            .handle_restart_command(move |req| {
                let restart_tx = restart_tx.clone();
                async move {
                    info!(?req, "received restart command");
                    let _ = restart_tx.lock().unwrap().take().unwrap().send(());
                    Ok(())
                }
            })
            .handle_update_command(|req| async move {
                info!(?req, "received update command");
                Err("unable to update".into())
            })
            .await?
            .http_endpoint()
            .listen_and_forward(forwards_to.clone())
            .await?;

        info!(url = fwd.url(), %forwards_to, "started forwarder");

        let mut fut = fwd.join().fuse();
        let mut stop_rx = stop_rx.fuse();
        let mut restart_rx = restart_rx.fuse();

        select! {
            res = fut => info!("{:?}", res?),
            _ = stop_rx => return Ok(()),
            _ = restart_rx => {
                drop(fut);
                let _ = fwd.close().await;
                continue
            },
        }
    }
}
