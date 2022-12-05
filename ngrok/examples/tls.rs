use std::net::SocketAddr;

use axum::{
    extract::ConnectInfo,
    routing::get,
    Router,
};
use ngrok::{
    Session,
    TLSEndpoint,
    Tunnel,
};

const CERT: &[u8] = include_bytes!("domain.crt");
const KEY: &[u8] = include_bytes!("domain.key");
// const CA_CERT: &[u8] = include_bytes!("ca.crt");

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

async fn start_tunnel() -> anyhow::Result<Tunnel> {
    let sess = Session::builder()
        .with_authtoken_from_env()
        .connect()
        .await?;

    let tun = sess
        .start_tunnel(
            TLSEndpoint::default()
                // .with_allow_cidr_string("0.0.0.0/0")
                // .with_deny_cidr_string("10.1.1.1/32")
                // .with_domain("<somedomain>.ngrok.io")
                // .with_forwards_to("example rust"),
                // .with_mutual_tlsca(CA_CERT.into())
                // .with_proxy_proto(ProxyProto::None)
                .with_cert_pem(CERT.into())
                .with_key_pem(KEY.into())
                .with_metadata("example tunnel metadata from rust"),
        )
        .await?;

    println!("Tunnel started on URL: {:?}", tun.url());

    Ok(tun)
}
