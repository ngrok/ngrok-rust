use std::{
    convert::Infallible,
    io,
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
    time::Duration,
};

use anyhow::anyhow;
use axum::{
    routing::get,
    BoxError,
    Router,
};
use bytes::Bytes;
use flate2::read::GzDecoder;
use futures::{
    channel::oneshot,
    prelude::*,
    stream::FuturesUnordered,
    TryStreamExt,
};
use futures_rustls::rustls::{
    pki_types,
    ClientConfig,
    RootCertStore,
};
// use native_tls;
use hyper::{
    body::Incoming,
    HeaderMap,
    Request,
    Uri,
};
use hyper_0_14::{
    header,
    StatusCode,
};
use hyper_util::{
    rt::TokioExecutor,
    server,
};
use once_cell::sync::Lazy;
use paste::paste;
use proxy_protocol::ProxyHeader;
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
    net::TcpStream,
    sync::mpsc,
    test,
};
use tokio_tungstenite::{
    connect_async,
    tungstenite::Message,
};
use tokio_util::compat::*;
use tower::{
    util::ServiceExt,
    Service,
};
use tracing_test::traced_test;
use url::Url;

use crate::{
    prelude::*,
    session::{
        SessionBuilder,
        CERT_BYTES,
    },
    Session,
};

async fn setup_session() -> Result<Session, BoxError> {
    Ok(Session::builder().authtoken_from_env().connect().await?)
}

#[cfg_attr(not(feature = "online-tests"), ignore)]
#[test]
async fn listen() -> Result<(), BoxError> {
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
async fn tunnel() -> Result<(), BoxError> {
    let tun = setup_session()
        .await?
        .http_endpoint()
        .binding("public")
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
    build_session: impl FnOnce(&mut SessionBuilder) -> &mut SessionBuilder,
    build_tunnel: impl FnOnce(&mut HttpTunnelBuilder) -> &mut HttpTunnelBuilder,
    router: axum::Router,
) -> Result<TunnelGuard, BoxError> {
    let sess = build_session(Session::builder().authtoken_from_env())
        .connect()
        .await?;

    let tun = build_tunnel(&mut sess.http_endpoint()).listen().await?;

    Ok(start_http_server(tun, router))
}

fn start_http_server<T>(mut tun: T, router: Router) -> TunnelGuard
where
    T: EndpointInfo + Tunnel + 'static,
    T::Conn: crate::tunnel_ext::ConnExt,
{
    let url = tun.url().into();

    let (tx, rx) = oneshot::channel::<()>();

    let mut make_service = router.into_make_service_with_connect_info::<SocketAddr>();

    let server = async move {
        while let Some(conn) = tun.try_next().await? {
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

    tokio::spawn(futures::future::select(Box::pin(server), rx));
    TunnelGuard { tx: tx.into(), url }
}

fn defaults<T>(opts: &mut T) -> &mut T {
    opts
}

fn hello_router() -> Router {
    Router::new().route("/", get(|| async { "Hello, world!" }))
}

async fn check_body(url: impl AsRef<str>, expected: impl AsRef<str>) -> Result<(), BoxError> {
    let body: String = reqwest::get(url.as_ref()).await?.text().await?;
    assert_eq!(body, expected.as_ref());
    Ok(())
}

#[cfg_attr(not(feature = "online-tests"), ignore)]
#[test]
async fn https() -> Result<(), BoxError> {
    let tun = serve_http(defaults, defaults, hello_router()).await?;
    let url = tun.url.as_str();

    assert!(url.starts_with("https://"));

    check_body(url, "Hello, world!").await?;

    Ok(())
}

#[cfg_attr(not(feature = "online-tests"), ignore)]
#[test]
async fn http() -> Result<(), BoxError> {
    let tun = serve_http(defaults, |tun| tun.scheme(Scheme::HTTP), hello_router()).await?;
    let url = tun.url.as_str();

    assert!(url.starts_with("http://"));

    check_body(url, "Hello, world!").await?;

    Ok(())
}

#[cfg_attr(not(feature = "paid-tests"), ignore)]
#[test]
async fn http_compression() -> Result<(), BoxError> {
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
async fn http_headers() -> Result<(), BoxError> {
    let (tx, mut rx) = mpsc::channel::<BoxError>(16);
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
                    .send(format!("unexpected value for 'foo' request header: {:?}", bar).into())
                    .await;
            }
        } else {
            let _ = tx.send("missing 'foo' request header".into()).await;
        }
        if headers.get("baz").is_some() {
            let _ = tx.send("got 'baz' request header".into()).await;
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
#[cfg_attr(not(feature = "authenticated-tests"), ignore)]
#[test]
async fn user_agent() -> Result<(), BoxError> {
    let tun = serve_http(
        defaults,
        |tun| tun.allow_user_agent("foo.*").deny_user_agent(".*"),
        hello_router(),
    )
    .await?;

    let client = reqwest::Client::new();
    let resp = client.get(&tun.url).send().await?;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    let client = reqwest::Client::builder()
        .user_agent("foobarbaz")
        .build()
        .expect("build reqwest client");

    let resp = client.get(&tun.url).send().await?;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.text().await?, "Hello, world!");

    Ok(())
}

#[traced_test]
#[cfg_attr(not(feature = "paid-tests"), ignore)]
#[test]
async fn basic_auth() -> Result<(), BoxError> {
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
async fn oauth() -> Result<(), BoxError> {
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
async fn custom_domain() -> Result<(), BoxError> {
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
#[cfg_attr(not(feature = "paid-tests"), ignore)]
#[test]
async fn policy() -> Result<(), BoxError> {
    let tun = serve_http(
        defaults,
        |tun| tun.policy(create_policy()).unwrap(),
        hello_router(),
    )
    .await?;

    let client = reqwest::Client::new();
    let resp = client.get(&tun.url).send().await?;
    assert_eq!(resp.status(), 222);

    Ok(())
}

fn create_policy() -> Result<Policy, InvalidPolicy> {
    Ok(Policy::new()
        .add_inbound(
            Rule::new("deny_put")
                .add_expression("req.Method == 'PUT'")
                .add_action(Action::new("deny", None)?),
        )
        .add_outbound(
            Rule::new("222_response")
                .add_expression("res.StatusCode == '200'")
                .add_action(Action::new(
                    "custom-response",
                    Some("{\"status_code\": 222}"),
                )?),
        )
        .to_owned())
}

#[traced_test]
#[cfg_attr(not(all(feature = "paid-tests", feature = "long-tests")), ignore)]
#[test]
async fn circuit_breaker() -> Result<(), BoxError> {
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
                    async { hyper::StatusCode::INTERNAL_SERVER_ERROR }
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
                Result::<_, BoxError>::Ok(resp.status())
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
            async fn [<proxy_proto_ $ept _ $vers>]() -> Result<(), BoxError> {
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
    [(V1, &b"PROXY TCP"[..]), (V2, &b"\x0D\x0A\x0D\x0A\x00\x0D\x0A\x51\x55\x49\x54\x0A"[..])]
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
async fn http_ip_restriction() -> Result<(), BoxError> {
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
async fn tcp_ip_restriction() -> Result<(), BoxError> {
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
async fn websocket_conversion() -> Result<(), BoxError> {
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
        Result::<_, BoxError>::Ok(())
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
                return Err(BoxError::from("didn't get message before close"));
            }
            _ => {}
        }
    }

    Ok(())
}

#[traced_test]
#[test]
#[cfg_attr(not(feature = "authenticated-tests"), ignore)]
async fn tcp() -> Result<(), BoxError> {
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
async fn tls() -> Result<(), BoxError> {
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

#[test]
#[cfg_attr(not(feature = "authenticated-tests"), ignore)]
async fn app_protocol() -> Result<(), BoxError> {
    let tun = Session::builder()
        .authtoken_from_env()
        .connect()
        .await?
        .http_endpoint()
        .app_protocol("http2")
        .listen_and_forward("https://ngrok.com".parse()?)
        .await?;

    // smoke test
    let client = reqwest::Client::new();
    let resp = client.get(tun.url()).send().await;

    assert!(resp.is_ok());

    Ok(())
}

#[test]
#[cfg_attr(not(feature = "authenticated-tests"), ignore)]
async fn verify_upstream_tls() -> Result<(), BoxError> {
    let tun = Session::builder()
        .authtoken_from_env()
        .connect()
        .await?
        .http_endpoint()
        .verify_upstream_tls(false)
        .listen_and_forward("https://ngrok.com".parse()?)
        .await?;

    // smoke test
    let client = reqwest::Client::new();
    let resp = client.get(tun.url()).send().await;

    assert!(resp.is_ok());

    Ok(())
}

#[cfg_attr(not(feature = "online-tests"), ignore)]
#[test]
async fn session_root_cas() -> Result<(), BoxError> {
    // host cannot validate cert
    let resp = Session::builder()
        .authtoken_from_env()
        .root_cas("host")?
        .connect()
        .await;
    assert!(resp.is_err());
    let err_str = resp.err().unwrap().to_string();
    tracing::debug!(?err_str);
    assert!(err_str.contains("tls")); // tls issue

    // default of 'trusted' cannot validate the marketing site
    let resp = Session::builder()
        .authtoken_from_env()
        .server_addr("ngrok.com:443")?
        .connect()
        .await;
    assert!(resp.is_err());
    let err_str = resp.err().unwrap().to_string();
    tracing::debug!(?err_str);
    assert!(err_str.contains("tls")); // tls issue

    // "host" certs can validate the marketing site's let's encrypt cert
    let resp = Session::builder()
        .authtoken_from_env()
        .root_cas("host")?
        .server_addr("ngrok.com:443")?
        .connect()
        .await;
    assert!(resp.is_err());
    let err_str = resp.err().unwrap().to_string();
    tracing::debug!(?err_str);
    assert!(!err_str.contains("tls")); // not a tls problem

    // use the trusted cert, this should connect
    Session::builder()
        .authtoken_from_env()
        .root_cas("trusted")?
        .connect()
        .await?;

    // use the default cert, this should connect
    Session::builder()
        .authtoken_from_env()
        .root_cas("assets/ngrok.ca.crt")?
        .connect()
        .await?;

    Ok(())
}

#[cfg_attr(not(feature = "online-tests"), ignore)]
#[test]
async fn session_ca_cert() -> Result<(), BoxError> {
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
async fn session_tls_config() -> Result<(), BoxError> {
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

fn tls_client_config() -> Result<Arc<ClientConfig>, &'static io::Error> {
    static CONFIG: Lazy<Result<Arc<ClientConfig>, io::Error>> = Lazy::new(|| {
        let der_certs = rustls_native_certs::load_native_certs()?
            .into_iter()
            .collect::<Vec<_>>();
        let mut root_store = RootCertStore::empty();
        root_store.add_parsable_certificates(der_certs);
        let config = ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth();
        Ok(Arc::new(config))
    });

    Ok(CONFIG.as_ref()?.clone())
}

#[traced_test]
#[cfg_attr(not(feature = "paid-tests"), ignore)]
#[test]
async fn forward_proxy_protocol_tls() -> Result<(), BoxError> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;

    let sess = Session::builder().authtoken_from_env().connect().await?;
    let forwarder = sess
        .tls_endpoint()
        .proxy_proto(ProxyProto::V2)
        .termination(Bytes::default(), Bytes::default())
        .listen_and_forward(format!("tls://{}", addr).parse()?)
        .await?;

    let tunnel_url: Url = forwarder.url().to_string().parse()?;

    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let tunnel_conn = TcpStream::connect(format!(
            "{}:{}",
            tunnel_url.host_str().unwrap(),
            tunnel_url.port().unwrap_or(443)
        ))
        .await?;

        let domain = pki_types::ServerName::try_from(tunnel_url.host_str().unwrap())
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?
            .to_owned();

        let mut tls_conn = futures_rustls::TlsConnector::from(
            tls_client_config().map_err(|e| io::Error::from(e.kind()))?,
        )
        .connect(domain, tunnel_conn.compat())
        .await?
        .compat();

        tls_conn.write_all(b"Hello, world!").await
    });

    let (conn, _) = listener.accept().await?;

    let mut proxy_conn = crate::proxy_proto::Stream::incoming(conn);
    let proxy_header = proxy_conn.proxy_header().await?.unwrap().cloned().unwrap();

    match proxy_header {
        ProxyHeader::Version2 { .. } => {}
        _ => unreachable!("we configured v2"),
    }

    // TODO: actually accept the tls connection from the server side

    Ok(())
}

fn unwrap_infallible<T>(result: Result<T, Infallible>) -> T {
    match result {
        Ok(value) => value,
        Err(err) => match err {},
    }
}
