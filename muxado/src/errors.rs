use std::io;

use thiserror::Error;

use crate::frame::{
    ErrorCode,
    Length,
    StreamID,
};

#[repr(u32)]
#[derive(Copy, Clone, Debug, Eq, PartialEq, Error)]
pub enum ErrorType {
    #[error("No Error")]
    NoError = 0x00,
    #[error("Protocol Error")]
    ProtocolError = 0x01,
    #[error("Internal Error")]
    InternalError = 0x02,
    #[error("Flow Control Error")]
    FlowControlError = 0x03,
    #[error("Stream Closed")]
    StreamClosed = 0x04,
    #[error("Stream Refused")]
    StreamRefused = 0x05,
    #[error("Stream Cancelled")]
    StreamCancelled = 0x06,
    #[error("Stream Reset")]
    StreamReset = 0x07,
    #[error("Frame Size Error")]
    FrameSizeError = 0x08,
    #[error("Accept Queue Full")]
    AcceptQueueFull = 0x09,
    #[error("Enhance Your Calm")]
    EnhanceYourCalm = 0x0A,
    #[error("Remote Gone Away")]
    RemoteGoneAway = 0x0B,
    #[error("Streams Exhausted")]
    StreamsExhausted = 0x0C,
    #[error("Write Timeout")]
    WriteTimeout = 0x0D,
    #[error("Session Closed")]
    SessionClosed = 0x0E,
    #[error("Peer EOF")]
    PeerEOF = 0x0F,

    #[error("Unknown Error")]
    ErrorUnknown = u32::MAX,
}

impl From<ErrorType> for ErrorCode {
    fn from(other: ErrorType) -> ErrorCode {
        ErrorCode::mask(other as u32)
    }
}

impl From<ErrorCode> for ErrorType {
    fn from(other: ErrorCode) -> ErrorType {
        use ErrorType::*;
        match *other {
            0x00 => NoError,
            0x01 => ProtocolError,
            0x02 => InternalError,
            0x03 => FlowControlError,
            0x04 => StreamClosed,
            0x05 => StreamRefused,
            0x06 => StreamCancelled,
            0x07 => StreamReset,
            0x08 => FrameSizeError,
            0x09 => AcceptQueueFull,
            0x0A => EnhanceYourCalm,
            0x0B => RemoteGoneAway,
            0x0C => StreamsExhausted,
            0x0D => WriteTimeout,
            0x0E => SessionClosed,
            0x0F => PeerEOF,

            _ => ErrorUnknown,
        }
    }
}

#[derive(Copy, Clone, Debug, Error, PartialEq, Eq)]
pub enum InvalidHeader {
    #[error("StreamID should be non-zero")]
    ZeroStreamID,
    #[error("StreamID should be zero, got {0}")]
    NonZeroStreamID(StreamID),
    #[error("Length should be {expected}, got {actual}")]
    Length { expected: Length, actual: Length },
    #[error("Length should be at least {expected}, got {actual}")]
    MinLength { expected: Length, actual: Length },
    #[error("Invalid frame type: {0}")]
    Type(u8),
}

impl From<InvalidHeader> for io::Error {
    fn from(other: InvalidHeader) -> io::Error {
        io::Error::new(io::ErrorKind::Other, other)
    }
}

impl From<InvalidHeader> for ErrorType {
    fn from(_: InvalidHeader) -> ErrorType {
        ErrorType::ProtocolError
    }
}