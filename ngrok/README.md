# ngrok-rust

[![Crates.io][crates-badge]][crates-url]
[![docs.rs][docs-badge]][docs-url]
[![MIT licensed][mit-badge]][mit-url]
[![Apache-2.0 licensed][apache-badge]][apache-url]
[![Continuous integration][ci-badge]][ci-url]

[crates-badge]: https://img.shields.io/crates/v/ngrok.svg
[crates-url]: https://crates.io/crates/ngrok
[docs-badge]: https://img.shields.io/docsrs/ngrok.svg
[docs-url]: https://docs.rs/ngrok
[ci-badge]: https://github.com/ngrok/ngrok-rust/actions/workflows/ci.yml/badge.svg
[ci-url]: https://github.com/ngrok/ngrok-rust/actions/workflows/ci.yml
[mit-badge]: https://img.shields.io/badge/license-MIT-blue.svg
[mit-url]: https://github.com/ngrok/ngrok-rust/blob/main/LICENSE-MIT
[apache-badge]: https://img.shields.io/badge/license-Apache_2.0-blue.svg
[apache-url]: https://github.com/ngrok/ngrok-rust/blob/main/LICENSE-APACHE

[API Docs (main)](https://ngrok.github.io/ngrok-rust/ngrok)

[ngrok](https://ngrok.com) is a simplified API-first ingress-as-a-service that adds connectivity, 
security, and observability to your apps.

ngrok-rust, our native and idiomatic crate for adding a public internet address
with secure ingress traffic directly into your Rust apps 🦀. If you’ve used ngrok in
the past, you can think of ngrok-rust as the ngrok agent packaged as a Rust crate.

ngrok-rust lets developers serve Rust services on the internet in a single statement
without setting up low-level network primitives like IPs, NAT, certificates,
load balancers, and even ports! Applications using ngrok-rust listen on ngrok’s global
ingress network for TCP and HTTP traffic. ngrok-rust listeners are usable with
[hyper Servers](https://docs.rs/hyper/latest/hyper/server/index.html), and connections
implement [tokio’s AsyncRead and AsyncWrite traits](https://docs.rs/tokio/latest/tokio/io/index.html).
This makes it easy to add ngrok-rust into any application that’s built on hyper, such
as the popular [axum](https://docs.rs/axum/latest/axum/) HTTP framework.

See [`/ngrok/examples/`][examples] for example usage, or the tests in
[`/ngrok/src/online_tests.rs`][online-tests].

[examples]: https://github.com/ngrok/ngrok-rust/blob/main/ngrok/examples
[online-tests]: https://github.com/ngrok/ngrok-rust/blob/main/ngrok/src/online_tests.rs

For working with the [ngrok API](https://ngrok.com/docs/api/), check out the
[ngrok Rust API Client Library](https://github.com/ngrok/ngrok-api-rs).

## Installation

Add `ngrok` to the `[dependencies]` section of your `Cargo.toml` with `cargo add`:

```bash
$ cargo add ngrok
```

## Quickstart

Create a simple HTTP server using `ngrok` and `axum`:

`Cargo.toml`:

```toml
[package]
name = "ngrok-rust-demo"
version = "0.1.0"
edition = "2021"

[dependencies]
ngrok = {version = "0.14.0"}
tokio = { version = "1", features = [
    "full"
] }
axum = { version = "0.7.4", features = ["tokio"] }
async-trait = "0.1.59"
hyper = {version = "1", features = ["full"]}
hyper-util = { version = "0.1", features = [
	"full"
] }
url = "2.5.4"
```

`src/main.rs`:

```rust
#![deny(warnings)]
use axum::{routing::get, Router};
use ngrok::config::{ForwarderBuilder, TunnelBuilder};
use std::net::SocketAddr;
use url::Url;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Create Axum app
    let app = Router::new().route("/", get(|| async { "Hello from Axum!" }));

    // Spawn Axum server
    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));
    tokio::spawn(async move {
        axum::serve(tokio::net::TcpListener::bind(addr).await.unwrap(), app)
            .await
            .unwrap();
    });

    // Set up ngrok tunnel
    let sess1 = ngrok::Session::builder()
        .authtoken_from_env()
        .connect()
        .await?;
    let sess2 = ngrok::Session::builder()
        .authtoken_from_env()
        .connect()
        .await?;

    let _listener = sess1
        .http_endpoint()
        .domain("/* your domain*/")
        .pooling_enabled(true)
        .listen_and_forward(Url::parse("http://localhost:3000").unwrap())
        .await?;
    let _listener2 = sess2
        .http_endpoint()
        .domain("/* your domain */")
        .pooling_enabled(true)
        .listen_and_forward(Url::parse("http://localhost:8000").unwrap())
        .await?;

    // Wait indefinitely
    tokio::signal::ctrl_c().await?;
    Ok(())
}

```

# Changelog

Changes to `ngrok-rust` are tracked under [CHANGELOG.md](https://github.com/ngrok/ngrok-rust/blob/main/ngrok/CHANGELOG.md).

# Join the ngrok Community

- Check out [our official docs](https://docs.ngrok.com)
- Read about updates on [our blog](https://ngrok.com/blog)
- Open an [issue](https://github.com/ngrok/ngrok-rust/issues) or [pull request](https://github.com/ngrok/ngrok-rust/pulls)
- Join our [Slack community](https://ngrok.com/slack)
- Follow us on [X / Twitter (@ngrokHQ)](https://twitter.com/ngrokhq)
- Subscribe to our [Youtube channel (@ngrokHQ)](https://www.youtube.com/@ngrokhq)

# License

This project is licensed under either of

- Apache License, Version 2.0, ([LICENSE-APACHE][apache-url] or
  <http://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT][mit-url] or
  <http://opensource.org/licenses/MIT>)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in ngrok by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.
