use std::net::SocketAddr;

use axum::{
    extract::ConnectInfo,
    routing::get,
    Router,
};
use ngrok::{
    prelude::*,
    tunnel::HttpTunnel,
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

async fn start_tunnel() -> anyhow::Result<HttpTunnel> {
    let tun = ngrok::Session::builder()
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
        // .policies(create_policies()?)
        // .proxy_proto(ProxyProto::None)
        // .remove_request_header("X-Req-Nope")
        // .remove_response_header("X-Res-Nope")
        // .request_header("X-Req-Yup", "true")
        // .response_header("X-Res-Yup", "true")
        // .scheme(ngrok::Scheme::HTTPS)
        // .websocket_tcp_conversion()
        // .webhook_verification("twilio", "asdf"),
        .metadata("example tunnel metadata from rust")
        .listen()
        .await?;

    println!("Tunnel started on URL: {:?}", tun.url());

    Ok(tun)
}

#[allow(dead_code)]
fn create_policies() -> Result<Policies, InvalidPolicies> {
    Ok(Policies::new()
        .add_inbound(
            Policy::new("deny_put")
                .add_expression("req.Method == 'PUT'")
                .add_action(Action::new("deny", None)?),
        )
        .add_outbound(
            Policy::new("200_response")
                .add_expression("res.StatusCode == '200'")
                .add_action(Action::new(
                    "custom-response",
                    Some(
                        r###"{
                    "status_code": 200,
                    "content_type": "text/html",
                    "content": "Custom 200 response."
                }"###,
                    ),
                )?),
        )
        .to_owned())
}
