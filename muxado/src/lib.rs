#[macro_use]
mod constrained;

pub mod codec;
pub mod errors;
pub mod frame;
pub mod session;
pub mod stream;
pub mod stream_manager;
pub mod stream_output;
pub mod typed;
pub mod window;

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
