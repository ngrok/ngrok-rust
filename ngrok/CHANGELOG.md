## 0.10.0

* Some api-breaking consistency fixes for the session builder.
* Update the connector to be more in-line with the other handlers and to support
  disconnect/reconnect error reporting.
* Add support for custom heartbeat handlers.

## 0.9.0

* Update docs to match ngrok-go
* Update the tls termination configuration methods to match those in ngrok-go
* Remove the `_string` suffix from the cidr restriction methods

## 0.8.1

* Fix cancellation bugs causing leaked muxado/ngrok sessions.

## 0.8.0

* Some breaking changes to builder method naming for consistency.
* Add dashboard command handlers

## 0.7.0

* Initial crates.io release.

## Pre-0.7.0

* There was originally a crate on crates.io named 'ngrok' that wrapped the agent
  binary. It can be found [here](https://github.com/nkconnor/ngrok).

  Thanks @nkconnor!