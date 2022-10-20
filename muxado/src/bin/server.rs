use std::env;

use futures::prelude::*;
use muxado::session::*;
use tokio::{
    io::{
        AsyncReadExt,
        AsyncWriteExt,
    },
    net::TcpListener,
};
use tracing::{
    info,
    span,
    subscriber::Interest,
    Callsite,
    Event,
    Metadata,
    Subscriber,
};
use tracing_subscriber::{
    self,
    filter::FilterFn,
    layer::{
        Filter,
        SubscriberExt,
    },
    util::SubscriberInitExt,
    EnvFilter,
    Layer,
};

/// Subscriber that claims that it's always enabled, but does nothing.
struct AlwaysSubscriber;
impl tracing::Subscriber for AlwaysSubscriber {
    #[inline]
    fn register_callsite(&self, _: &'static Metadata<'static>) -> Interest {
        Interest::always()
    }

    fn new_span(&self, _: &span::Attributes<'_>) -> span::Id {
        span::Id::from_u64(0xDEAD)
    }

    fn event(&self, _event: &Event<'_>) {}

    fn record(&self, _span: &span::Id, _values: &span::Record<'_>) {}

    fn record_follows_from(&self, _span: &span::Id, _follows: &span::Id) {}

    #[inline]
    fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
        true
    }

    fn enter(&self, _span: &span::Id) {}
    fn exit(&self, _span: &span::Id) {}
}

fn env_filter_fn(env_filter: EnvFilter) -> FilterFn<impl Fn(&Metadata<'_>) -> bool> {
    let fake_subscriber = AlwaysSubscriber.with(env_filter);

    FilterFn::new(move |m| fake_subscriber.enabled(m))
}

#[test]
fn test_env_filter_fn() {
    let filter_string = "foo=warn,bar=info,error,baz=trace";
    let env_filter = EnvFilter::new(filter_string);
    let filter_fn = env_filter_fn(env_filter);
    let filter = &filter_fn as &dyn Filter<AlwaysSubscriber>;

    struct Cs;
    impl Callsite for Cs {
        fn set_interest(&self, _interest: Interest) {}
        fn metadata(&self) -> &Metadata<'_> {
            unimplemented!()
        }
    }

    macro_rules! meta {
        ($tgt:expr, $lvl:expr) => {
            Metadata::new(
                "arya",
                $tgt,
                $lvl,
                None,
                None,
                None,
                FieldSet::new(&[], identify_callsite!(&Cs)),
                Kind::SPAN,
            )
        };
    }

    macro_rules! test_filter {
        ($filter:expr, $mname:ident, $tgt:expr, $lvl:expr, $expect:ident) => {
            #[allow(warnings)]
            const $mname: Metadata<'_> = meta!($tgt, $lvl);
            assert!($filter.callsite_enabled(&$mname).$expect());
        };
    }

    test_filter!(filter, m1, "foo", Level::DEBUG, is_never);
    test_filter!(filter, m2, "foo", Level::WARN, is_always);
    test_filter!(filter, m3, "baz", Level::WARN, is_always);
    test_filter!(filter, m4, "spam", Level::WARN, is_never);
    test_filter!(filter, m5, "eggs", Level::ERROR, is_never);
}

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    // let std_filter_string = env::var("RUST_LOG").unwrap_or("info".into());
    // let otel_filter_string = env::var("OTEL_LOG").unwrap_or("info".into());
    // println!("stdout filter: {}", std_filter_string);
    // println!("otel filter: {}", otel_filter_string);
    // let std_filter = EnvFilter::new(std_filter_string);
    // let otel_filter = EnvFilter::new(otel_filter_string);
    // let std_output = tracing_subscriber::fmt::layer().pretty();
    // let tracer = opentelemetry_jaeger::new_pipeline()
    //     .with_service_name(env!("CARGO_PKG_NAME"))
    //     .install_simple()?;
    // let opentelemetry = tracing_opentelemetry::layer().with_tracer(tracer);
    // tracing_subscriber::registry()
    //     .with(opentelemetry.with_filter(env_filter_fn(otel_filter)))
    //     .with(std_output.with_filter(env_filter_fn(std_filter)))
    //     .try_init()?;

    // console_subscriber::init();

    tracing_subscriber::fmt()
        .pretty()
        // .with_span_events(FmtSpan::ENTER)
        .with_env_filter(env::var("RUST_LOG").unwrap_or("info".into()))
        .init();

    let l = TcpListener::bind("127.0.0.1:1234").await?;

    loop {
        let (conn, _addr) = l.accept().await?;

        let res = (|| async move {
            let mut sess = SessionBuilder::new(conn).start();
            let sess = &mut sess;

            loop {
                let mut stream = if let Some(stream) = sess.accept().await {
                    stream
                } else {
                    break;
                };

                tokio::spawn(
                    async move {
                        let mut buf = vec![0u8; 512];

                        loop {
                            let n = stream.read(&mut buf).await?;
                            if n == 0 {
                                info!("hit eof");
                                break;
                            }
                            info!(
                                msg = String::from_utf8_lossy(&buf[..n]).as_ref(),
                                "read data from stream"
                            );
                            stream.write_all(&buf[0..n]).await?;
                            info!(
                                msg = String::from_utf8_lossy(&buf[..n]).as_ref(),
                                "echoed data to stream"
                            );
                        }

                        Result::<(), anyhow::Error>::Ok(())
                    }
                    .then(|res| async {
                        if let Err(err) = res {
                            tracing::warn!(err = %err, "stream closed with error");
                        } else {
                            tracing::info!("stream closed");
                        }
                    }),
                );
            }
            Result::<(), anyhow::Error>::Ok(())
        })()
        .await;

        if let Err(err) = res {
            tracing::warn!(err = %err, "session closed with error");
        } else {
            tracing::warn!("session closed");
        }
    }
}
