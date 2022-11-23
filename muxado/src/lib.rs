#[macro_use]
mod constrained;

mod codec;
pub mod errors;
mod frame;
pub mod session;
pub mod stream;
pub mod stream_manager;
mod stream_output;
pub mod typed;
mod window;

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
