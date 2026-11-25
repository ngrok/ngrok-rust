## Unreleased

* The heartbeat responder echoes each heartbeat whole even if it arrives in pieces, and stops when the remote ends the heartbeat stream. Before, a short read was echoed padded with zeroes, and after the remote ended the stream the responder kept echoing zeroes.

## 0.3.0

* Rework the heartbeat callback into a handler trait object.

## 0.2.0

* `Heartbeat` no longer `(Ref)?UnwindSafe`. Technically a breaking change,
therefore bumping its minor version.


## 0.1.0

* Initial Public Release