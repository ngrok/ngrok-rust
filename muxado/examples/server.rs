use std::env;

use futures::prelude::*;
use muxado::*;
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
    Event,
    Metadata,
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

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
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
