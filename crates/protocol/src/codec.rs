//! Length-delimited framing for [`Envelope`]s.
//!
//! A frame is a base-128 varint holding the length of one serialized
//! [`Envelope`], followed by exactly that many bytes. This is the format of
//! protobuf's `writeDelimitedTo` / `parseDelimitedFrom` on the JVM, so the
//! Android app reads and writes the same frames without extra glue.
//!
//! [`EnvelopeCodec`] plugs into `tokio_util::codec::Framed` (or `FramedRead`
//! / `FramedWrite`) to turn any byte stream into a stream of envelopes.

use bytes::{Buf, BytesMut};
use prost::Message;
use tokio_util::codec::{Decoder, Encoder};

use crate::Envelope;

/// Default limit on the size of one frame body: 4 MiB.
///
/// Bulk data (photos, files, screen) is split into chunks well below this,
/// so a bigger frame means a broken or hostile peer.
pub const DEFAULT_MAX_FRAME_LEN: usize = 4 * 1024 * 1024;

/// A varint never needs more than ten bytes (64 bits, 7 per byte).
const MAX_VARINT_LEN: usize = 10;

/// Errors raised while encoding or decoding frames.
///
/// After a decode error the stream is out of sync, so the connection must be
/// closed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum FrameError {
    /// The frame body is longer than the codec's limit.
    #[error("frame of {len} bytes exceeds the limit of {max} bytes")]
    TooLarge {
        /// Body length announced by the prefix, or of the envelope to send.
        len: u64,
        /// The codec's configured limit.
        max: usize,
    },
    /// The length prefix is not a valid varint (longer than ten bytes or
    /// overflowing 64 bits).
    #[error("malformed frame length prefix")]
    InvalidLength,
    /// The frame body is not a valid `Envelope`.
    #[error("frame does not hold a valid Envelope: {0}")]
    Decode(#[from] prost::DecodeError),
    /// The envelope could not be serialized.
    #[error("envelope could not be encoded: {0}")]
    Encode(#[from] prost::EncodeError),
    /// The underlying stream failed.
    #[error("I/O error on the frame stream: {0}")]
    Io(#[from] std::io::Error),
}

/// Encodes and decodes length-delimited [`Envelope`] frames.
///
/// It encodes borrowed envelopes (`Encoder<&Envelope>`), so a sender keeps
/// ownership of what it sent, for example until the peer acknowledges it.
///
/// ```
/// use bytes::BytesMut;
/// use link2linux_protocol::codec::EnvelopeCodec;
/// use link2linux_protocol::{Envelope, Payload, Ping};
/// use tokio_util::codec::{Decoder, Encoder};
///
/// let mut codec = EnvelopeCodec::new();
/// let ping = Envelope {
///     seq: 1,
///     payload: Some(Payload::Ping(Ping { nonce: 9 })),
///     ..Envelope::default()
/// };
/// let mut buf = BytesMut::new();
/// codec.encode(&ping, &mut buf)?;
/// assert_eq!(codec.decode(&mut buf)?, Some(ping));
/// # Ok::<(), link2linux_protocol::codec::FrameError>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnvelopeCodec {
    max_frame_len: usize,
}

impl EnvelopeCodec {
    /// A codec with the [`DEFAULT_MAX_FRAME_LEN`] limit.
    #[must_use]
    pub const fn new() -> Self {
        Self::with_max_frame_len(DEFAULT_MAX_FRAME_LEN)
    }

    /// A codec that rejects frame bodies longer than `max_frame_len` bytes,
    /// in both directions.
    #[must_use]
    pub const fn with_max_frame_len(max_frame_len: usize) -> Self {
        Self { max_frame_len }
    }

    /// The largest frame body this codec accepts, in bytes.
    #[must_use]
    pub const fn max_frame_len(&self) -> usize {
        self.max_frame_len
    }
}

impl Default for EnvelopeCodec {
    fn default() -> Self {
        Self::new()
    }
}

impl Decoder for EnvelopeCodec {
    type Item = Envelope;
    type Error = FrameError;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Envelope>, FrameError> {
        let Some((len, prefix_len)) = peek_varint(src)? else {
            return Ok(None);
        };
        let body_len = usize::try_from(len)
            .ok()
            .filter(|&body_len| body_len <= self.max_frame_len)
            .ok_or(FrameError::TooLarge {
                len,
                max: self.max_frame_len,
            })?;

        let frame_len = prefix_len + body_len;
        if src.len() < frame_len {
            // Make room for the rest of the frame in one allocation.
            src.reserve(frame_len - src.len());
            return Ok(None);
        }

        src.advance(prefix_len);
        let body = src.split_to(body_len);
        Ok(Some(Envelope::decode(body)?))
    }
}

impl Encoder<&Envelope> for EnvelopeCodec {
    type Error = FrameError;

    fn encode(&mut self, item: &Envelope, dst: &mut BytesMut) -> Result<(), FrameError> {
        let len = item.encoded_len();
        if len > self.max_frame_len {
            return Err(FrameError::TooLarge {
                len: len as u64,
                max: self.max_frame_len,
            });
        }
        dst.reserve(prost::length_delimiter_len(len) + len);
        item.encode_length_delimited(dst)?;
        Ok(())
    }
}

/// Reads the varint at the start of `buf` without consuming it.
///
/// Returns the value and the number of bytes it occupies, or `None` when
/// `buf` ends before the varint does.
fn peek_varint(buf: &[u8]) -> Result<Option<(u64, usize)>, FrameError> {
    let mut value = 0u64;
    for (index, &byte) in buf.iter().take(MAX_VARINT_LEN).enumerate() {
        // The tenth byte may only contribute the 64th bit.
        if index == MAX_VARINT_LEN - 1 && byte > 1 {
            return Err(FrameError::InvalidLength);
        }
        value |= u64::from(byte & 0x7f) << (7 * index);
        if byte & 0x80 == 0 {
            return Ok(Some((value, index + 1)));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peek_varint_reads_canonical_and_padded_forms() {
        assert_eq!(peek_varint(&[0x00]).unwrap(), Some((0, 1)));
        assert_eq!(peek_varint(&[0x83, 0x01, 0xff]).unwrap(), Some((131, 2)));
        // Non-minimal encodings are legal protobuf and must be accepted.
        assert_eq!(peek_varint(&[0x81, 0x80, 0x00]).unwrap(), Some((1, 3)));
        let max = [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01];
        assert_eq!(peek_varint(&max).unwrap(), Some((u64::MAX, 10)));
    }

    #[test]
    fn peek_varint_waits_for_more_bytes() {
        assert_eq!(peek_varint(&[]).unwrap(), None);
        assert_eq!(peek_varint(&[0x80, 0x80]).unwrap(), None);
    }

    #[test]
    fn peek_varint_rejects_overlong_prefixes() {
        assert!(matches!(
            peek_varint(&[0xff; 10]),
            Err(FrameError::InvalidLength)
        ));
        let overflow = [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x02];
        assert!(matches!(
            peek_varint(&overflow),
            Err(FrameError::InvalidLength)
        ));
    }
}
