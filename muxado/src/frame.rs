/// This module contains the definitions for muxado frames.
///
/// The [`codec`] module is responsible converting between their struct and wire
/// format.
use std::{
    cmp,
    fmt,
    ops::{
        Deref,
        RangeInclusive,
    },
};

use bitflags::bitflags;
use bytes::Bytes;

use crate::{
    constrained::*,
    errors::{
        Error,
        InvalidHeader,
    },
};

pub const WNDINC_MAX: u32 = 0x7FFFFFFF;
pub const STREAMID_MAX: u32 = 0x7FFFFFFF;
pub const LENGTH_MAX: u32 = 0x00FFFFFF;
pub const ERROR_MAX: u32 = u32::MAX;

constrained_num!(StreamID, u32, 0..=STREAMID_MAX, clamp, mask);
constrained_num!(WndInc, u32, 0..=WNDINC_MAX, clamp, mask);
constrained_num!(Length, u32, 0..=LENGTH_MAX, clamp);
constrained_num!(ErrorCode, u32, 0..=ERROR_MAX);

bitflags! {
    #[derive(Default)]
    pub struct Flags: u8 {
        const FIN = 0b0001;
        const SYN = 0b0010;
    }
}

#[derive(Default, Debug, Copy, Clone, Eq, PartialEq)]
pub struct Header {
    pub length: Length,
    pub typ: HeaderType,
    pub flags: Flags,
    pub stream_id: StreamID,
}

#[derive(Debug, Eq, PartialEq, Clone)]
pub struct Frame {
    pub header: Header,
    pub body: Body,
}

impl From<Body> for Frame {
    fn from(mut other: Body) -> Self {
        let typ = other.header_type();
        other.clamp_len();
        let length = Length::clamp(other.len() as u32);
        Frame {
            header: Header {
                length,
                typ,
                ..Default::default()
            },
            body: other,
        }
    }
}

impl Frame {
    pub fn is_fin(&self) -> bool {
        self.header.flags.contains(Flags::FIN)
    }
    pub fn is_syn(&self) -> bool {
        self.header.flags.contains(Flags::SYN)
    }
    pub fn fin(mut self) -> Frame {
        self.header.flags.set(Flags::FIN, true);
        self
    }
    pub fn syn(mut self) -> Frame {
        self.header.flags.set(Flags::SYN, true);
        self
    }
    pub fn stream_id(mut self, id: StreamID) -> Frame {
        self.header.stream_id = id;
        self
    }

    pub fn rst(stream_id: StreamID, error: Error) -> Frame {
        let length = Length::clamp(4);
        Frame {
            header: Header {
                length,
                stream_id,
                typ: HeaderType::Rst,
                ..Default::default()
            },
            body: Body::Rst(error),
        }
    }
    pub fn goaway(last_stream_id: StreamID, error: Error, mut message: Bytes) -> Frame {
        let length = Length::clamp(message.len() as u32);
        message = message.slice(..*length as usize);
        Frame {
            header: Header {
                length,
                typ: HeaderType::GoAway,
                ..Default::default()
            },
            body: Body::GoAway {
                last_stream_id,
                error,
                message,
            },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeaderType {
    Rst,
    Data,
    WndInc,
    GoAway,
    Invalid(u8),
}

impl Default for HeaderType {
    fn default() -> Self {
        HeaderType::Invalid(255)
    }
}

impl From<u8> for HeaderType {
    fn from(other: u8) -> HeaderType {
        match other {
            0 => HeaderType::Rst,
            1 => HeaderType::Data,
            2 => HeaderType::WndInc,
            3 => HeaderType::GoAway,
            t => HeaderType::Invalid(t),
        }
    }
}

impl From<HeaderType> for u8 {
    fn from(other: HeaderType) -> u8 {
        match other {
            HeaderType::Rst => 0,
            HeaderType::Data => 1,
            HeaderType::WndInc => 2,
            HeaderType::GoAway => 3,
            HeaderType::Invalid(t) => t,
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum Body {
    Rst(Error),
    Data(Bytes),
    WndInc(WndInc),
    GoAway {
        last_stream_id: StreamID,
        error: Error,
        message: Bytes,
    },
    Invalid {
        error: InvalidHeader,
        body: Bytes,
    },
}

impl Default for Body {
    fn default() -> Self {
        Body::Data(Default::default())
    }
}

impl Body {
    pub fn header_type(&self) -> HeaderType {
        match self {
            Body::Data(_) => HeaderType::Data,
            Body::GoAway { .. } => HeaderType::GoAway,
            Body::WndInc(_) => HeaderType::WndInc,
            Body::Rst(_) => HeaderType::Rst,
            Body::Invalid { .. } => HeaderType::Invalid(0xff),
        }
    }

    pub fn len(&self) -> usize {
        match self {
            Body::Data(bs) => bs.len(),
            Body::GoAway { .. } => 8,
            Body::WndInc(_) => 4,
            Body::Rst(_) => 4,
            Body::Invalid { body, .. } => body.len(),
        }
    }

    pub fn clamp_len(&mut self) {
        match self {
            Body::Data(bs) => *bs = bs.slice(0..cmp::min(LENGTH_MAX as usize, bs.len())),
            Body::Invalid { body, .. } => {
                *body = body.slice(0..cmp::min(LENGTH_MAX as usize, body.len()))
            }
            _ => {}
        }
    }
}
