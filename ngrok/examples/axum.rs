use std::{
    convert::Infallible,
    net::SocketAddr,
};

use axum::{
    extract::ConnectInfo,
    routing::get,
    Router,
};
use axum_core::BoxError;
use futures::stream::TryStreamExt;
use hyper::{
    body::Incoming,
    Request,
};
use hyper_util::{
    rt::TokioExecutor,
    server,
};
use ngrok::prelude::*;
use tower::{
    util::ServiceExt,
    Service,
};

#[tokio::main]
async fn main() -> Result<(), BoxError> {
    // build our application with a single route
    let app = Router::new().route(
        "/",
        get(
            |ConnectInfo(remote_addr): ConnectInfo<SocketAddr>| async move {
                format!("Hello, {remote_addr:?}!\r\n")
            },
        ),
    );

    let mut listener = ngrok::Session::builder()
        .authtoken_from_env()
        .connect()
        .await?
        .http_endpoint()
        // .allow_cidr("0.0.0.0/0")
        // .basic_auth("ngrok", "online1line")
        // .circuit_breaker(0.5)
        // .compression()
        // .deny_cidr("10.1.1.1/32")
        // .verify_upstream_tls(false)
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
        // .traffic_policy(POLICY_JSON)
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

    println!("Listener started on URL: {:?}", listener.url());

    let mut make_service = app.into_make_service_with_connect_info::<SocketAddr>();

    let server = async move {
        while let Some(conn) = listener.try_next().await? {
            let remote_addr = conn.remote_addr();
            let tower_service = unwrap_infallible(make_service.call(remote_addr).await);

            tokio::spawn(async move {
                let hyper_service =
                    hyper::service::service_fn(move |request: Request<Incoming>| {
                        tower_service.clone().oneshot(request)
                    });

                if let Err(err) = server::conn::auto::Builder::new(TokioExecutor::new())
                    .serve_connection_with_upgrades(conn, hyper_service)
                    .await
                {
                    eprintln!("failed to serve connection: {err:#}");
                }
            });
        }
        Ok::<(), BoxError>(())
    };

    server.await?;

    Ok(())
}

#[allow(dead_code)]
const POLICY_JSON: &str = r###"{
    "inbound":[
        {
            "name":"deny_put",
            "expressions":["req.Method == 'PUT'"],
            "actions":[{"Type":"deny"}]
        }],
    "outbound":[
        {
            "name":"change success response",
            "expressions":["res.StatusCode == '200'"],
            "actions":[{
                "type":"custom-response",
                "config":{
                    "status_code":201, 
                    "content": "Custom 200 response.", 
                    "headers": {
                        "content_type": "text/html"
                    }
                }
            }]
        }]
}"###;

#[allow(dead_code)]
const POLICY_YAML: &str = r###"
---
inbound:
    - name: "deny_put"
      expressions:
      - "req.Method == 'PUT'"
      actions:
      - type: "deny"
outbound:
    - name: "change success response"
      expressions:
      - "res.StatusCode == '200'"
      actions:
      - type: "custom-response"
        config:
          status_code: 201
          content: "Custom 200 response."
          headers:
            content_type: "text/html"
"###;

#[allow(dead_code)]
fn create_policy() -> Result<Policy, InvalidPolicy> {
    Ok(Policy::new()
        .add_inbound(
            Rule::new("deny_put")
                .add_expression("req.Method == 'PUT'")
                .add_action(Action::new("deny", None)?),
        )
        .add_outbound(
            Rule::new("200_response")
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

// const CA_CERT: &[u8] = include_bytes!("ca.crt");

fn unwrap_infallible<T>(result: Result<T, Infallible>) -> T {
    match result {
        Ok(value) => value,
        Err(err) => match err {},
    }
}
