//! Session tests, run over in-memory pipes.

use std::collections::BTreeSet;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use link2linux_core::{
    DeviceInfo, DeviceKind, Feature, Session, SessionConfig, SessionError, SessionEvent,
};
use link2linux_protocol::{Envelope, EnvelopeCodec, Hello, PROTOCOL_VERSION, Payload, Ping, Pong};
use tokio::io::{AsyncWriteExt, DuplexStream, duplex};
use tokio::time::Instant;
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

/// Completes the desktop's handshake with a scripted phone.
async fn connected(config: SessionConfig) -> (Session<DuplexStream>, RawPeer) {
    let (io, mut peer) = pipe();
    let script = async move {
        peer.next().await.unwrap().unwrap();
        peer.send(&envelope(1, Payload::Hello(phone_hello())))
            .await
            .unwrap();
        peer
    };
    let (session, peer) = tokio::join!(Session::handshake(io, desktop(), config), script);
    (session.unwrap(), peer)
}

/// Short timers, so paused-time tests stay readable.
fn fast() -> SessionConfig {
    SessionConfig {
        ping_interval: Duration::from_secs(1),
        idle_timeout: Duration::from_secs(3),
        ..SessionConfig::default()
    }
}

/// Sends `ping` from the scripted peer and returns the session's answer.
/// The peer hangs up afterwards, so the session must report a clean close.
async fn answer_to(
    session: &mut Session<DuplexStream>,
    mut peer: RawPeer,
    ping: Envelope,
) -> Envelope {
    let script = async move {
        peer.send(&ping).await.unwrap();
        peer.next().await.unwrap().unwrap()
    };
    let (event, reply) = tokio::join!(session.next_event(), script);
    assert!(matches!(event, Ok(None)), "{event:?}");
    reply
}

#[tokio::test]
async fn answers_pings() {
    let (mut session, peer) = connected(SessionConfig::default()).await;
    let ping = envelope(2, Payload::Ping(Ping { nonce: 42 }));
    let reply = answer_to(&mut session, peer, ping).await;
    assert_eq!(reply, envelope(2, Payload::Pong(Pong { nonce: 42 })));
}

#[tokio::test]
async fn skips_messages_from_newer_versions() {
    let (mut session, mut peer) = connected(SessionConfig::default()).await;
    // Frame with seq 2 and field 50, which this build does not know.
    let unknown = [0x05, 0x08, 0x02, 0x92, 0x03, 0x00];
    peer.get_mut().write_all(&unknown).await.unwrap();
    let ping = envelope(3, Payload::Ping(Ping { nonce: 7 }));
    let reply = answer_to(&mut session, peer, ping).await;
    assert_eq!(reply, envelope(2, Payload::Pong(Pong { nonce: 7 })));
}

#[tokio::test]
async fn measures_round_trip_time() {
    let (mut session, mut peer) = connected(SessionConfig::default()).await;
    assert_eq!(session.rtt(), None);
    session.ping().await.unwrap();
    let ping = peer.next().await.unwrap().unwrap();
    let Some(Payload::Ping(Ping { nonce })) = ping.payload else {
        panic!("expected a Ping, got {ping:?}");
    };
    peer.send(&envelope(2, Payload::Pong(Pong { nonce })))
        .await
        .unwrap();

    let event = session.next_event().await.unwrap();
    assert!(
        matches!(event, Some(SessionEvent::Pong { .. })),
        "{event:?}"
    );
    assert!(session.rtt().is_some());
}

#[tokio::test(start_paused = true)]
async fn idle_sessions_stay_alive_by_pinging() {
    let (a, b) = duplex(64 * 1024);
    let (desk, phone_side) = tokio::join!(
        Session::handshake(a, desktop(), fast()),
        Session::handshake(b, phone(), fast())
    );
    let (mut desk, mut phone_side) = (desk.unwrap(), phone_side.unwrap());
    let start = Instant::now();

    // Nothing but pings flows, for longer than the idle timeout.
    let mut pongs = (0, 0);
    while pongs.0 < 5 || pongs.1 < 5 {
        tokio::select! {
            event = desk.next_event() => {
                assert!(matches!(event, Ok(Some(SessionEvent::Pong { .. }))), "{event:?}");
                pongs.0 += 1;
            }
            event = phone_side.next_event() => {
                assert!(matches!(event, Ok(Some(SessionEvent::Pong { .. }))), "{event:?}");
                pongs.1 += 1;
            }
        }
    }
    assert!(start.elapsed() > fast().idle_timeout);
}

#[tokio::test(start_paused = true)]
async fn a_silent_peer_times_out() {
    let (mut session, mut peer) = connected(fast()).await;
    let start = Instant::now();
    let err = session.next_event().await.unwrap_err();
    assert!(
        matches!(err, SessionError::IdleTimeout(t) if t == fast().idle_timeout),
        "{err:?}"
    );
    assert!(start.elapsed() >= fast().idle_timeout);

    // The session kept pinging while it waited.
    drop(session);
    let mut pings = 0;
    while let Some(frame) = peer.next().await {
        assert!(matches!(frame.unwrap().payload, Some(Payload::Ping(_))));
        pings += 1;
    }
    assert!(pings >= 2, "only {pings} pings");
}

#[tokio::test]
async fn rejects_repeated_sequence_numbers() {
    let (mut session, mut peer) = connected(SessionConfig::default()).await;
    peer.send(&envelope(1, Payload::Ping(Ping { nonce: 1 })))
        .await
        .unwrap();
    let err = session.next_event().await.unwrap_err();
    assert!(
        matches!(
            err,
            SessionError::OutOfOrder {
                previous: 1,
                received: 1
            }
        ),
        "{err:?}"
    );
}

#[tokio::test]
async fn hello_is_only_allowed_in_the_handshake() {
    let (mut session, mut peer) = connected(SessionConfig::default()).await;
    let err = session
        .send(Payload::Hello(phone_hello()))
        .await
        .unwrap_err();
    assert!(matches!(err, SessionError::UnexpectedHello), "{err:?}");

    peer.send(&envelope(2, Payload::Hello(phone_hello())))
        .await
        .unwrap();
    let err = session.next_event().await.unwrap_err();
    assert!(matches!(err, SessionError::UnexpectedHello), "{err:?}");
}
