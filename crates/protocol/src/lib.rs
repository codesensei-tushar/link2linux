//! The link2linux wire format.
//!
//! The message types are generated at build time from the shared schema in
//! `proto/link2linux/v1`, which the Android app compiles too, so both sides
//! always agree on every field. See `docs/protocol.md` for the layers that
//! sit around these messages.
//!
//! [`codec::EnvelopeCodec`] frames envelopes on a byte stream, in the same
//! length-delimited format the Android app uses.
//!
//! The most used types are re-exported at the crate root:
//!
//! ```
//! use link2linux_protocol::{Envelope, Payload, Ping};
//!
//! let envelope = Envelope {
//!     seq: 1,
//!     payload: Some(Payload::Ping(Ping { nonce: 7 })),
//!     ..Envelope::default()
//! };
//! assert!(matches!(envelope.payload, Some(Payload::Ping(_))));
//! ```

#![warn(missing_docs)]

pub mod codec;

/// Version 1 of the wire format, generated from the `link2linux.v1` package.
///
/// Field-level documentation lives in the `.proto` files and is copied into
/// the generated items where the schema has comments.
pub mod v1 {
    #![allow(missing_docs)]
    // `Payload` holds every message inline, so its variants differ in size.
    // Envelopes are short-lived, and boxing would make every match clumsier.
    #![allow(clippy::large_enum_variant)]

    include!(concat!(env!("OUT_DIR"), "/link2linux.v1.rs"));
}

pub use codec::{DEFAULT_MAX_FRAME_LEN, EnvelopeCodec, FrameError};
pub use v1::envelope::Payload;
pub use v1::{DeviceKind, Envelope, Hello, Ping, Pong};

/// The protocol major version this crate speaks.
///
/// It matches the `v1` in the schema's package name and is sent as
/// `Hello.protocol_version`. Peers with a different major refuse the session.
pub const PROTOCOL_VERSION: u32 = 1;

#[cfg(test)]
mod tests {
    use prost::Message;

    use super::*;

    #[test]
    fn generated_types_round_trip() {
        let envelope = Envelope {
            seq: 1,
            payload: Some(Payload::Hello(Hello {
                protocol_version: PROTOCOL_VERSION,
                device_id: "desktop-1".into(),
                device_name: "Desk".into(),
                kind: DeviceKind::Desktop.into(),
                features: vec!["clipboard".into()],
                ..Hello::default()
            })),
            ..Envelope::default()
        };
        let bytes = envelope.encode_to_vec();
        assert_eq!(Envelope::decode(bytes.as_slice()), Ok(envelope));
    }

    #[test]
    fn enum_names_follow_the_schema() {
        assert_eq!(DeviceKind::Phone.as_str_name(), "DEVICE_KIND_PHONE");
        assert_eq!(
            DeviceKind::from_str_name("DEVICE_KIND_DESKTOP"),
            Some(DeviceKind::Desktop)
        );
    }
}
