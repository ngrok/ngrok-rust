use std::io::Write;

use bytes::{
    Buf,
    BufMut,
    BytesMut,
};
use tokio_util::codec::{
    Decoder,
    Encoder,
};
use tracing::instrument;

use super::{
    errors::InvalidHeader,
    frame::*,
};

/// Codec for muxado frames.
#[derive(Default, Debug)]
pub struct FrameCodec {
    // the header has to be read to know how big a frame is.
    // We'll decode it once when we have enough bytes, and then wait for the
    // rest, keeping the already-decoded header around in the meantime to avoid
    // decoding it repeatedly.
    input_header: Option<Header>,
}

#[instrument(level = "trace")]
fn decode_header(mut bs: BytesMut) -> Header {
    let length_type_flags = bs.get_u32();
    let length = ((length_type_flags & 0xFFFFFF00) >> 8).try_into().unwrap();
    let type_flags = length_type_flags as u8;

    Header {
        length,
        typ: ((type_flags & 0xF0) >> 4).into(),
        flags: Flags::from_bits_truncate(type_flags & 0x0F),
        stream_id: StreamID::mask(bs.get_u32()),
    }
}

fn expect_zero_stream_id(header: Header) -> Result<(), InvalidHeader> {
    if header.stream_id != StreamID::clamp(0) {
        Err(InvalidHeader::NonZeroStreamID(header.stream_id))
    } else {
        Ok(())
    }
}

fn expect_non_zero_stream_id(header: Header) -> Result<(), InvalidHeader> {
    if header.stream_id == StreamID::clamp(0) {
        Err(InvalidHeader::ZeroStreamID)
    } else {
        Ok(())
    }
}

fn expect_length(header: Header, length: Length) -> Result<(), InvalidHeader> {
    if header.length != length {
        Err(InvalidHeader::Length {
            expected: length,
            actual: header.length,
        })
    } else {
        Ok(())
    }
}

fn expect_min_length(header: Header, length: Length) -> Result<(), InvalidHeader> {
    if header.length < length {
        Err(InvalidHeader::MinLength {
            expected: length,
            actual: header.length,
        })
    } else {
        Ok(())
    }
}

#[instrument(level = "trace")]
fn validate_header(header: Header) -> Result<(), InvalidHeader> {
    match header.typ {
        HeaderType::Rst => {
            expect_non_zero_stream_id(header)?;
            expect_length(header, Length::clamp(4))?;
        }
        HeaderType::Data => {
            expect_non_zero_stream_id(header)?;
        }
        HeaderType::WndInc => {
            expect_non_zero_stream_id(header)?;
            expect_length(header, Length::clamp(4))?;
        }
        HeaderType::GoAway => {
            expect_zero_stream_id(header)?;
            expect_min_length(header, Length::clamp(8))?;
        }
        HeaderType::Invalid(t) => return Err(InvalidHeader::Type(t)),
    }

    Ok(())
}

#[instrument(level = "trace")]
fn decode_frame(header: Header, mut body: BytesMut) -> Frame {
    if let Err(error) = validate_header(header) {
        return Frame {
            header,
            body: Body::Invalid {
                error,
                body: body.freeze(),
            },
        };
    }

    Frame {
        header,
        body: match header.typ {
            HeaderType::Rst => Body::Rst(ErrorCode::mask(body.get_u32()).into()),
            HeaderType::Data => Body::Data(body.freeze()),
            HeaderType::WndInc => Body::WndInc(WndInc::mask(body.get_u32())),
            HeaderType::GoAway => Body::GoAway {
                last_stream_id: StreamID::mask(body.get_u32()),
                error: ErrorCode::mask(body.get_u32()).into(),
                message: body.freeze(),
            },
            HeaderType::Invalid(t) => Body::Invalid {
                error: InvalidHeader::Type(t),
                body: body.freeze(),
            },
        },
    }
}

impl Decoder for FrameCodec {
    type Item = Frame;
    type Error = std::io::Error;

    #[instrument(level = "trace")]
    fn decode(&mut self, b: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let header = if let Some(header) = self.input_header {
            header
        } else {
            if b.len() < 8 {
                return Ok(None);
            }

            let header = decode_header(b.split_to(8));
            self.input_header = Some(header);
            header
        };

        if b.len() < *header.length as usize {
            return Ok(None);
        }

        let body_bytes = b.split_to(*header.length as usize);

        // Drop the header to get ready for the next frame.
        self.input_header.take();

        Ok(Some(decode_frame(header, body_bytes)))
    }
}

#[instrument(level = "trace")]
fn encode_header(header: Header, buf: &mut BytesMut) {
    // Pack the type into the upper nibble and flags into the lower.
    let type_flags: u8 = ((u8::from(header.typ) << 4) & 0xF0) | (header.flags.bits() & 0x0F);
    // Pack the 24-bit length and packed type & flags into a u32
    let length_type_flags: u32 = (*header.length << 8 & 0xFFFFFF00) | type_flags as u32;

    buf.put_u32(length_type_flags);
    buf.put_u32(*header.stream_id);
}

#[instrument(level = "trace")]
fn encode_body(body: Body, buf: &mut BytesMut) {
    match body {
        Body::Rst(err) => buf.put_u32(*ErrorCode::from(err)),
        Body::Data(data) => buf.writer().write_all(&data).unwrap(),
        Body::WndInc(inc) => buf.put_u32(*inc),
        Body::GoAway {
            last_stream_id,
            error,
            message,
        } => {
            buf.put_u32(*last_stream_id);
            buf.put_u32(*ErrorCode::from(error));
            buf.writer().write_all(&message).unwrap();
        }
        Body::Invalid { body, .. } => buf.writer().write_all(&body).unwrap(),
    }
}

impl Encoder<Frame> for FrameCodec {
    type Error = std::io::Error;

    #[instrument(level = "trace")]
    fn encode(&mut self, frame: Frame, buf: &mut BytesMut) -> Result<(), std::io::Error> {
        validate_header(frame.header)?;
        encode_header(frame.header, buf);
        encode_body(frame.body, buf);
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use bytes::Bytes;

    use super::*;

    #[test]
    fn round_trip() {
        let frame = Frame::from(Body::Data(Bytes::from_static(b"Hello, world!")))
            .stream_id(StreamID::clamp(5));
        let mut buf = bytes::BytesMut::new();
        let mut codec = FrameCodec::default();

        codec
            .encode(frame.clone(), &mut buf)
            .expect("no encode error");

        let decoded = codec
            .decode(&mut buf)
            .expect("no decode error")
            .expect("decoded frame");

        assert_eq!(frame, decoded);
    }
}
