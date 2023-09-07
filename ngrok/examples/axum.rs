use std::net::SocketAddr;

use axum::{
    extract::ConnectInfo,
    routing::get,
    Router,
};
use ngrok::prelude::*;

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

    let mut tun = ngrok::Session::builder()
        .authtoken_from_env()
        .connect()
        .await?
        .http_endpoint()
        // .allow_cidr("0.0.0.0/0")
        // .basic_auth("ngrok", "online1line")
        // .circuit_breaker(0.5)
        // .compression()
        // .deny_cidr("10.1.1.1/32")
        // .domain("<somedomain>.ngrok.io")
        // .forwards_to("example rust")
        // .mutual_tlsca(CA_CERT.into())
        // .oauth(
        //     OauthOptions::new("google")
        //         .allow_email("<user>@<domain>")
        //         .allow_domain("<domain>")
        //         .scope("<scope>"),
        // )
        // .oidc(
        //     OidcOptions::new("<url>", "<id>", "<secret>")
        //         .allow_email("<user>@<domain>")
        //         .allow_domain("<domain>")
        //         .scope("<scope>"),
        // )
        // .proxy_proto(ProxyProto::None)
        // .remove_request_header("X-Req-Nope")
        // .remove_response_header("X-Res-Nope")
        // .request_header("X-Req-Yup", "true")
        // .response_header("X-Res-Yup", "true")
        // .scheme(ngrok::Scheme::HTTPS)
        // .websocket_tcp_conversion()
        // .webhook_verification("twilio", "asdf"),
        .metadata("example tunnel metadata from rust")
        .listen_and_serve(app.into_make_service_with_connect_info::<SocketAddr>())
        .await?;

    println!("tunnel started on {url}", url = tun.url());

    tun.join().await??;

    Ok(())
}
