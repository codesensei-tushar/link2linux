//! What a peer can make a session hold: the small frames before the session
//! starts, the Hello's fields, the frames queued for a peer that does not
//! read, and the events kept while waiting for an answer.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use link2linux_core::session::{PingSchedule, RequestError, SessionStart};
use link2linux_core::{Session, SessionConfig, SessionError, SessionEvent};
use link2linux_protocol::v1::{
    NotificationPosted, NotificationSyncRequest, PairingMethod, PairingProof, PhoneNotification,
};
use link2linux_protocol::{
    Envelope, EnvelopeCodec, FrameError, HANDSHAKE_MAX_FRAME_LEN, Hello, Payload,
};
use tokio::io::{AsyncWriteExt, DuplexStream};
use tokio::time::timeout;
use tokio_util::bytes::BytesMut;
use tokio_util::codec::Encoder;

mod session_common;

use session_common::*;

/// Runs the desktop's handshake with `start` against a scripted phone that
/// answers with `hello`.
async fn handshake_with(
    start: SessionStart,
    hello: Hello,
) -> (Result<Session<DuplexStream>, SessionError>, RawPeer) {
    let (io, mut peer) = pipe();
    let script = async move {
        peer.next().await.unwrap().unwrap();
        peer.send(&envelope(1, Payload::Hello(hello)))
            .await
            .unwrap();
        peer
    };
    tokio::join!(
        Session::handshake(io, start, SessionConfig::default()),
        script
    )
}

fn posted(text_len: usize) -> Payload {
    Payload::NotificationPosted(NotificationPosted {
        notification: Some(PhoneNotification {
            key: "0|chat|1".into(),
            text: "x".repeat(text_len),
            ..PhoneNotification::default()
        }),
        alert: false,
    })
}

fn is_handshake_limit(err: &SessionError) -> bool {
    matches!(
        err,
        SessionError::Frame(FrameError::TooLarge {
            max: HANDSHAKE_MAX_FRAME_LEN,
            ..
        })
    )
}

#[tokio::test]
async fn refuses_a_hello_over_the_handshake_limit() {
    let hello = Hello {
        device_name: "x".repeat(HANDSHAKE_MAX_FRAME_LEN),
        ..phone_hello()
    };
    let (result, _peer) = handshake_with(SessionStart::new(desktop()), hello).await;
    let err = result.unwrap_err();
    assert!(is_handshake_limit(&err), "{err:?}");
}

#[tokio::test]
async fn a_pairing_peer_sends_small_frames_until_the_pairing_is_stored() {
    let mut start = SessionStart::new(desktop());
    start.pairing = Some(PairingMethod::QrCode);
    let (session, mut peer) = handshake_with(start.clone(), notifying_phone_hello()).await;
    let mut session = session.unwrap();
    let proof = Payload::PairingProof(PairingProof {
        mac: vec![9; HANDSHAKE_MAX_FRAME_LEN],
    });
    peer.send(&envelope(2, proof)).await.unwrap();
    let err = session.next_event().await.unwrap_err();
    assert!(is_handshake_limit(&err), "{err:?}");

    // Once the pairing is stored, the session takes frames of any size.
    let (session, mut peer) = handshake_with(start, notifying_phone_hello()).await;
    let mut session = session.unwrap();
    session.finish_pairing().await.unwrap();
    read_announcement(&mut peer).await;
    let status = all_ready(&["notifications", "clipboard", "files", "messages"]);
    let script = async {
        peer.send(&envelope(2, status)).await.unwrap();
        peer.send(&envelope(3, posted(200 * 1024))).await.unwrap();
    };
    let events = async {
        let status = session.next_event().await.unwrap();
        (status, session.next_event().await.unwrap())
    };
    let ((), (status, message)) = tokio::join!(script, events);
    assert!(
        matches!(status, Some(SessionEvent::FeatureStatus(_))),
        "{status:?}"
    );
    assert_eq!(
        message,
        Some(SessionEvent::Message {
            seq: 3,
            payload: posted(200 * 1024)
        })
    );
}

#[tokio::test]
async fn a_paired_session_takes_large_frames_right_after_the_hello() {
    let (mut session, mut peer) =
        connected_to(notifying_phone_hello(), SessionConfig::default()).await;
    let large = envelope(2, posted(200 * 1024));
    let (sent, event) = tokio::join!(peer.send(&large), session.next_event());
    sent.unwrap();
    let event = event.unwrap();
    assert!(
        matches!(event, Some(SessionEvent::Message { seq: 2, .. })),
        "{event:?}"
    );
}

#[tokio::test]
async fn refuses_device_ids_that_cannot_be_one() {
    for id in [
        String::new(),
        "x".repeat(65),
        "phone 1".into(),
        "phone\n1".into(),
        "phöne-1".into(),
        "phone/1".into(),
    ] {
        let hello = Hello {
            device_id: id.clone(),
            ..phone_hello()
        };
        let (result, _peer) = handshake_with(SessionStart::new(desktop()), hello).await;
        let err = result.unwrap_err();
        assert!(
            matches!(err, SessionError::InvalidHello(_)),
            "{id:?}: {err:?}"
        );
    }
    // A UUID, and the form of the desktop app up to 0.1.1.
    for id in [
        "6f1c2a9e-3b7d-4e58-9a0c-d2b4f8e1a7c3",
        "desktop-eec6ff5657707ee588785d3a27d2c2a9",
        &"x".repeat(64),
    ] {
        let hello = Hello {
            device_id: id.into(),
            ..phone_hello()
        };
        let (result, _peer) = handshake_with(SessionStart::new(desktop()), hello).await;
        assert_eq!(result.unwrap().peer().id, id);
    }
}

#[tokio::test]
async fn refuses_a_feature_token_longer_than_32_bytes() {
    let mut hello = phone_hello();
    hello.features.push("x".repeat(33));
    let (result, _peer) = handshake_with(SessionStart::new(desktop()), hello).await;
    let err = result.unwrap_err();
    assert!(matches!(err, SessionError::InvalidHello(_)), "{err:?}");
}

#[tokio::test]
async fn shortens_the_peer_name_and_drops_control_characters() {
    let hello = Hello {
        device_name: format!("\u{202e}Evil\n\u{2066}Phone{}", "é".repeat(40)),
        ..phone_hello()
    };
    let (result, _peer) = handshake_with(SessionStart::new(desktop()), hello).await;
    let session = result.unwrap();
    // 64 bytes is "EvilPhone" and 27 of the 2-byte "é"s, cut at a
    // character boundary.
    assert_eq!(session.peer().name, format!("EvilPhone{}", "é".repeat(27)));
}

#[tokio::test]
async fn stops_reading_from_a_peer_that_does_not_read() {
    let (session, peer) = connected(SessionConfig::default()).await;
    let (_unread, mut writer) = tokio::io::split(peer.into_inner());
    let driver = tokio::spawn(async move {
        let mut session = session;
        while let Ok(Some(_)) = session.next_event().await {}
    });

    // Requests of a type the session does not know: it answers each one
    // with an ErrorReply about five times as long, which the peer never
    // reads.
    let mut codec = EnvelopeCodec::new();
    let mut seq = 2;
    let mut written = 0;
    loop {
        let mut chunk = BytesMut::new();
        for _ in 0..1000 {
            let unknown = Envelope {
                seq,
                expects_reply: true,
                ..Envelope::default()
            };
            codec.encode(&unknown, &mut chunk).unwrap();
            seq += 1;
        }
        match timeout(Duration::from_millis(500), writer.write_all(&chunk)).await {
            Ok(result) => result.unwrap(),
            // The session stopped reading.
            Err(_) => break,
        }
        written += chunk.len();
        assert!(
            written < 1024 * 1024,
            "the session read {written} bytes from a peer that reads nothing"
        );
    }
    driver.abort();
}

#[tokio::test(start_paused = true)]
async fn ends_the_session_when_too_much_arrives_while_waiting_for_an_answer() {
    let rare = Duration::from_secs(600);
    let config = SessionConfig {
        ping: Some(PingSchedule {
            active: rare,
            idle: rare,
            missed: 3,
        }),
        ..SessionConfig::default()
    };
    let (mut session, mut peer) = connected_to(notifying_phone_hello(), config).await;
    let status = all_ready(&["notifications", "clipboard", "files", "messages"]);
    peer.send(&envelope(2, status)).await.unwrap();
    session.next_event().await.unwrap();
    let request = session
        .start_request(Payload::NotificationSyncRequest(
            NotificationSyncRequest::default(),
        ))
        .await
        .unwrap();

    // The phone posts notifications instead of answering.
    let flood = async {
        for seq in 3..3 + 1100 {
            peer.feed(&envelope(seq, posted(0))).await.unwrap();
        }
        peer.flush().await.unwrap();
        peer
    };
    let (result, _peer) = tokio::join!(session.next_reply(request), flood);
    let err = result.unwrap_err();
    assert!(
        matches!(err, RequestError::Session(SessionError::Protocol(_))),
        "{err:?}"
    );
}
