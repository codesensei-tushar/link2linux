//! Requests this side sends: answers, refusals, timeouts and pipelining.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use link2linux_core::session::Reply;
use link2linux_core::{Session, SessionConfig, SessionError, SessionEvent};
use link2linux_protocol::v1::{
    Ack, ImageData, ImageRequest, NotificationDismissRequest, NotificationSyncBatch,
    NotificationSyncRequest,
};
use link2linux_protocol::{Payload, Ping, Pong};
use tokio::io::DuplexStream;

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

/// A session with notifications active, and timers that stay out of the
/// way of the request timeout.
async fn notifying() -> (Session<DuplexStream>, RawPeer) {
    let config = SessionConfig {
        ping_interval: Duration::from_secs(600),
        idle_timeout: Duration::from_secs(1800),
        ..SessionConfig::default()
    };
    connected_to(notifying_phone_hello(), config).await
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

    peer.send(&answer(2, request, Payload::Ack(Ack {})))
        .await
        .unwrap();
    let ping = envelope(3, Payload::Ping(Ping { nonce: 3 }));
    let reply = answer_to(&mut session, peer, ping).await;
    assert_eq!(reply, envelope(3, Payload::Pong(Pong { nonce: 3 })));
}

#[tokio::test]
async fn pipelined_requests_get_their_own_answers() {
    let (mut session, mut peer) = notifying().await;
    let first = session.start_request(dismiss()).await.unwrap();
    let second = session.start_request(image_request()).await.unwrap();
    assert_eq!((read_request(&mut peer).await, first), (2, 2));
    assert_eq!((read_request(&mut peer).await, second), (3, 3));

    // The peer answers the second request first.
    let image = Payload::ImageData(ImageData::default());
    peer.send(&answer(2, second, image.clone())).await.unwrap();
    peer.send(&answer(3, first, Payload::Ack(Ack {})))
        .await
        .unwrap();

    let reply = next_answer(&mut session).await;
    assert_eq!(
        (reply.request, reply.seq, reply.payload),
        (second, 2, image)
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
    peer.send(&answer(2, request, batch(true))).await.unwrap();
    peer.send(&answer(3, request, batch(false))).await.unwrap();
    // The request is done after the last batch, so this one is ignored.
    peer.send(&answer(4, request, batch(false))).await.unwrap();

    let first = next_answer(&mut session).await;
    assert_eq!((first.seq, first.last), (2, false));
    let second = next_answer(&mut session).await;
    assert_eq!((second.seq, second.last), (3, true));
    drop(peer);
    assert_eq!(session.next_event().await.unwrap(), None);
}

#[tokio::test]
async fn an_answer_that_does_not_fit_is_a_protocol_error() {
    let (mut session, mut peer) = notifying().await;
    let request = session.start_request(dismiss()).await.unwrap();
    read_request(&mut peer).await;
    let image = Payload::ImageData(ImageData::default());
    peer.send(&answer(2, request, image)).await.unwrap();
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
