# ngrok-rs

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

**Note: This is alpha-quality software. Interfaces may change without warning.**

ngrok is a globally distributed reverse proxy commonly used for quickly getting
a public URL to a service running inside a private network, such as on your
local laptop. The ngrok agent is usually deployed inside a private network and
is used to communicate with the ngrok cloud service.

This is the ngrok agent in library form, suitable for integrating directly into
Rust applications. This allows you to quickly build ngrok into your application
with no separate process to manage.

If you're looking for the agent wrapper, it's over
[here](https://github.com/nkconnor/ngrok). See [UPGRADING.md](./UPGRADING.md)
for tips on migrating.

# License

This project is licensed under either of

 * Apache License, Version 2.0, ([LICENSE-APACHE](LICENSE-APACHE) or
   http://www.apache.org/licenses/LICENSE-2.0)
 * MIT license ([LICENSE-MIT](LICENSE-MIT) or
   http://opensource.org/licenses/MIT)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in tokio-core by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.
