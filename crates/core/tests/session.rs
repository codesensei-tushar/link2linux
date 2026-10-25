//! Session tests, run over in-memory pipes.

use std::collections::BTreeSet;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use link2linux_core::{DeviceInfo, DeviceKind, Feature, Session, SessionConfig, SessionError};
use link2linux_protocol::{Envelope, EnvelopeCodec, Hello, PROTOCOL_VERSION, Payload, Ping};
use tokio::io::{DuplexStream, duplex};
use tokio_util::codec::Framed;

/// Far end of a pipe, scripted frame by frame by a test.
type RawPeer = Framed<DuplexStream, EnvelopeCodec>;

fn desktop() -> DeviceInfo {
    DeviceInfo::new(
        "desktop-1",
        "Desk",
        DeviceKind::Desktop,
        [Feature::Notifications, Feature::Clipboard, Feature::Files],
    )
}

fn phone() -> DeviceInfo {
    DeviceInfo::new(
        "phone-1",
        "Pixel",
        DeviceKind::Phone,
        [Feature::Clipboard, Feature::Files, Feature::Messages],
    )
}

fn pipe() -> (DuplexStream, RawPeer) {
    let (local, remote) = duplex(64 * 1024);
    (local, Framed::new(remote, EnvelopeCodec::new()))
}

fn phone_hello() -> Hello {
    Hello {
        protocol_version: PROTOCOL_VERSION,
        device_id: "phone-1".into(),
        device_name: "Pixel".into(),
        kind: DeviceKind::Phone.into(),
        features: vec!["clipboard".into(), "files".into(), "messages".into()],
    }
}

fn envelope(seq: u64, payload: Payload) -> Envelope {
    Envelope {
        seq,
        payload: Some(payload),
    }
}

/// Runs the desktop's handshake against a peer that reads one frame and
/// answers with `reply`.
async fn handshake_against(reply: Envelope) -> Result<Session<DuplexStream>, SessionError> {
    let (io, mut peer) = pipe();
    let script = async move {
        peer.next().await.expect("the desktop's Hello").unwrap();
        peer.send(&reply).await.unwrap();
        peer
    };
    let (result, _peer) = tokio::join!(
        Session::handshake(io, desktop(), SessionConfig::default()),
        script
    );
    result
}

#[tokio::test]
async fn both_sides_learn_each_other() {
    let (a, b) = duplex(64 * 1024);
    let config = SessionConfig::default();
    let (desk, phone_side) = tokio::join!(
        Session::handshake(a, desktop(), config.clone()),
        Session::handshake(b, phone(), config)
    );
    let (desk, phone_side) = (desk.unwrap(), phone_side.unwrap());

    assert_eq!(desk.local(), &desktop());
    assert_eq!(desk.peer(), &phone());
    assert_eq!(phone_side.peer(), &desktop());
    let shared = BTreeSet::from([Feature::Clipboard, Feature::Files]);
    assert_eq!(desk.shared_features(), shared);
    assert_eq!(phone_side.shared_features(), shared);

    desk.close().await.unwrap();
    phone_side.close().await.unwrap();
}

#[tokio::test]
async fn sends_its_hello_as_frame_one() {
    let (io, mut peer) = pipe();
    let script = async move {
        let first = peer.next().await.unwrap().unwrap();
        assert_eq!(first.seq, 1);
        let Some(Payload::Hello(hello)) = first.payload else {
            panic!("expected a Hello, got {first:?}");
        };
        assert_eq!(hello.protocol_version, PROTOCOL_VERSION);
        assert_eq!(hello.device_id, "desktop-1");
        assert_eq!(hello.device_name, "Desk");
        assert_eq!(hello.kind(), DeviceKind::Desktop);
        assert_eq!(hello.features, ["notifications", "files", "clipboard"]);
        peer.send(&envelope(1, Payload::Hello(phone_hello())))
            .await
            .unwrap();
        peer
    };
    let (session, _peer) = tokio::join!(
        Session::handshake(io, desktop(), SessionConfig::default()),
        script
    );
    assert_eq!(session.unwrap().peer().id, "phone-1");
}

#[tokio::test]
async fn ignores_unknown_features_and_device_kinds() {
    let hello = Hello {
        kind: 42,
        features: vec!["clipboard".into(), "teleport".into()],
        ..phone_hello()
    };
    let session = handshake_against(envelope(1, Payload::Hello(hello)))
        .await
        .unwrap();
    assert_eq!(session.peer().kind, DeviceKind::Unspecified);
    assert_eq!(
        session.peer().features,
        BTreeSet::from([Feature::Clipboard])
    );
}

#[tokio::test]
async fn refuses_another_protocol_version() {
    let hello = Hello {
        protocol_version: PROTOCOL_VERSION + 1,
        ..phone_hello()
    };
    let err = handshake_against(envelope(1, Payload::Hello(hello)))
        .await
        .unwrap_err();
    assert!(
        matches!(err, SessionError::VersionMismatch { local, remote }
            if local == PROTOCOL_VERSION && remote == PROTOCOL_VERSION + 1),
        "{err:?}"
    );
}

#[tokio::test]
async fn refuses_a_peer_that_does_not_start_with_hello() {
    let err = handshake_against(envelope(1, Payload::Ping(Ping { nonce: 1 })))
        .await
        .unwrap_err();
    assert!(matches!(err, SessionError::ExpectedHello), "{err:?}");
}

#[tokio::test]
async fn refuses_a_hello_without_device_id() {
    let hello = Hello {
        device_id: String::new(),
        ..phone_hello()
    };
    let err = handshake_against(envelope(1, Payload::Hello(hello)))
        .await
        .unwrap_err();
    assert!(matches!(err, SessionError::InvalidHello(_)), "{err:?}");
}

#[tokio::test]
async fn refuses_sequence_number_zero() {
    let err = handshake_against(envelope(0, Payload::Hello(phone_hello())))
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            SessionError::OutOfOrder {
                previous: 0,
                received: 0
            }
        ),
        "{err:?}"
    );
}

#[tokio::test]
async fn fails_when_the_peer_hangs_up() {
    let (io, mut peer) = pipe();
    let script = async move {
        peer.next().await.unwrap().unwrap();
        drop(peer);
    };
    let (result, ()) = tokio::join!(
        Session::handshake(io, desktop(), SessionConfig::default()),
        script
    );
    let err = result.unwrap_err();
    assert!(
        matches!(err, SessionError::ClosedDuringHandshake),
        "{err:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn times_out_on_a_silent_peer() {
    let (io, _silent_peer) = pipe();
    let config = SessionConfig {
        handshake_timeout: Duration::from_secs(5),
        ..SessionConfig::default()
    };
    let err = Session::handshake(io, desktop(), config).await.unwrap_err();
    assert!(
        matches!(err, SessionError::HandshakeTimeout(t) if t == Duration::from_secs(5)),
        "{err:?}"
    );
}
