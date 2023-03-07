use ngrok::prelude::*;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let sess = ngrok::Session::builder()
        .authtoken_from_env()
        .connect()
        .await?;

    println!("session connected!");

    let tun = sess.tcp_endpoint().listen().await?;

    println!("tunnel connected! url: {}", tun.url());

    // while let Some(conn) = tun.accept().await? {
    // 	println!("got tunnel connection! remote: {:?}", conn.remote_addr());

    // 	let (rd, rw) = conn.split();
    // }

    futures::future::pending::<()>().await;

    Ok(())
}
