# The ngrok Agent SDK

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

[Website](https://ngrok.com) |
[API Docs (main)](https://ngrok.github.io/ngrok-rust/ngrok)

ngrok is a simplified API-first ingress-as-a-service that adds connectivity,
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

If you're looking for the agent wrapper, it's over
[here](https://github.com/nkconnor/ngrok). See [UPGRADING.md][upgrading]
for tips on migrating.

[upgrading]: https://github.com/ngrok/ngrok-rust/blob/main/ngrok/UPGRADING.md

## Installation

Add `ngrok` to the `[dependencies]` section of your `Cargo.toml`:

```toml
...

[dependencies]
ngrok = "0.13"

...
```

Alternatively, with `cargo add`:

```bash
$ cargo add ngrok
```

## Quickstart

Create a simple HTTP server using `ngrok` and `axum`:

`Cargo.toml`:

```toml
[package]
name = "ngrok-axum-example"
version = "0.1.0"
edition = "2021"

[dependencies]
ngrok = { version="0.13", features=["axum"] }
tokio = { version = "1.26", features = ["full"] }
axum = "0.6"
anyhow = "1.0"
```

`src/main.rs`:

```rust
use std::net::SocketAddr;

use axum::{
    extract::ConnectInfo,
    routing::get,
    Router,
};
use ngrok::prelude::*;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // build our application with a single route
    let app = Router::new().route(
        "/",
        get(
            |ConnectInfo(remote_addr): ConnectInfo<SocketAddr>| async move {
                format!("Hello, {remote_addr:?}!\r\n")
            },
        ),
    );

    let tun = ngrok::Session::builder()
        // Read the token from the NGROK_AUTHTOKEN environment variable
        .authtoken_from_env()
        // Connect the ngrok session
        .connect()
        .await?
        // Start a tunnel with an HTTP edge
        .http_endpoint()
        .listen()
        .await?;

    println!("Tunnel started on URL: {:?}", tun.url());

    // Instead of binding a local port like so:
    // axum::Server::bind(&"0.0.0.0:8000".parse().unwrap())
    // Run it with an ngrok tunnel instead!
    axum::Server::builder(tun)
        .serve(app.into_make_service_with_connect_info::<SocketAddr>())
        .await
        .unwrap();

    Ok(())
}
```

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
