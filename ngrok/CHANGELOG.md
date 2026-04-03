## 1.0.0

### Breaking Changes — v2 API

This is a major API redesign aligning ngrok-rust with the ngrok-go v2 API design.

#### New Types
- **`Agent`** / **`AgentBuilder`**: Replaces `Session` / `SessionBuilder`. Use `Agent::builder()` to configure, `.build()` to create (sync), then `.connect()` / `.listen()` / `.forward()` for async operations. Auto-connects by default on first use.
- **`EndpointListener`**: Replaces `HttpTunnel`, `TcpTunnel`, `TlsTunnel`, `LabeledTunnel`. A unified endpoint that implements `Stream<Item = Result<EndpointConn, AcceptError>>`.
- **`EndpointForwarder`**: Replaces `Forwarder<T>`. Wraps an `EndpointListener` that is actively forwarding to an upstream.
- **`EndpointOptions`** / **`EndpointListenBuilder`** / **`EndpointForwardBuilder`**: Unified endpoint configuration. Protocol is inferred from the URL scheme (`https://`, `tcp://`, `tls://`, `http://`).
- **`Upstream`**: Describes where traffic is forwarded to.
- **`Endpoint`** trait: Common interface for endpoint metadata (`id()`, `url()`, `protocol()`, etc.).
- **`Event`** enum: Unified agent events (connect, disconnect, heartbeat).
- **`RpcRequest`** trait and method constants: Unified RPC handler interface.
- **Default agent**: `ngrok::listen()`, `ngrok::forward()`, `ngrok::default_agent()` — top-level convenience using a global default agent initialized from `NGROK_AUTHTOKEN`.

#### Removed from Public API
- `Session`, `SessionBuilder` — use `Agent`, `AgentBuilder`
- `HttpTunnelBuilder`, `TcpTunnelBuilder`, `TlsTunnelBuilder`, `LabeledTunnelBuilder` — use `EndpointListenBuilder` with `.url()` to set protocol
- `Tunnel` trait, `TunnelInfo`, `EndpointInfo`, `EdgeInfo`, `TunnelCloser` — use `Endpoint` trait
- `HttpTunnel`, `TcpTunnel`, `TlsTunnel`, `LabeledTunnel` — use `EndpointListener`
- `Forwarder<T>` — use `EndpointForwarder`
- `OauthOptions`, `OidcOptions`, `Policy`, `Rule`, `Action` — use traffic policy YAML/JSON via `EndpointListenBuilder::traffic_policy()`
- `Scheme` enum — protocol now inferred from URL
- `TunnelBuilder`, `ForwarderBuilder` traits
- `EdgeConn`, `EdgeConnInfo` — labeled tunnels removed
- `config` module — all configuration now through `EndpointOptions`

#### Migration Guide

**Before (v0.x):**
```rust
let session = ngrok::Session::builder()
    .authtoken_from_env()
    .connect()
    .await?;
let tunnel = session.http_endpoint()
    .domain("app.ngrok.app")
    .listen_and_forward(url)
    .await?;
```

**After (v1.0):**
```rust
let fwd = ngrok::forward(ngrok::Upstream::new("localhost:8080"))
    .url("https://app.ngrok.app")
    .start()
    .await?;
```

Or with a custom agent:
```rust
let agent = ngrok::Agent::builder()
    .authtoken("your-token")
    .build()?;
let listener = agent.listen()
    .url("https://app.ngrok.app")
    .start()
    .await?;
```

#### Added
- `ffi` feature flag for FFI-friendly types (`FfiAgent`, `FfiEndpointOptions`, etc.)

## 0.18.0
- Add support for CEL filtering when listing resources.
- Add support for service users
- Add support for `vault_name` on Secrets
-Make `pooling_enabled` on Endpoints optional

## 0.17.0

### Breaking Changes
- **Binding is now optional**: Tests no longer hardcode `binding("public")`. The ngrok service will use its default binding configuration when not explicitly specified.
- **Binding validation**: The `binding()` method now validates input values and panics on invalid values or multiple calls.

### Added
- Added `Binding` enum with three variants: `Public`, `Internal`, and `Kubernetes`
- Added validation for binding values - only "public", "internal", and "kubernetes" are accepted (case-insensitive)
- Added `binding()` method documentation with examples for both string and typed enum usage
- Added panic behavior when `binding()` is called more than once (only one binding allowed)

### Changed
- `binding()` method now accepts both strings and the `Binding` enum via `Into<String>`
- Removed hardcoded "public" binding from all tests - bindings are now truly optional

## 0.15.0
- - Removes `hyper-proxy` and `ring` dependencies 

## 0.14.0
- - Adds `pooling_enabled` option, allowing the endpoint to pool with other endpoints with the same host/port/binding

## 0.13.1

- Preserve the `ERR_NGROK` prefix for error codes.

## 0.13.0

- Add the `NgrokError` trait
- Add the `ErrResp` type
- Change the `RpcError::Response` variant to the `ErrResp` type (from `String`)
- Implement `NgrokError` for `ErrResp`, `RpcError`, and `ConnectError`

## 0.12.4

- Add `Win32_Foundation` feature
- Update nix for rust `1.72`

## 0.12.3

- Add `session.id()`

## 0.12.2

- Updated readme and changelog

## 0.12.1

- Add source error on reconnect
- Rename repository to ngrok-rust

## 0.12.0

- Add `client_info` to SessionBuilder
- Update UserAgent generation
- Make `circuit_breaker` test more reliable

## 0.11.3

- Update stream forwarding logic
- Add `ca_cert` option to SessionBuilder
- Unpin `bstr`

## 0.11.2

- Send UserAgent when authenticating
- Update readme documentation

## 0.11.0

- Include a session close method
- Mark errors as non-exhaustive

## 0.10.2

- Update default forwards-to
- Expose OAuth Client ID/Secret setters
- Muxado: close method on the opener

## 0.10.1

- Add windows pipe support
- Require tokio rt

## 0.10.0

- Some api-breaking consistency fixes for the session builder.
- Update the connector to be more in-line with the other handlers and to support
  disconnect/reconnect error reporting.
- Add support for custom heartbeat handlers.

## 0.9.0

- Update docs to match ngrok-go
- Update the tls termination configuration methods to match those in ngrok-go
- Remove the `_string` suffix from the cidr restriction methods

## 0.8.1

- Fix cancellation bugs causing leaked muxado/ngrok sessions.

## 0.8.0

- Some breaking changes to builder method naming for consistency.
- Add dashboard command handlers

## 0.7.0

- Initial crates.io release.

## Pre-0.7.0

- There was originally a crate on crates.io named 'ngrok' that wrapped the agent
  binary. It can be found [here](https://github.com/nkconnor/ngrok).

  Thanks @nkconnor!
