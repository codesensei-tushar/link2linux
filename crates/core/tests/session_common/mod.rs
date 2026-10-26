//! Helpers shared by the session tests: a desktop session talking to a
//! phone that each test scripts frame by frame over an in-memory pipe.

// Each test binary uses its own subset.
#![allow(dead_code)]

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use link2linux_core::{DeviceInfo, DeviceKind, Feature, Session, SessionConfig};
use link2linux_protocol::v1::{FeatureAvailability, FeatureState, FeatureStatus, PresenceUpdate};
use link2linux_protocol::{Envelope, EnvelopeCodec, Hello, PROTOCOL_VERSION, Payload};
use tokio::io::{DuplexStream, duplex};
use tokio_util::codec::Framed;

/// Far end of a pipe, scripted frame by frame by a test.
pub type RawPeer = Framed<DuplexStream, EnvelopeCodec>;

pub fn desktop() -> DeviceInfo {
    DeviceInfo::new(
        "desktop-1",
        "Desk",
        DeviceKind::Desktop,
        [Feature::Notifications, Feature::Clipboard, Feature::Files],
    )
}

pub fn phone() -> DeviceInfo {
    DeviceInfo::new(
        "phone-1",
        "Pixel",
        DeviceKind::Phone,
        [Feature::Clipboard, Feature::Files, Feature::Messages],
    )
}

pub fn pipe() -> (DuplexStream, RawPeer) {
    let (local, remote) = duplex(64 * 1024);
    (local, Framed::new(remote, EnvelopeCodec::new()))
}

/// The Hello of [`phone`].
pub fn phone_hello() -> Hello {
    Hello {
        protocol_version: PROTOCOL_VERSION,
        device_id: "phone-1".into(),
        device_name: "Pixel".into(),
        kind: DeviceKind::Phone.into(),
        features: vec!["clipboard".into(), "files".into(), "messages".into()],
        ..Hello::default()
    }
}

/// The Hello of a phone that also serves notifications, so the feature is
/// active with [`desktop`].
pub fn notifying_phone_hello() -> Hello {
    let mut hello = phone_hello();
    hello.features.push("notifications".into());
    hello
}

pub fn envelope(seq: u64, payload: Payload) -> Envelope {
    Envelope {
        seq,
        payload: Some(payload),
        ..Envelope::default()
    }
}

/// An envelope that answers the request `reply_to`.
pub fn answer(seq: u64, reply_to: u64, payload: Payload) -> Envelope {
    Envelope {
        reply_to,
        ..envelope(seq, payload)
    }
}

/// An envelope that expects an answer.
pub fn request(seq: u64, payload: Payload) -> Envelope {
    Envelope {
        expects_reply: true,
        ..envelope(seq, payload)
    }
}

/// Completes the desktop's handshake with a scripted phone.
pub async fn connected(config: SessionConfig) -> (Session<DuplexStream>, RawPeer) {
    connected_to(phone_hello(), config).await
}

/// Completes the desktop's handshake with a scripted phone that sends
/// `hello`. The phone reads the desktop's `FeatureStatus` (seq 2) and
/// `PresenceUpdate` (seq 3), and sends neither of its own, so the next
/// frame from the desktop has seq 4 and the next one from the phone seq 2.
pub async fn connected_to(hello: Hello, config: SessionConfig) -> (Session<DuplexStream>, RawPeer) {
    let (io, mut peer) = pipe();
    let script = async move {
        peer.next().await.unwrap().unwrap();
        peer.send(&envelope(1, Payload::Hello(hello)))
            .await
            .unwrap();
        read_announcement(&mut peer).await;
        peer
    };
    let (session, peer) = tokio::join!(Session::handshake(io, desktop(), config), script);
    (session.unwrap(), peer)
}

/// Reads what a session sends right after the `Hello`s: `FeatureStatus`
/// with seq 2, then `PresenceUpdate` with seq 3.
pub async fn read_announcement(peer: &mut RawPeer) -> (FeatureStatus, PresenceUpdate) {
    let status = peer.next().await.unwrap().unwrap();
    let presence = peer.next().await.unwrap().unwrap();
    match (status, presence) {
        (
            Envelope {
                seq: 2,
                payload: Some(Payload::FeatureStatus(status)),
                ..
            },
            Envelope {
                seq: 3,
                payload: Some(Payload::PresenceUpdate(presence)),
                ..
            },
        ) => (status, presence),
        frames => panic!("expected FeatureStatus and PresenceUpdate, got {frames:?}"),
    }
}

/// A `FeatureStatus` that reports every feature in `features` as ready.
pub fn all_ready(features: &[&str]) -> Payload {
    Payload::FeatureStatus(FeatureStatus {
        features: features
            .iter()
            .map(|&feature| FeatureState {
                feature: feature.into(),
                availability: FeatureAvailability::Ready.into(),
                ..FeatureState::default()
            })
            .collect(),
    })
}

/// Short timers, so paused-time tests stay readable.
pub fn fast() -> SessionConfig {
    SessionConfig {
        ping_interval: Duration::from_secs(1),
        idle_timeout: Duration::from_secs(3),
        ..SessionConfig::default()
    }
}

/// Sends `frame` from the scripted peer and returns the session's answer.
/// The peer hangs up afterwards, so the session must report a clean close.
pub async fn answer_to(
    session: &mut Session<DuplexStream>,
    peer: RawPeer,
    frame: Envelope,
) -> Envelope {
    let [reply] = answers_to(session, peer, frame, 1)
        .await
        .try_into()
        .unwrap();
    reply
}

/// Sends `frame` from the scripted peer and returns the first `count`
/// frames the session writes back. The peer hangs up afterwards, so the
/// session must report a clean close.
pub async fn answers_to(
    session: &mut Session<DuplexStream>,
    mut peer: RawPeer,
    frame: Envelope,
    count: usize,
) -> Vec<Envelope> {
    let script = async move {
        peer.send(&frame).await.unwrap();
        let mut replies = Vec::new();
        while replies.len() < count {
            replies.push(peer.next().await.unwrap().unwrap());
        }
        replies
    };
    let (event, replies) = tokio::join!(session.next_event(), script);
    assert!(matches!(event, Ok(None)), "{event:?}");
    replies
}
