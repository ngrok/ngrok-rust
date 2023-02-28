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

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::fmt()
        .pretty()
        .with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into()))
        .init();

    let forwards_to = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("missing forwarding address"))?;

    loop {
        let (stop_tx, stop_rx) = oneshot::channel();
        let stop_tx = Arc::new(Mutex::new(Some(stop_tx)));

        let (restart_tx, restart_rx) = oneshot::channel();
        let restart_tx = Arc::new(Mutex::new(Some(restart_tx)));

        let mut tun = ngrok::Session::builder()
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
            .connect()
            .await?
            .http_endpoint()
            .forwards_to(&forwards_to)
            .listen()
            .await?;

        info!(url = tun.url(), forwards_to, "started tunnel");

        let mut fut = if forwards_to.contains('/') {
            tun.forward_pipe(&forwards_to)
        } else {
            tun.forward_http(&forwards_to)
        }
        .fuse();

        let mut stop_rx = stop_rx.fuse();
        let mut restart_rx = restart_rx.fuse();

        select! {
            res = fut => return Ok(res?),
            _ = stop_rx => return Ok(()),
            _ = restart_rx => {
                drop(fut);
                let _ = tun.close().await;
                continue
            },
        }
    }
}
