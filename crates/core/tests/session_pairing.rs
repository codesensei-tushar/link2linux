//! Pairing connections, and the device id a peer's key belongs to.

use futures_util::{SinkExt, StreamExt};
use link2linux_core::session::SessionStart;
use link2linux_core::{Session, SessionConfig, SessionError, SessionEvent};
use link2linux_protocol::v1::{
    GoodbyeReason, NotificationPosted, NotificationSyncRequest, PairingMethod, PairingProof,
    PairingResult,
};
use link2linux_protocol::{Envelope, Hello, Payload};
use tokio::io::DuplexStream;

mod session_common;

use session_common::*;

fn proof() -> Payload {
    Payload::PairingProof(PairingProof { mac: vec![9; 32] })
}

/// Runs the desktop's handshake with `start` against a scripted phone that
/// answers with `hello`. Returns the outcome, the desktop's Hello, and the
/// phone.
async fn handshake_with(
    start: SessionStart,
    hello: Hello,
) -> (Result<Session<DuplexStream>, SessionError>, Hello, RawPeer) {
    let (io, mut peer) = pipe();
    let script = async move {
        let first = peer.next().await.unwrap().unwrap();
        let Some(Payload::Hello(desktop_hello)) = first.payload else {
            panic!("expected a Hello, got {first:?}");
        };
        peer.send(&envelope(1, Payload::Hello(hello)))
            .await
            .unwrap();
        (desktop_hello, peer)
    };
    let (session, (desktop_hello, peer)) = tokio::join!(
        Session::handshake(io, start, SessionConfig::default()),
        script
    );
    (session, desktop_hello, peer)
}

/// A desktop that dialed to pair by QR code, connected to a phone that
/// serves notifications.
async fn pairing() -> (Session<DuplexStream>, RawPeer) {
    let mut start = SessionStart::new(desktop());
    start.pairing = Some(PairingMethod::QrCode);
    let (session, hello, peer) = handshake_with(start, notifying_phone_hello()).await;
    assert_eq!(hello.pairing_method(), PairingMethod::QrCode);
    (session.unwrap(), peer)
}

#[tokio::test]
async fn pairing_comes_before_the_status_reports() {
    let (mut session, mut peer) = pairing().await;
    assert!(session.is_pairing());
    assert_eq!(session.pairing_method(), Some(PairingMethod::QrCode));

    // Only pairing messages go either way, and nothing else was sent yet.
    peer.send(&envelope(2, proof())).await.unwrap();
    let event = session.next_event().await.unwrap();
    assert_eq!(
        event,
        Some(SessionEvent::Message {
            seq: 2,
            payload: proof()
        })
    );
    assert_eq!(session.send(proof()).await.unwrap(), 2);
    assert_eq!(peer.next().await.unwrap().unwrap(), envelope(2, proof()));

    let result = Payload::PairingResult(PairingResult::default());
    session.send(result.clone()).await.unwrap();
    session.finish_pairing().await.unwrap();
    assert!(!session.is_pairing());
    assert_eq!(peer.next().await.unwrap().unwrap(), envelope(3, result));
    let status = peer.next().await.unwrap().unwrap();
    assert!(matches!(status.payload, Some(Payload::FeatureStatus(_))));
    let presence = peer.next().await.unwrap().unwrap();
    assert!(matches!(presence.payload, Some(Payload::PresenceUpdate(_))));
    assert_eq!((status.seq, presence.seq), (4, 5));

    // Pairing is over on this side.
    let err = session.send(proof()).await.unwrap_err();
    assert!(
        matches!(err, SessionError::NotAllowed("PairingProof")),
        "{err:?}"
    );
}

#[tokio::test]
async fn feature_traffic_waits_for_the_pairing() {
    let (mut session, mut peer) = pairing().await;
    let posted = Payload::NotificationPosted(NotificationPosted::default());
    let err = session.send(posted.clone()).await.unwrap_err();
    assert!(matches!(err, SessionError::NotAllowed(_)), "{err:?}");
    let sync = Payload::NotificationSyncRequest(NotificationSyncRequest::default());
    let err = session.start_request(sync).await.unwrap_err();
    assert!(matches!(err, SessionError::NotAllowed(_)), "{err:?}");

    // From the peer, before its FeatureStatus, it breaks the protocol.
    peer.send(&envelope(2, posted)).await.unwrap();
    let err = session.next_event().await.unwrap_err();
    assert!(matches!(err, SessionError::Protocol(_)), "{err:?}");
}

#[tokio::test]
async fn the_peers_status_ends_its_pairing() {
    let (mut session, mut peer) = pairing().await;
    peer.send(&envelope(2, all_ready(&["notifications"])))
        .await
        .unwrap();
    let posted = Payload::NotificationPosted(NotificationPosted::default());
    peer.send(&envelope(3, posted.clone())).await.unwrap();
    // A pairing message from the peer after its status is out of place.
    peer.send(&envelope(4, proof())).await.unwrap();

    let event = session.next_event().await.unwrap();
    assert!(
        matches!(event, Some(SessionEvent::FeatureStatus(_))),
        "{event:?}"
    );
    let event = session.next_event().await.unwrap();
    assert_eq!(
        event,
        Some(SessionEvent::Message {
            seq: 3,
            payload: posted
        })
    );
    let err = session.next_event().await.unwrap_err();
    assert!(matches!(err, SessionError::Protocol(_)), "{err:?}");
}

#[tokio::test]
async fn pairing_messages_outside_pairing_break_the_protocol() {
    let (mut session, mut peer) = connected(SessionConfig::default()).await;
    assert!(!session.is_pairing());
    assert_eq!(session.pairing_method(), None);
    let err = session.send(proof()).await.unwrap_err();
    assert!(
        matches!(err, SessionError::NotAllowed("PairingProof")),
        "{err:?}"
    );

    peer.send(&envelope(2, proof())).await.unwrap();
    let err = session.next_event().await.unwrap_err();
    assert!(matches!(err, SessionError::Protocol(_)), "{err:?}");
}

#[tokio::test]
async fn the_phone_side_learns_the_method_from_the_desktop() {
    let (io, mut desk) = pipe();
    let hello = Hello {
        pairing_method: PairingMethod::Sas.into(),
        ..Hello {
            device_id: "desktop-1".into(),
            ..phone_hello()
        }
    };
    let script = async move {
        desk.next().await.unwrap().unwrap();
        desk.send(&envelope(1, Payload::Hello(hello)))
            .await
            .unwrap();
        desk
    };
    let (session, _desk) = tokio::join!(
        Session::handshake(io, phone(), SessionConfig::default()),
        script
    );
    let session = session.unwrap();
    assert!(session.is_pairing());
    assert_eq!(session.pairing_method(), Some(PairingMethod::Sas));
}

#[tokio::test]
async fn checks_the_device_id_against_the_pairing_record() {
    let mut start = SessionStart::new(desktop());
    start.expected_peer_id = Some("phone-1".into());
    let (session, _, _peer) = handshake_with(start.clone(), phone_hello()).await;
    assert_eq!(session.unwrap().peer().id, "phone-1");

    let impostor = Hello {
        device_id: "phone-2".into(),
        ..phone_hello()
    };
    let (session, _, mut peer) = handshake_with(start, impostor).await;
    let err = session.unwrap_err();
    assert!(
        matches!(&err, SessionError::UnexpectedPeer { expected, received }
            if expected == "phone-1" && received == "phone-2"),
        "{err:?}"
    );
    assert!(err.is_protocol_error());
    let goodbye: Envelope = peer.next().await.unwrap().unwrap();
    let Some(Payload::Goodbye(goodbye)) = goodbye.payload else {
        panic!("expected a Goodbye, got {goodbye:?}");
    };
    assert_eq!(goodbye.reason(), GoodbyeReason::ProtocolError);
}
