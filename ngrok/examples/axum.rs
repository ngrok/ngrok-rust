use std::net::SocketAddr;

use axum::{
    extract::ConnectInfo,
    routing::get,
    Router,
};
use ngrok::{
    config::HTTPEndpoint,
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

// const CA_CERT: &[u8] = include_bytes!("ca.crt");

async fn start_tunnel() -> anyhow::Result<Tunnel> {
    let sess = Session::builder()
        .with_authtoken_from_env()
        .connect()
        .await?;

    let tun = sess
        .start_tunnel(
            HTTPEndpoint::default()
                // .with_allow_cidr_string("0.0.0.0/0")
                // .with_basic_auth("ngrok", "online1line")
                // .with_circuit_breaker(0.5)
                // .with_compression()
                // .with_deny_cidr_string("10.1.1.1/32")
                // .with_domain("<somedomain>.ngrok.io")
                // .with_mutual_tlsca(CA_CERT.into())
                // .with_oauth(
                //     OauthOptions::new("google")
                //         .with_allow_email("<user>@<domain>")
                //         .with_allow_domain("<domain>")
                //         .with_scope("<scope>"),
                // )
                // .with_oidc(
                //     OidcOptions::new("<url>", "<id>", "<secret>")
                //         .with_allow_email("<user>@<domain>")
                //         .with_allow_domain("<domain>")
                //         .with_scope("<scope>"),
                // )
                // .with_proxy_proto(ProxyProto::None)
                // .with_remove_request_header("X-Req-Nope")
                // .with_remove_response_header("X-Res-Nope")
                // .with_request_header("X-Req-Yup", "true")
                // .with_response_header("X-Res-Yup", "true")
                // .with_scheme(ngrok::Scheme::HTTPS)
                // .with_websocket_tcp_conversion()
                // .with_webhook_verification("twilio", "asdf"),
                .with_metadata("example tunnel metadata from rust"),
        )
        .await?;

    println!("Tunnel started on URL: {:?}", tun.url());

    Ok(tun)
}
