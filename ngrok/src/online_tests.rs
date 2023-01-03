use std::{
    io::prelude::*,
    net::SocketAddr,
};

use anyhow::Error;
use axum::{
    routing::get,
    Router,
};
use flate2::read::GzDecoder;
use futures::channel::oneshot;
use hyper::header;
use tokio::test;

use crate::{
    config::{
        HttpTunnelBuilder,
        Scheme,
    },
    prelude::*,
    session::SessionBuilder,
    Session,
};

async fn setup_session() -> Result<Session, Error> {
    Ok(Session::builder().authtoken_from_env().connect().await?)
}

#[cfg_attr(not(feature = "online-tests"), ignore)]
#[test]
async fn listen() -> Result<(), Error> {
    let _ = Session::builder()
        .authtoken_from_env()
        .connect()
        .await?
        .http_endpoint()
        .listen()
        .await?;
    Ok(())
}

#[cfg_attr(not(feature = "online-tests"), ignore)]
#[test]
async fn tunnel() -> Result<(), Error> {
    let tun = setup_session()
        .await?
        .http_endpoint()
        .metadata("Hello, world!")
        .forwards_to("some application")
        .listen()
        .await?;

    assert_eq!("Hello, world!", tun.metadata());
    assert_eq!("some application", tun.forwards_to());

    Ok(())
}

struct TunnelGuard {
    tx: Option<oneshot::Sender<()>>,
    url: String,
}

impl Drop for TunnelGuard {
    fn drop(&mut self) {
        let _ = self.tx.take().unwrap().send(());
    }
}

// Spawn an http server using the provided session and tunnel options, and an
// axum router.
// The returned guard, when dropped, will cause the server to shut down.
async fn serve_http(
    build_session: impl FnOnce(SessionBuilder) -> SessionBuilder,
    build_tunnel: impl FnOnce(HttpTunnelBuilder) -> HttpTunnelBuilder,
    router: axum::Router,
) -> Result<TunnelGuard, Error> {
    let sess = build_session(Session::builder().authtoken_from_env())
        .connect()
        .await?;

    let tun = build_tunnel(sess.http_endpoint()).listen().await?;

    let url = tun.url().into();

    let (tx, rx) = oneshot::channel::<()>();

    tokio::spawn(futures::future::select(
        axum::Server::builder(tun)
            .serve(router.into_make_service_with_connect_info::<SocketAddr>()),
        rx,
    ));
    Ok(TunnelGuard { tx: tx.into(), url })
}

fn defaults<T>(opts: T) -> T {
    opts
}

fn hello_router() -> Router {
    Router::new().route("/", get(|| async { "Hello, world!" }))
}

#[cfg_attr(not(feature = "online-tests"), ignore)]
#[test]
async fn https() -> Result<(), Error> {
    let tun = serve_http(defaults, defaults, hello_router()).await?;
    let url = tun.url.as_str();

    assert!(url.starts_with("https://"));

    let body: String = reqwest::get(url).await?.text().await?;

    assert_eq!(body, "Hello, world!");

    Ok(())
}

#[cfg_attr(not(feature = "online-tests"), ignore)]
#[test]
async fn http() -> Result<(), Error> {
    let tun = serve_http(defaults, |tun| tun.scheme(Scheme::HTTP), hello_router()).await?;
    let url = tun.url.as_str();

    assert!(url.starts_with("http://"));

    let body: String = reqwest::get(url).await?.text().await?;

    assert_eq!(body, "Hello, world!");

    Ok(())
}

#[cfg_attr(not(feature = "authenticated-tests"), ignore)]
#[test]
async fn http_compression() -> Result<(), Error> {
    let tun = serve_http(defaults, |tun| tun.compression(), hello_router()).await?;
    let url = tun.url.as_str();

    let client = reqwest::Client::new();
    let resp = client
        .get(url)
        .header(header::ACCEPT_ENCODING, "gzip")
        .send()
        .await?;

    assert_eq!(
        resp.headers().get(header::CONTENT_ENCODING).unwrap(),
        "gzip"
    );

    let body_bytes = resp.bytes().await?;

    let mut decoder = GzDecoder::new(&*body_bytes);
    let mut body_string = String::new();
    decoder.read_to_string(&mut body_string).unwrap();

    assert_eq!(body_string, "Hello, world!");

    Ok(())
}
