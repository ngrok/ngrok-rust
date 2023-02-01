use std::{
    error::Error,
    time::Duration,
};

use tokio::{
    io::AsyncWriteExt,
    test,
};
use tracing::info_span;
use tracing_test::traced_test;

use crate::*;

#[traced_test]
#[test]
async fn test_cancellation() -> Result<(), Box<dyn Error>> {
    let (left, right) = tokio::io::duplex(256);

    let (dropref, waiter) = awaitdrop::awaitdrop();

    let mut server = info_span!("muxado", side = "server")
        .in_scope(move || SessionBuilder::new(left).server().start());
    let mut client = info_span!("muxado", side = "client")
        .in_scope(|| SessionBuilder::new(right).client().start());

    tokio::spawn(async move {
        let mut streams = vec![];
        while let Some(stream) = server.accept().await {
            streams.push(stream);
        }

        drop(dropref);
    });

    let mut stream1 = client.open().await?;
    let mut stream2 = client.open().await?;

    stream1.write_all("Hello,".as_bytes()).await?;
    stream2.write_all("World!".as_bytes()).await?;

    drop(stream1);
    drop(stream2);
    drop(client);

    tokio::time::timeout(Duration::from_secs(5), waiter.wait()).await?;
    Ok(())
}
