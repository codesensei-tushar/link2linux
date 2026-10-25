//! Framing tests for `EnvelopeCodec`, including the byte-level vector in
//! `proto/testdata/v1/frames.bin` that the Android tests check as well.

use bytes::{BufMut, BytesMut};
use futures_util::{SinkExt, StreamExt};
use link2linux_protocol::{
    DeviceKind, Envelope, EnvelopeCodec, FrameError, Hello, PROTOCOL_VERSION, Payload, Ping, Pong,
};
use tokio::io::AsyncWriteExt;
use tokio_util::codec::{Decoder, Encoder, FramedRead, FramedWrite};

/// The shared vector; see `proto/testdata/README.md`.
const FRAMES_V1: &[u8] = include_bytes!("../../../proto/testdata/v1/frames.bin");

/// The envelopes in `FRAMES_V1`, as written in `proto/testdata/v1/*.txtpb`.
fn vector_envelopes() -> Vec<Envelope> {
    let nonce = 1_234_567_890_123;
    let features = [
        "notifications",
        "messages",
        "calls",
        "photos",
        "files",
        "clipboard",
        "screen",
    ];
    vec![
        Envelope {
            seq: 1,
            payload: Some(Payload::Hello(Hello {
                protocol_version: PROTOCOL_VERSION,
                device_id: "6f1c2a9e-3b7d-4e58-9a0c-d2b4f8e1a7c3".into(),
                device_name: "Büro workstation".into(),
                kind: DeviceKind::Desktop.into(),
                features: features.map(String::from).to_vec(),
            })),
        },
        Envelope {
            seq: 2,
            payload: Some(Payload::Ping(Ping { nonce })),
        },
        Envelope {
            seq: 3,
            payload: Some(Payload::Pong(Pong { nonce })),
        },
    ]
}

fn encode_all(codec: &mut EnvelopeCodec, envelopes: &[Envelope]) -> BytesMut {
    let mut buf = BytesMut::new();
    for envelope in envelopes {
        codec.encode(envelope, &mut buf).expect("encode");
    }
    buf
}

fn decode_all(codec: &mut EnvelopeCodec, buf: &mut BytesMut) -> Vec<Envelope> {
    let mut envelopes = Vec::new();
    while let Some(envelope) = codec.decode(buf).expect("decode") {
        envelopes.push(envelope);
    }
    envelopes
}

#[test]
fn shared_vector_decodes_to_the_expected_envelopes() {
    let mut buf = BytesMut::from(FRAMES_V1);
    assert_eq!(
        decode_all(&mut EnvelopeCodec::new(), &mut buf),
        vector_envelopes()
    );
    assert!(buf.is_empty(), "no bytes may be left over");
}

#[test]
fn shared_vector_matches_our_encoding_byte_for_byte() {
    let buf = encode_all(&mut EnvelopeCodec::new(), &vector_envelopes());
    assert_eq!(&buf[..], FRAMES_V1);
}

#[test]
fn round_trips_a_sequence_of_frames() {
    let mut codec = EnvelopeCodec::new();
    let envelopes = vector_envelopes();
    let mut buf = encode_all(&mut codec, &envelopes);
    assert_eq!(decode_all(&mut codec, &mut buf), envelopes);
    assert!(buf.is_empty());
}

#[test]
fn round_trips_an_empty_envelope() {
    // Every field at its default value encodes to nothing: the frame is a
    // lone zero length prefix.
    let mut codec = EnvelopeCodec::new();
    let mut buf = encode_all(&mut codec, &[Envelope::default()]);
    assert_eq!(&buf[..], [0x00]);
    assert_eq!(codec.decode(&mut buf).unwrap(), Some(Envelope::default()));
}

#[test]
fn waits_for_partial_frames() {
    // Feed the vector one byte at a time. A frame decodes only once its last
    // byte is in, including across the first frame's two-byte length prefix.
    let mut codec = EnvelopeCodec::new();
    let mut buf = BytesMut::new();
    let mut decoded = Vec::new();
    let mut frame_ends = Vec::new();
    for (index, &byte) in FRAMES_V1.iter().enumerate() {
        buf.put_u8(byte);
        if let Some(envelope) = codec.decode(&mut buf).unwrap() {
            decoded.push(envelope);
            frame_ends.push(index + 1);
            assert!(buf.is_empty());
        }
    }
    assert_eq!(decoded, vector_envelopes());
    // 2 + 131 bytes of Hello, then two frames of 1 + 11 bytes.
    assert_eq!(frame_ends, [133, 145, 157]);
}

#[test]
fn accepts_frames_at_the_limit() {
    let ping = vector_envelopes().swap_remove(1);
    let mut codec = EnvelopeCodec::with_max_frame_len(11);
    let mut buf = encode_all(&mut codec, std::slice::from_ref(&ping));
    assert_eq!(codec.decode(&mut buf).unwrap(), Some(ping));
}

#[test]
fn rejects_oversize_frames_on_decode_before_buffering_the_body() {
    let mut codec = EnvelopeCodec::with_max_frame_len(16);
    // Only the prefix has arrived; it announces 17 bytes.
    let mut buf = BytesMut::from(&[17u8][..]);
    let err = codec.decode(&mut buf).unwrap_err();
    assert!(matches!(err, FrameError::TooLarge { len: 17, max: 16 }));

    let mut huge =
        BytesMut::from(&[0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01][..]);
    let err = EnvelopeCodec::new().decode(&mut huge).unwrap_err();
    assert!(matches!(err, FrameError::TooLarge { len: u64::MAX, .. }));
}

#[test]
fn rejects_oversize_frames_on_encode() {
    let mut codec = EnvelopeCodec::with_max_frame_len(10);
    let mut buf = BytesMut::new();
    let err = codec.encode(&vector_envelopes()[1], &mut buf).unwrap_err();
    assert!(matches!(err, FrameError::TooLarge { len: 11, max: 10 }));
    assert!(buf.is_empty(), "a rejected frame writes nothing");
}

#[test]
fn rejects_malformed_prefixes_and_bodies() {
    let mut codec = EnvelopeCodec::new();
    let mut overlong = BytesMut::from(&[0x80; 10][..]);
    assert!(matches!(
        codec.decode(&mut overlong),
        Err(FrameError::InvalidLength)
    ));

    // A two-byte body holding a truncated field tag.
    let mut garbage = BytesMut::from(&[0x02, 0xff, 0xff][..]);
    assert!(matches!(
        codec.decode(&mut garbage),
        Err(FrameError::Decode(_))
    ));
}

#[tokio::test]
async fn streams_frames_through_a_narrow_pipe() {
    // A 7-byte pipe makes every frame arrive in several reads.
    let (client, server) = tokio::io::duplex(7);
    let mut writer = FramedWrite::new(client, EnvelopeCodec::new());
    let mut reader = FramedRead::new(server, EnvelopeCodec::new());
    let envelopes = vector_envelopes();

    let send = async {
        for envelope in &envelopes {
            writer.send(envelope).await.unwrap();
        }
        writer.close().await.unwrap();
    };
    let receive = async {
        let mut received = Vec::new();
        while let Some(envelope) = reader.next().await {
            received.push(envelope.unwrap());
        }
        received
    };
    let ((), received) = tokio::join!(send, receive);
    assert_eq!(received, envelopes);
}

#[tokio::test]
async fn a_stream_that_ends_mid_frame_is_an_error() {
    let (mut client, server) = tokio::io::duplex(64);
    client.write_all(&FRAMES_V1[..20]).await.unwrap();
    drop(client);

    let mut reader = FramedRead::new(server, EnvelopeCodec::new());
    let err = reader.next().await.expect("an item").unwrap_err();
    assert!(matches!(err, FrameError::Io(_)));
}
