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
[ci-badge]: https://github.com/ngrok/ngrok-rs/actions/workflows/ci.yml/badge.svg
[ci-url]: https://github.com/ngrok/ngrok-rs/actions/workflows/ci.yml
[mit-badge]: https://img.shields.io/badge/license-MIT-blue.svg
[mit-url]: https://github.com/ngrok/ngrok-rs/blob/main/LICENSE-MIT
[apache-badge]: https://img.shields.io/badge/license-Apache_2.0-blue.svg
[apache-url]: https://github.com/ngrok/ngrok-rs/blob/main/LICENSE-APACHE

[Website](https://ngrok.com) |
[API Docs (unstable)](https://ngrok.github.io/ngrok-rs/ngrok)

ngrok is a globally distributed reverse proxy commonly used for quickly getting
a public URL to a service running inside a private network, such as on your
local laptop. The ngrok agent is usually deployed inside a private network and
is used to communicate with the ngrok cloud service.

This is the ngrok agent in library form, suitable for integrating directly into
Rust applications. This allows you to quickly build ngrok into your application
with no separate process to manage.

See [`/ngrok/examples/`][examples] for example usage, or the tests in
[`/ngrok/src/online_tests.rs`][online-tests].

[examples]: https://github.com/ngrok/ngrok-rs/blob/main/ngrok/examples
[online-tests]: https://github.com/ngrok/ngrok-rs/blob/main/ngrok/src/online_tests.rs

For working with the [ngrok API](https://ngrok.com/docs/api/), check out the
[ngrok Rust API Client Library](https://github.com/ngrok/ngrok-api-rs).

If you're looking for the agent wrapper, it's over
[here](https://github.com/nkconnor/ngrok). See [UPGRADING.md][upgrading]
for tips on migrating.

[upgrading]: https://github.com/ngrok/ngrok-rs/blob/main/ngrok/UPGRADING.md

## Installation

Add `ngrok` to the `[dependencies]` section of your `Cargo.toml`:

```toml
...

[dependencies]
ngrok = "0.8"

...
```

Alternatively, with [cargo-edit][cargo-edit]:

```bash
$ cargo add ngrok
```

[cargo-edit]: https://crates.io/crates/cargo-edit

## Quickstart

Create a simple HTTP server using `ngrok` and `axum`:

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

 * Apache License, Version 2.0, ([LICENSE-APACHE][apache-url] or
   <http://www.apache.org/licenses/LICENSE-2.0>)
 * MIT license ([LICENSE-MIT][mit-url] or
   <http://opensource.org/licenses/MIT>)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in ngrok by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.
