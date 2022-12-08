use std::env;

use muxado::*;
use tokio::{
    io::{
        AsyncReadExt,
        AsyncWriteExt,
    },
    net::TcpStream,
};
use tracing::info;

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    tracing_subscriber::fmt()
        .pretty()
        // .with_span_events(FmtSpan::ENTER)
        .with_env_filter(env::var("RUST_LOG").unwrap_or("info".into()))
        .init();

    let conn = TcpStream::connect("localhost:1234").await?;

    let mut sess = SessionBuilder::new(conn).start();
    let mut stream = sess.open().await?;

    for _ in 0..1024 {
        stream.write_all(b"Hello, world!\n").await?;
    }
    stream.shutdown().await?;

    let mut buf = vec![0u8; 512];
    let mut n = 0;

    loop {
        let prev = n;
        n += stream.read(&mut buf[n..]).await?;
        if n == prev {
            // EOF
            info!("EoF");
            break;
        }
        info!(
            msg = String::from_utf8_lossy(&buf[prev..n]).as_ref(),
            "got a stream response"
        );
        n = 0;
    }
    Ok(())
}
