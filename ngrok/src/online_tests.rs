use std::{
    io::prelude::*,
    net::SocketAddr,
    str::FromStr,
    sync::{
        atomic::{
            AtomicUsize,
            Ordering,
        },
        Arc,
    },
};

use anyhow::{
    anyhow,
    Error,
};
use axum::{
    routing::get,
    Router,
};
use flate2::read::GzDecoder;
use futures::{
    channel::oneshot,
    prelude::*,
    stream::FuturesUnordered,
};
use hyper::{
    header,
    HeaderMap,
    StatusCode,
    Uri,
};
use paste::paste;
use rand::{
    distributions::Alphanumeric,
    thread_rng,
    Rng,
};
use tokio::{
    io::{
        AsyncReadExt,
        AsyncWriteExt,
    },
    sync::mpsc,
    test,
};
use tokio_tungstenite::{
    connect_async,
    tungstenite::Message,
};
use tracing_test::traced_test;

use crate::{
    config::{
        HttpTunnelBuilder,
        OauthOptions,
        ProxyProto,
        Scheme,
    },
    prelude::*,
    session::{
        SessionBuilder,
        CERT_BYTES,
    },
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

    Ok(start_http_server(tun, router))
}

fn start_http_server(tun: impl UrlTunnel, router: Router) -> TunnelGuard {
    let url = tun.url().into();

    let (tx, rx) = oneshot::channel::<()>();

    tokio::spawn(futures::future::select(
        axum::Server::builder(tun)
            .serve(router.into_make_service_with_connect_info::<SocketAddr>()),
        rx,
    ));
    TunnelGuard { tx: tx.into(), url }
}

fn defaults<T>(opts: T) -> T {
    opts
}

fn hello_router() -> Router {
    Router::new().route("/", get(|| async { "Hello, world!" }))
}

async fn check_body(url: impl AsRef<str>, expected: impl AsRef<str>) -> Result<(), Error> {
    let body: String = reqwest::get(url.as_ref()).await?.text().await?;
    assert_eq!(body, expected.as_ref());
    Ok(())
}

#[cfg_attr(not(feature = "online-tests"), ignore)]
#[test]
async fn https() -> Result<(), Error> {
    let tun = serve_http(defaults, defaults, hello_router()).await?;
    let url = tun.url.as_str();

    assert!(url.starts_with("https://"));

    check_body(url, "Hello, world!").await?;

    Ok(())
}

#[cfg_attr(not(feature = "online-tests"), ignore)]
#[test]
async fn http() -> Result<(), Error> {
    let tun = serve_http(defaults, |tun| tun.scheme(Scheme::HTTP), hello_router()).await?;
    let url = tun.url.as_str();

    assert!(url.starts_with("http://"));

    check_body(url, "Hello, world!").await?;

    Ok(())
}

#[cfg_attr(not(feature = "paid-tests"), ignore)]
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

#[cfg_attr(not(feature = "paid-tests"), ignore)]
#[test]
async fn http_headers() -> Result<(), Error> {
    let (tx, mut rx) = mpsc::channel::<Error>(16);
    // For some reason, the hyper machinery keeps a clone of the `tx`, which
    // causes it to never look closed, even when we drop the tunnel guard, which
    // shuts down the hyper server. Maybe a leaked task? Work around it by
    // keeping only one RAII tx here, and only give the handler a weak ref to
    // it.
    let weak = tx.downgrade();
    let handler = move |headers: HeaderMap| async move {
        let tx = weak
            .upgrade()
            .expect("no more requests after server shutdown");

        if let Some(bar) = headers.get("foo") {
            if bar != "bar" {
                let _ = tx
                    .send(anyhow!(
                        "unexpected value for 'foo' request header: {:?}",
                        bar
                    ))
                    .await;
            }
        } else {
            let _ = tx.send(anyhow!("missing 'foo' request header")).await;
        }
        if headers.get("baz").is_some() {
            let _ = tx.send(anyhow!("got 'baz' request header")).await;
        }

        ([("python", "lolnope")], "Hello, world!")
    };
    let tun = serve_http(
        defaults,
        |tun| {
            tun.request_header("foo", "bar")
                .remove_request_header("baz")
                .response_header("spam", "eggs")
                .remove_response_header("python")
        },
        Router::new().route("/", get(handler)),
    )
    .await?;
    let url = &tun.url;

    let client = reqwest::Client::new();
    let resp = client.get(url).header("baz", "bad header").send().await?;

    assert_eq!(
        resp.headers()
            .get("spam")
            .expect("'spam' header should exist"),
        "eggs"
    );
    assert!(resp.headers().get("python").is_none(),);

    drop(tun);
    drop(tx);

    if let Some(err) = rx.recv().await {
        return Err(err);
    }

    Ok(())
}

#[traced_test]
#[cfg_attr(not(feature = "paid-tests"), ignore)]
#[test]
async fn basic_auth() -> Result<(), Error> {
    let tun = serve_http(
        defaults,
        |tun| tun.basic_auth("user", "foobarbaz"),
        hello_router(),
    )
    .await?;

    let client = reqwest::Client::new();
    let resp = client.get(&tun.url).send().await?;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    let resp = client
        .get(&tun.url)
        .basic_auth("user", "foobarbaz".into())
        .send()
        .await?;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.text().await?, "Hello, world!");

    Ok(())
}

#[traced_test]
#[cfg_attr(not(feature = "paid-tests"), ignore)]
#[test]
async fn oauth() -> Result<(), Error> {
    let tun = serve_http(
        defaults,
        |tun| tun.oauth(OauthOptions::new("google")),
        hello_router(),
    )
    .await?;

    let client = reqwest::Client::new();
    let resp = client.get(&tun.url).send().await?;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.text().await?;
    assert_ne!(body, "Hello, world!");
    assert!(body.contains("accounts.google.com"));

    Ok(())
}

#[traced_test]
#[cfg_attr(not(feature = "paid-tests"), ignore)]
#[test]
async fn custom_domain() -> Result<(), Error> {
    let mut rng = thread_rng();
    let subdomain = (0..7)
        .map(|_| rng.sample(Alphanumeric) as char)
        .collect::<String>()
        .to_lowercase();
    let _tun = serve_http(
        defaults,
        |tun| tun.domain(format!("{subdomain}.ngrok.io")),
        hello_router(),
    )
    .await?;

    check_body(format!("https://{subdomain}.ngrok.io"), "Hello, world!").await?;

    Ok(())
}

#[traced_test]
#[cfg_attr(not(all(feature = "paid-tests", feature = "long-tests")), ignore)]
#[test]
async fn circuit_breaker() -> Result<(), Error> {
    let ctr = Arc::new(AtomicUsize::new(0));
    let tun = serve_http(
        defaults,
        |tun| tun.circuit_breaker(0.01),
        Router::new().route(
            "/",
            get({
                let ctr = ctr.clone();
                move || {
                    ctr.fetch_add(1, Ordering::SeqCst);
                    async { StatusCode::INTERNAL_SERVER_ERROR }
                }
            }),
        ),
    )
    .await?;

    let mut attempts = 0;
    for _ in 0..20 {
        let mut futs = FuturesUnordered::new();
        // smaller batches to have less in-flight requests and break sooner
        for _ in 0..25 {
            attempts += 1;
            let url = tun.url.clone();
            futs.push(async move {
                let resp = reqwest::get(url).await?;
                let status = resp.status();
                tracing::debug!(?status);
                Result::<_, Error>::Ok(resp.status())
            });
        }
        let mut done = false;
        while let Some(res) = futs.next().await {
            if res? == StatusCode::SERVICE_UNAVAILABLE {
                // circuit breaker is working, done after this batch
                done = true;
            }
        }
        if done {
            break;
        }
    }

    // validate that some, but not all, requests were dropped
    let actual = ctr.load(Ordering::SeqCst);
    assert!(actual > 4, "expected > 4 requests, got {actual}");
    assert!(
        actual < attempts,
        "expected < {attempts} requests, got {actual}"
    );

    Ok(())
}

// Shamelessly ripped from stackoverflow:
// https://stackoverflow.com/questions/35901547/how-can-i-find-a-subsequence-in-a-u8-slice
fn find_subsequence<T>(haystack: &[T], needle: &[T]) -> Option<usize>
where
    for<'a> &'a [T]: PartialEq,
{
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

macro_rules! proxy_proto_test {
    (genone: $ept:ident, $vers:ident, $tun:ident, $req:expr, $cont:expr) => {
        paste! {
            #[traced_test]
            #[cfg_attr(not(feature = "paid-tests"), ignore)]
            #[test]
            #[allow(non_snake_case)]
            async fn [<proxy_proto_ $ept _ $vers>]() -> Result<(), Error> {
                let sess = Session::builder().authtoken_from_env().connect().await?;
                let mut $tun = sess
                    .[<$ept _endpoint>]()
                    .proxy_proto(ProxyProto::$vers).listen().await?;

                let req = $req;
                tokio::spawn(req);


                let mut buf = vec![0u8; 12];
                let mut conn = $tun
                    .try_next()
                    .await?
                    .ok_or_else(|| anyhow!("tunnel closed"))?;

                conn.read_exact(&mut buf).await?;

                assert!(find_subsequence(&buf, $cont).is_some());

                Ok(())
            }
        }
    };

    ($vers:ident, $ex:expr, [$(($ept:ident, |$tun:ident| $req:expr)),*]) => {
        $(
            proxy_proto_test!(genone: $ept, $vers, $tun, $req, $ex);
        )*
    };

    ([$(($vers:ident, $ex:expr)),*] $rest:tt) => {
        $(
            proxy_proto_test!($vers, $ex, $rest);
        )*
    };
}

proxy_proto_test!(
    [(V1, &b"PROXY TCP4"[..]), (V2, &b"\x0D\x0A\x0D\x0A\x00\x0D\x0A\x51\x55\x49\x54\x0A"[..])]
    [
        (http, |tun| {
            reqwest::get(tun.url().to_string())
        }),
        (tcp, |tun| {
            reqwest::get(tun.url().to_string().replacen("tcp", "http", 1))
        })
    ]
);

#[traced_test]
#[test]
#[cfg_attr(not(feature = "paid-tests"), ignore)]
async fn http_ip_restriction() -> Result<(), Error> {
    let tun = serve_http(
        defaults,
        |tun| tun.allow_cidr("127.0.0.1/32").deny_cidr("0.0.0.0/0"),
        hello_router(),
    )
    .await?;

    let resp = reqwest::get(&tun.url).await?;

    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    Ok(())
}

#[traced_test]
#[test]
#[cfg_attr(not(feature = "paid-tests"), ignore)]
async fn tcp_ip_restriction() -> Result<(), Error> {
    let tun = Session::builder()
        .authtoken_from_env()
        .connect()
        .await?
        .tcp_endpoint()
        .allow_cidr("127.0.0.1/32")
        .deny_cidr("0.0.0.0/0")
        .listen()
        .await?;

    let tun = start_http_server(tun, hello_router());

    let url = tun.url.replacen("tcp", "http", 1);

    assert!(reqwest::get(&url).await.is_err());

    Ok(())
}

#[traced_test]
#[test]
#[cfg_attr(not(feature = "paid-tests"), ignore)]
async fn websocket_conversion() -> Result<(), Error> {
    let mut tun = Session::builder()
        .authtoken_from_env()
        .connect()
        .await?
        .http_endpoint()
        .websocket_tcp_conversion()
        .listen()
        .await?;

    let url = Uri::from_str(&tun.url().replacen("https", "wss", 1))?;

    tokio::spawn(async move {
        while let Some(mut conn) = tun.try_next().await? {
            conn.write_all("Hello, websockets!".as_bytes()).await?;
        }
        Result::<_, Error>::Ok(())
    });

    let mut wss = connect_async(url).await.expect("connect").0;

    loop {
        let msg = wss.try_next().await.expect("read").expect("message");

        match msg {
            Message::Binary(bs) => {
                assert_eq!(String::from_utf8_lossy(&bs), "Hello, websockets!");
                break;
            }
            Message::Text(t) => {
                assert_eq!(t, "Hello, websockets!");
                break;
            }
            Message::Ping(b) => {
                wss.send(Message::Pong(b)).await?;
            }
            Message::Close(_) => {
                anyhow::bail!("didn't get message before close");
            }
            _ => {}
        }
    }

    Ok(())
}

#[traced_test]
#[test]
#[cfg_attr(not(feature = "authenticated-tests"), ignore)]
async fn tcp() -> Result<(), Error> {
    let tun = Session::builder()
        .authtoken_from_env()
        .connect()
        .await?
        .tcp_endpoint()
        .listen()
        .await?;

    let tun = start_http_server(tun, hello_router());

    let url = tun.url.replacen("tcp", "http", 1);

    check_body(url, "Hello, world!").await?;

    Ok(())
}

const CERT: &[u8] = include_bytes!("../examples/domain.crt");
const KEY: &[u8] = include_bytes!("../examples/domain.key");

#[traced_test]
#[test]
#[cfg_attr(not(feature = "authenticated-tests"), ignore)]
async fn tls() -> Result<(), Error> {
    let tun = Session::builder()
        .authtoken_from_env()
        .connect()
        .await?
        .tls_endpoint()
        .termination(CERT.into(), KEY.into())
        .listen()
        .await?;

    let tun = start_http_server(tun, hello_router());

    let url = tun.url.replacen("tls", "http", 1);

    let client = reqwest::Client::new();
    let resp = client.get(url.clone()).send().await;

    assert!(resp.is_err());
    let err_str = resp.err().unwrap().to_string();
    tracing::debug!(?err_str);
    assert!(err_str.contains("certificate"));

    Ok(())
}

#[cfg_attr(not(feature = "online-tests"), ignore)]
#[test]
async fn session_ca_cert() -> Result<(), Error> {
    // invalid cert
    let resp = Session::builder()
        .authtoken_from_env()
        .ca_cert(CERT.into())
        .connect()
        .await;

    assert!(resp.is_err());
    let err_str = resp.err().unwrap().to_string();
    tracing::debug!(?err_str);
    assert!(err_str.contains("tls"));

    // use the default cert, this should connect
    Session::builder()
        .authtoken_from_env()
        .ca_cert(CERT_BYTES.into())
        .connect()
        .await?;

    Ok(())
}

#[cfg_attr(not(feature = "online-tests"), ignore)]
#[test]
async fn session_tls_config() -> Result<(), Error> {
    let default_tls_config = Session::builder().get_or_create_tls_config();

    // invalid cert, but valid tls_config overrides
    Session::builder()
        .authtoken_from_env()
        .ca_cert(CERT.into())
        .tls_config(default_tls_config)
        .connect()
        .await?;

    Ok(())
}
