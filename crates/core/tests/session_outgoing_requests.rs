//! Requests this side sends: answers, refusals, timeouts and pipelining.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use link2linux_core::session::{Reply, RequestError};
use link2linux_core::{Session, SessionConfig, SessionError, SessionEvent};
use link2linux_protocol::v1::{
    Ack, ErrorCode, ErrorReply, Goodbye, GoodbyeReason, ImageData, ImageRequest,
    NotificationDismissRequest, NotificationSyncBatch, NotificationSyncRequest,
};
use link2linux_protocol::{Payload, Ping, Pong};
use tokio::io::DuplexStream;
use tokio::time::Instant;

mod session_common;

use session_common::*;

fn dismiss() -> Payload {
    Payload::NotificationDismissRequest(NotificationDismissRequest {
        idempotency_key: vec![7; 16],
        created_unix_ms: 1_700_000_000_000,
        key: "0|com.example|1|null|10001".into(),
    })
}

fn image_request() -> Payload {
    Payload::ImageRequest(ImageRequest {
        sha256: vec![1; 32],
    })
}

fn batch(more: bool) -> Payload {
    Payload::NotificationSyncBatch(NotificationSyncBatch {
        more,
        ..NotificationSyncBatch::default()
    })
}

/// A session with notifications active and ready on both sides, and timers
/// that stay out of the way of the request timeout. The phone's
/// `FeatureStatus` has seq 2, so its next frame has seq 3.
async fn notifying() -> (Session<DuplexStream>, RawPeer) {
    let config = SessionConfig {
        ping_interval: Duration::from_secs(600),
        idle_timeout: Duration::from_secs(1800),
        ..SessionConfig::default()
    };
    let (mut session, mut peer) = connected_to(notifying_phone_hello(), config).await;
    let status = all_ready(&["notifications", "clipboard", "files", "messages"]);
    peer.send(&envelope(2, status)).await.unwrap();
    let event = session.next_event().await.unwrap();
    assert!(
        matches!(event, Some(SessionEvent::FeatureStatus(_))),
        "{event:?}"
    );
    (session, peer)
}

/// Reads the next request from the session, checking that it expects an
/// answer, and returns its sequence number.
async fn read_request(peer: &mut RawPeer) -> u64 {
    let frame = peer.next().await.unwrap().unwrap();
    assert!(frame.expects_reply, "{frame:?}");
    assert_eq!(frame.reply_to, 0, "{frame:?}");
    frame.seq
}

/// The next event, which has to be an answer.
async fn next_answer(session: &mut Session<DuplexStream>) -> Reply {
    match session.next_event().await.unwrap() {
        Some(SessionEvent::Reply(reply)) => reply,
        event => panic!("expected a Reply, got {event:?}"),
    }
}

#[tokio::test(start_paused = true)]
async fn next_event_reports_timeouts_and_ignores_late_answers() {
    let (mut session, mut peer) = notifying().await;
    let request = session.start_request(dismiss()).await.unwrap();
    assert_eq!(read_request(&mut peer).await, request);
    let event = session.next_event().await.unwrap();
    assert_eq!(event, Some(SessionEvent::RequestTimedOut { request }));

    peer.send(&answer(3, request, Payload::Ack(Ack {})))
        .await
        .unwrap();
    let ping = envelope(4, Payload::Ping(Ping { nonce: 3 }));
    let reply = answer_to(&mut session, peer, ping).await;
    assert_eq!(reply, envelope(5, Payload::Pong(Pong { nonce: 3 })));
}

#[tokio::test]
async fn pipelined_requests_get_their_own_answers() {
    let (mut session, mut peer) = notifying().await;
    let first = session.start_request(dismiss()).await.unwrap();
    let second = session.start_request(image_request()).await.unwrap();
    assert_eq!((read_request(&mut peer).await, first), (4, 4));
    assert_eq!((read_request(&mut peer).await, second), (5, 5));

    // The peer answers the second request first.
    let image = Payload::ImageData(ImageData::default());
    peer.send(&answer(3, second, image.clone())).await.unwrap();
    peer.send(&answer(4, first, Payload::Ack(Ack {})))
        .await
        .unwrap();

    let reply = next_answer(&mut session).await;
    assert_eq!(
        (reply.request, reply.seq, reply.payload),
        (second, 3, image)
    );
    let reply = next_answer(&mut session).await;
    assert_eq!(
        (reply.request, reply.payload),
        (first, Payload::Ack(Ack {}))
    );
    assert!(reply.last);
}

#[tokio::test]
async fn requests_with_several_answers() {
    let (mut session, mut peer) = notifying().await;
    let sync = Payload::NotificationSyncRequest(NotificationSyncRequest::default());
    let request = session.start_request(sync).await.unwrap();
    read_request(&mut peer).await;
    peer.send(&answer(3, request, batch(true))).await.unwrap();
    peer.send(&answer(4, request, batch(false))).await.unwrap();
    // The request is done after the last batch, so this one is ignored.
    peer.send(&answer(5, request, batch(false))).await.unwrap();

    let first = next_answer(&mut session).await;
    assert_eq!((first.seq, first.last), (3, false));
    let second = next_answer(&mut session).await;
    assert_eq!((second.seq, second.last), (4, true));
    drop(peer);
    assert_eq!(session.next_event().await.unwrap(), None);
}

#[tokio::test]
async fn an_answer_that_does_not_fit_is_a_protocol_error() {
    let (mut session, mut peer) = notifying().await;
    let request = session.start_request(dismiss()).await.unwrap();
    read_request(&mut peer).await;
    let image = Payload::ImageData(ImageData::default());
    peer.send(&answer(3, request, image)).await.unwrap();
    let err = session.next_event().await.unwrap_err();
    assert!(matches!(err, SessionError::Protocol(_)), "{err:?}");
}

#[tokio::test]
async fn only_requests_go_through_the_request_methods() {
    let (mut session, _peer) = notifying().await;
    let ping = Payload::Ping(Ping { nonce: 1 });
    let err = session.start_request(ping).await.unwrap_err();
    assert!(matches!(err, SessionError::NotAllowed("Ping")), "{err:?}");
    let err = session.send(dismiss()).await.unwrap_err();
    assert!(
        matches!(err, SessionError::NotAllowed("NotificationDismissRequest")),
        "{err:?}"
    );
}

#[tokio::test]
async fn send_request_returns_the_answer() {
    let (mut session, mut peer) = notifying().await;
    let script = async {
        let request = read_request(&mut peer).await;
        peer.send(&answer(3, request, Payload::Ack(Ack {})))
            .await
            .unwrap();
        request
    };
    let (reply, request) = tokio::join!(session.send_request(dismiss()), script);
    assert_eq!(request, 4);
    let reply = reply.unwrap();
    assert_eq!((reply.request, reply.seq), (4, 3));
    assert_eq!(reply.payload, Payload::Ack(Ack {}));
    assert!(reply.last);
}

#[tokio::test]
async fn send_request_reports_refusals() {
    let (mut session, mut peer) = notifying().await;
    let error = ErrorReply {
        code: ErrorCode::NotFound.into(),
        detail: "no such notification".into(),
        ..ErrorReply::default()
    };
    let script = async {
        let request = read_request(&mut peer).await;
        let refusal = Payload::ErrorReply(error.clone());
        peer.send(&answer(3, request, refusal)).await.unwrap();
    };
    let (reply, ()) = tokio::join!(session.send_request(dismiss()), script);
    let err = reply.unwrap_err();
    assert!(
        matches!(&err, RequestError::Refused(e) if *e == error),
        "{err:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn send_request_times_out() {
    let (mut session, mut peer) = notifying().await;
    let start = Instant::now();
    let script = async {
        read_request(&mut peer).await;
    };
    let (reply, ()) = tokio::join!(session.send_request(dismiss()), script);
    let err = reply.unwrap_err();
    let timeout = SessionConfig::default().request_timeout;
    assert!(
        matches!(err, RequestError::TimedOut(t) if t == timeout),
        "{err:?}"
    );
    assert!(start.elapsed() >= timeout);
}

#[tokio::test]
async fn next_reply_holds_other_events_back() {
    let (mut session, mut peer) = notifying().await;
    let first = session.start_request(dismiss()).await.unwrap();
    let second = session.start_request(image_request()).await.unwrap();
    let image = Payload::ImageData(ImageData::default());
    peer.send(&answer(3, second, image.clone())).await.unwrap();
    let ping = envelope(4, Payload::Ping(Ping { nonce: 8 }));
    peer.send(&ping).await.unwrap();
    peer.send(&answer(5, first, Payload::Ack(Ack {})))
        .await
        .unwrap();

    let reply = session.next_reply(first).await.unwrap();
    assert_eq!(reply.payload, Payload::Ack(Ack {}));
    // The other answer waited, and the Ping was answered meanwhile.
    let reply = next_answer(&mut session).await;
    assert_eq!((reply.request, reply.payload), (second, image));
    read_request(&mut peer).await;
    read_request(&mut peer).await;
    let pong = peer.next().await.unwrap().unwrap();
    assert_eq!(pong, envelope(6, Payload::Pong(Pong { nonce: 8 })));
}

#[tokio::test]
async fn next_reply_collects_several_answers() {
    let (mut session, mut peer) = notifying().await;
    let sync = Payload::NotificationSyncRequest(NotificationSyncRequest::default());
    let script = async {
        let request = read_request(&mut peer).await;
        peer.send(&answer(3, request, batch(true))).await.unwrap();
        peer.send(&answer(4, request, batch(false))).await.unwrap();
    };
    let (first, ()) = tokio::join!(session.send_request(sync), script);
    let first = first.unwrap();
    assert!(!first.last);
    let second = session.next_reply(first.request).await.unwrap();
    assert_eq!((second.seq, second.last), (4, true));

    let err = session.next_reply(first.request).await.unwrap_err();
    assert!(matches!(err, RequestError::UnknownRequest(4)), "{err:?}");
}

#[tokio::test]
async fn the_session_ending_fails_waiting_requests() {
    let (mut session, mut peer) = notifying().await;
    let script = async {
        read_request(&mut peer).await;
        let goodbye = Goodbye {
            reason: GoodbyeReason::Shutdown.into(),
            ..Goodbye::default()
        };
        peer.send(&envelope(3, Payload::Goodbye(goodbye)))
            .await
            .unwrap();
    };
    let (reply, ()) = tokio::join!(session.send_request(dismiss()), script);
    let err = reply.unwrap_err();
    assert!(matches!(err, RequestError::Ended), "{err:?}");

    // The Goodbye still comes out of next_event.
    let event = session.next_event().await.unwrap();
    assert!(matches!(event, Some(SessionEvent::Goodbye(_))), "{event:?}");
    let err = session.send_request(dismiss()).await.unwrap_err();
    assert!(matches!(err, RequestError::Ended), "{err:?}");
}
