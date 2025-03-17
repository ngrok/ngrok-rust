use std::{
    convert::Infallible,
    error::Error,
    net::SocketAddr,
};

use axum::{
    extract::ConnectInfo,
    routing::get,
    BoxError,
    Router,
};
use futures::TryStreamExt;
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
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    // build our application with a single route
    let app = Router::new().route(
        "/",
        get(
            |ConnectInfo(remote_addr): ConnectInfo<SocketAddr>| async move {
                format!("Hello, {remote_addr:?}!\r\n")
            },
        ),
    );

    let sess = ngrok::Session::builder()
        .authtoken_from_env()
        .connect()
        .await?;

    let mut listener = sess
        .labeled_tunnel()
        // .app_protocol("http2")
        // .verify_upstream_tls(false)
        .label("edge", "edghts_<edge_id>")
        .metadata("example tunnel metadata from rust")
        .listen()
        .await?;

    println!("Labeled listener started!");

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

fn unwrap_infallible<T>(result: Result<T, Infallible>) -> T {
    match result {
        Ok(value) => value,
        Err(err) => match err {},
    }
}
