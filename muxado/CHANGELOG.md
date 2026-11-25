## Unreleased

* Add `ShutdownHandle`, which forcibly shuts a session down: it stops the session's read and write tasks, fails its open streams and pending opens, ends `accept`, and drops the transport. Only a frame that's already being written gets a moment (250 ms) to finish. Get one from the new `Session::shutdown_handle` and `TypedSession::shutdown_handle` methods. They return `None` by default, so wrappers around a session should forward them.
* An error returned from a `HeartbeatHandler` is now logged and ignored. Before, it ended the heartbeat check task, so the handler was never called again and missed heartbeats went unreported.
* `HeartbeatCtl::set_interval` now changes how often heartbeats are sent. Before, the new interval was used only for the timeout. A shorter interval brings the next heartbeat forward, a longer one applies after the heartbeat that's already due, and intervals under 1 ms, including zero, are treated as 1 ms.
* The heartbeat responder echoes each heartbeat whole even if it arrives in pieces, and stops when the remote ends the heartbeat stream. Before, a short read was echoed padded with zeroes, and after the remote ended the stream the responder kept echoing zeroes.

## 0.3.0

* Rework the heartbeat callback into a handler trait object.

## 0.2.0

* `Heartbeat` no longer `(Ref)?UnwindSafe`. Technically a breaking change,
therefore bumping its minor version.


## 0.1.0

* Initial Public Release