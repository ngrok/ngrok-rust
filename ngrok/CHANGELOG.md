## Unreleased

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
