//! FeatureStatus and PresenceUpdate: what each side reports after Hello.

use futures_util::{SinkExt, StreamExt};
use link2linux_core::session::{Availability, FeatureStates, SessionStart};
use link2linux_core::{Feature, Session, SessionConfig, SessionError, SessionEvent};
use link2linux_protocol::Payload;
use link2linux_protocol::v1::{
    ErrorCode, FeatureAvailability, FeatureState, FeatureStatus, NotificationDismissRequest,
    PresenceUpdate,
};

mod session_common;

use session_common::*;

fn presence() -> PresenceUpdate {
    PresenceUpdate {
        ble_address: vec![0xa4, 0xc1, 0x38, 0x01, 0x02, 0x03],
        ..PresenceUpdate::default()
    }
}

fn dismiss() -> Payload {
    Payload::NotificationDismissRequest(NotificationDismissRequest {
        idempotency_key: vec![7; 16],
        ..NotificationDismissRequest::default()
    })
}

#[tokio::test]
async fn reports_its_status_and_presence_after_hello() {
    let mut start = SessionStart::new(desktop());
    start.availability = FeatureStates::from([
        (Feature::Files, Availability::Disabled),
        (
            Feature::Notifications,
            Availability::PermissionRequired("notification-access".into()),
        ),
    ]);
    start.presence = presence();
    let (io, mut peer) = pipe();
    let script = async {
        peer.next().await.unwrap().unwrap();
        peer.send(&envelope(1, Payload::Hello(notifying_phone_hello())))
            .await
            .unwrap();
        read_announcement(&mut peer).await
    };
    let (session, (status, sent_presence)) = tokio::join!(
        Session::handshake(io, start, SessionConfig::default()),
        script
    );
    let session = session.unwrap();

    let state = |feature: &str, availability: FeatureAvailability, permission: &str| FeatureState {
        feature: feature.into(),
        availability: availability.into(),
        permission: permission.into(),
    };
    // Every feature of the desktop's Hello, in its order.
    let expected = [
        state(
            "notifications",
            FeatureAvailability::PermissionRequired,
            "notification-access",
        ),
        state("files", FeatureAvailability::Disabled, ""),
        state("clipboard", FeatureAvailability::Ready, ""),
    ];
    assert_eq!(status.features, expected);
    assert_eq!(sent_presence, presence());
    assert_eq!(
        session.availability()[&Feature::Clipboard],
        Availability::Ready
    );
}

#[tokio::test]
async fn takes_the_peers_status_and_presence() {
    let (mut session, mut peer) =
        connected_to(notifying_phone_hello(), SessionConfig::default()).await;
    assert_eq!(session.peer_availability(), None);
    assert!(!session.is_ready(Feature::Notifications));
    let err = session.start_request(dismiss()).await.unwrap_err();
    assert!(
        matches!(err, SessionError::FeatureNotReady(Feature::Notifications)),
        "{err:?}"
    );

    let status = FeatureStatus {
        features: vec![
            FeatureState {
                feature: "notifications".into(),
                availability: FeatureAvailability::Ready.into(),
                ..FeatureState::default()
            },
            FeatureState {
                feature: "files".into(),
                availability: FeatureAvailability::Disabled.into(),
                ..FeatureState::default()
            },
        ],
    };
    peer.send(&envelope(2, Payload::FeatureStatus(status)))
        .await
        .unwrap();
    peer.send(&envelope(3, Payload::PresenceUpdate(presence())))
        .await
        .unwrap();

    let expected = FeatureStates::from([
        (Feature::Notifications, Availability::Ready),
        (Feature::Files, Availability::Disabled),
    ]);
    let event = session.next_event().await.unwrap();
    assert_eq!(event, Some(SessionEvent::FeatureStatus(expected.clone())));
    let event = session.next_event().await.unwrap();
    assert_eq!(event, Some(SessionEvent::Presence(presence())));
    assert_eq!(session.peer_availability(), Some(&expected));
    assert_eq!(session.peer_presence(), Some(&presence()));
    assert!(session.is_ready(Feature::Notifications));
    assert!(!session.is_ready(Feature::Files));
    assert_eq!(session.start_request(dismiss()).await.unwrap(), 4);
}

#[tokio::test]
async fn sends_changes_only() {
    let (mut session, mut peer) = connected(SessionConfig::default()).await;
    session
        .set_availability(Feature::Clipboard, Availability::Disabled)
        .await
        .unwrap();
    // Unchanged, or not a feature of this device: nothing to send.
    session
        .set_availability(Feature::Clipboard, Availability::Disabled)
        .await
        .unwrap();
    session
        .set_availability(Feature::Screen, Availability::Disabled)
        .await
        .unwrap();
    session
        .set_presence(PresenceUpdate::default())
        .await
        .unwrap();
    session.set_presence(presence()).await.unwrap();
    let err = session
        .send(Payload::FeatureStatus(FeatureStatus::default()))
        .await
        .unwrap_err();
    assert!(
        matches!(err, SessionError::NotAllowed("FeatureStatus")),
        "{err:?}"
    );

    let frame = peer.next().await.unwrap().unwrap();
    let Some(Payload::FeatureStatus(status)) = frame.payload else {
        panic!("expected a FeatureStatus, got {frame:?}");
    };
    assert_eq!(frame.seq, 4);
    // A complete report, not only the change.
    assert_eq!(status.features.len(), 3);
    let clipboard = status.features.iter().find(|s| s.feature == "clipboard");
    assert_eq!(
        clipboard.map(FeatureState::availability),
        Some(FeatureAvailability::Disabled)
    );
    let frame = peer.next().await.unwrap().unwrap();
    assert_eq!(frame, envelope(5, Payload::PresenceUpdate(presence())));
}

#[tokio::test]
async fn refuses_requests_of_features_it_reported_as_not_ready() {
    let mut start = SessionStart::new(desktop());
    start.availability = FeatureStates::from([(
        Feature::Notifications,
        Availability::PermissionRequired("notification-access".into()),
    )]);
    let (io, mut peer) = pipe();
    let script = async {
        peer.next().await.unwrap().unwrap();
        peer.send(&envelope(1, Payload::Hello(notifying_phone_hello())))
            .await
            .unwrap();
        read_announcement(&mut peer).await;
    };
    let (session, ()) = tokio::join!(
        Session::handshake(io, start, SessionConfig::default()),
        script
    );
    let mut session = session.unwrap();

    let reply = answers_to(&mut session, peer, request(2, dismiss()), 1).await;
    assert_eq!(reply[0].reply_to, 2);
    let Some(Payload::ErrorReply(error)) = &reply[0].payload else {
        panic!("expected an ErrorReply, got {reply:?}");
    };
    assert_eq!(error.code(), ErrorCode::PermissionRequired);
    assert_eq!(error.permission, "notification-access");
}
