//! Implementation of the muxado protocol.

#![warn(missing_docs)]

#[macro_use]
mod constrained;

mod codec;
mod errors;
mod frame;
mod session;
mod stream;
mod stream_manager;
mod stream_output;
pub mod typed;
mod window;

pub use errors::Error;
pub use session::*;
pub use stream::Stream;

#[cfg(feature = "tokio_rt")]
pub mod heartbeat;

#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
        let result = 2 + 2;
        assert_eq!(result, 4);
    }
}
