use std::net::SocketAddr;

use axum::{
    extract::ConnectInfo,
    routing::get,
    Router,
};
use ngrok::prelude::*;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // build our application with a single route
    let app = Router::new().route(
        "/",
        get(
            |ConnectInfo(remote_addr): ConnectInfo<SocketAddr>| async move {
                format!("Hello, {remote_addr:?}!\r\n")
            },
        ),
    );

    // run it with hyper on localhost:8000
    // axum::Server::bind(&"0.0.0.0:8000".parse().unwrap())
    // Or with an ngrok tunnel
    axum::Server::builder(start_tunnel().await?)
        .serve(app.into_make_service_with_connect_info::<SocketAddr>())
        .await
        .unwrap();

    Ok(())
}

async fn start_tunnel() -> anyhow::Result<impl Tunnel> {
    let sess = ngrok::Session::builder()
        .authtoken_from_env()
        .connect()
        .await?;

    let tun = sess
        .labeled_tunnel()
        .label("edge", "edghts_<edge_id>")
        .metadata("example tunnel metadata from rust")
        .listen()
        .await?;

    println!("Labeled tunnel started!");

    Ok(tun)
}
