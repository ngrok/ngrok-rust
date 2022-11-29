use std::net::SocketAddr;

use axum::{
    extract::ConnectInfo,
    routing::get,
    Router,
};
use ngrok::{
    HTTPEndpoint,
    Session,
    Tunnel,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // build our application with a single route
    let app = Router::new().route(
        "/",
        get(
            |ConnectInfo(remote_addr): ConnectInfo<SocketAddr>| async move {
                format!("Hello, {:?}!\r\n", remote_addr)
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

async fn start_tunnel() -> anyhow::Result<Tunnel> {
    let sess = Session::builder()
        .with_authtoken_from_env()
        .connect()
        .await?;

    let tun = sess.start_tunnel(HTTPEndpoint::default()).await?;

    println!("Tunnel started on URL: {:?}", tun.url());

    Ok(tun)
}
