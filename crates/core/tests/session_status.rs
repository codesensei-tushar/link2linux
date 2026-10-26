//! FeatureStatus and PresenceUpdate: what each side reports after Hello.

use futures_util::SinkExt;
use link2linux_core::session::{Availability, FeatureStates};
use link2linux_core::{Feature, SessionConfig, SessionEvent};
use link2linux_protocol::Payload;
use link2linux_protocol::v1::{FeatureAvailability, FeatureState, FeatureStatus, PresenceUpdate};

mod session_common;

use session_common::*;

fn presence() -> PresenceUpdate {
    PresenceUpdate {
        ble_address: vec![0xa4, 0xc1, 0x38, 0x01, 0x02, 0x03],
        ..PresenceUpdate::default()
    }
}

#[tokio::test]
async fn takes_the_peers_status_and_presence() {
    let (mut session, mut peer) =
        connected_to(notifying_phone_hello(), SessionConfig::default()).await;
    assert_eq!(session.peer_availability(), None);
    assert_eq!(session.peer_presence(), None);

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
}
