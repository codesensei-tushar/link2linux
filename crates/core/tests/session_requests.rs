//! Requests and replies on a session, and messages it does not expect.

use futures_util::{SinkExt, StreamExt};
use link2linux_core::{SessionConfig, SessionError, SessionEvent};
use link2linux_protocol::v1::{
    Ack, ErrorCode, ErrorReply, Goodbye, GoodbyeReason, NotificationDismissRequest,
    NotificationPosted,
};
use link2linux_protocol::{Envelope, Payload, Ping, Pong};
use tokio::io::AsyncWriteExt;

mod session_common;

use session_common::*;

fn dismiss() -> Payload {
    Payload::NotificationDismissRequest(NotificationDismissRequest {
        idempotency_key: vec![7; 16],
        created_unix_ms: 1_700_000_000_000,
        key: "0|com.example|1|null|10001".into(),
    })
}

fn error_reply(code: ErrorCode, detail: &str) -> ErrorReply {
    ErrorReply {
        code: code.into(),
        detail: detail.into(),
        ..ErrorReply::default()
    }
}

/// Checks that `frame` is an `ErrorReply` to `request` with `code`.
fn assert_refused(frame: &Envelope, request: u64, code: ErrorCode) {
    assert_eq!(frame.reply_to, request, "{frame:?}");
    assert!(!frame.expects_reply, "{frame:?}");
    let Some(Payload::ErrorReply(error)) = &frame.payload else {
        panic!("expected an ErrorReply, got {frame:?}");
    };
    assert_eq!(error.code(), code, "{error:?}");
}

#[tokio::test]
async fn answers_unknown_requests_with_unsupported() {
    let (mut session, mut peer) = connected(SessionConfig::default()).await;
    // Frame with seq 2, expects_reply and field 50, which this build does
    // not know.
    let unknown = [0x07, 0x08, 0x02, 0x18, 0x01, 0x92, 0x03, 0x00];
    peer.get_mut().write_all(&unknown).await.unwrap();
    let ping = envelope(3, Payload::Ping(Ping { nonce: 1 }));
    let replies = answers_to(&mut session, peer, ping, 2).await;
    assert_refused(&replies[0], 2, ErrorCode::Unsupported);
    assert_eq!(replies[0].seq, 2);
    assert_eq!(replies[1], envelope(3, Payload::Pong(Pong { nonce: 1 })));
}

#[tokio::test]
async fn hands_requests_over() {
    let (mut session, mut peer) =
        connected_to(notifying_phone_hello(), SessionConfig::default()).await;
    peer.send(&request(2, dismiss())).await.unwrap();
    let event = session.next_event().await.unwrap();
    assert_eq!(
        event,
        Some(SessionEvent::Request {
            seq: 2,
            payload: dismiss()
        })
    );
}

#[tokio::test]
async fn refuses_requests_of_inactive_features() {
    // The phone does not serve notifications, so the feature is inactive.
    let (mut session, peer) = connected(SessionConfig::default()).await;
    let reply = answer_to(&mut session, peer, request(2, dismiss())).await;
    assert_refused(&reply, 2, ErrorCode::Unsupported);
}

#[tokio::test]
async fn ignores_events_of_inactive_features() {
    let (mut session, mut peer) = connected(SessionConfig::default()).await;
    let posted = Payload::NotificationPosted(NotificationPosted::default());
    peer.send(&envelope(2, posted)).await.unwrap();
    let ping = envelope(3, Payload::Ping(Ping { nonce: 4 }));
    let reply = answer_to(&mut session, peer, ping).await;
    assert_eq!(reply, envelope(2, Payload::Pong(Pong { nonce: 4 })));
}

#[tokio::test]
async fn answers_a_message_that_is_not_a_request_but_expects_a_reply() {
    let (mut session, peer) = connected(SessionConfig::default()).await;
    let ping = request(2, Payload::Ping(Ping { nonce: 5 }));
    let replies = answers_to(&mut session, peer, ping, 2).await;
    assert_refused(&replies[0], 2, ErrorCode::Unsupported);
    assert_eq!(replies[1], envelope(3, Payload::Pong(Pong { nonce: 5 })));
}

#[tokio::test]
async fn ignores_answers_nobody_waits_for() {
    let (mut session, mut peer) = connected(SessionConfig::default()).await;
    peer.send(&answer(2, 9, Payload::Ack(Ack {})))
        .await
        .unwrap();
    // An answer to nothing, other than an ErrorReply, is ignored too.
    peer.send(&envelope(3, Payload::Ack(Ack {}))).await.unwrap();
    let ping = envelope(4, Payload::Ping(Ping { nonce: 6 }));
    let reply = answer_to(&mut session, peer, ping).await;
    assert_eq!(reply, envelope(2, Payload::Pong(Pong { nonce: 6 })));
}

#[tokio::test]
async fn misplaced_answers_are_protocol_errors() {
    let cases = [
        // An answer never expects a reply.
        Envelope {
            expects_reply: true,
            ..answer(2, 1, Payload::Ack(Ack {}))
        },
        request(2, Payload::Ack(Ack {})),
        // Only answers carry reply_to.
        answer(2, 1, Payload::Ping(Ping { nonce: 1 })),
    ];
    for frame in cases {
        let (mut session, mut peer) = connected(SessionConfig::default()).await;
        peer.send(&frame).await.unwrap();
        let err = session.next_event().await.unwrap_err();
        assert!(
            matches!(err, SessionError::Protocol(_)),
            "{frame:?}: {err:?}"
        );

        let goodbye = peer.next().await.unwrap().unwrap();
        let Some(Payload::Goodbye(goodbye)) = goodbye.payload else {
            panic!("expected a Goodbye, got {goodbye:?}");
        };
        assert_eq!(goodbye.reason(), GoodbyeReason::ProtocolError);
    }
}

#[tokio::test]
async fn an_error_before_goodbye_explains_it() {
    let (mut session, mut peer) = connected(SessionConfig::default()).await;
    let error = error_reply(ErrorCode::InvalidArgument, "bad seq");
    peer.send(&envelope(2, Payload::ErrorReply(error.clone())))
        .await
        .unwrap();
    let goodbye = Goodbye {
        reason: GoodbyeReason::ProtocolError.into(),
        detail: "bad seq".into(),
    };
    peer.send(&envelope(3, Payload::Goodbye(goodbye)))
        .await
        .unwrap();

    let event = session.next_event().await.unwrap();
    let Some(SessionEvent::Goodbye(goodbye)) = event else {
        panic!("expected a Goodbye, got {event:?}");
    };
    assert_eq!(goodbye.reason, GoodbyeReason::ProtocolError);
    assert_eq!(goodbye.error, Some(error));
}
