//! Session tests, run over in-memory pipes.

use std::collections::BTreeSet;
use std::time::Duration;

use futures_util::{FutureExt, SinkExt, StreamExt};
use link2linux_core::session::{ReconnectPolicy, VersionRange};
use link2linux_core::{
    DeviceInfo, DeviceKind, Feature, Session, SessionConfig, SessionError, SessionEvent,
};
use link2linux_protocol::v1::{Goodbye, GoodbyeReason};
use link2linux_protocol::{Envelope, EnvelopeCodec, Hello, PROTOCOL_VERSION, Payload, Ping, Pong};
use tokio::io::{AsyncWriteExt, DuplexStream, duplex};
use tokio::time::Instant;
use tokio_util::bytes::BytesMut;
use tokio_util::codec::{Encoder, Framed};

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
        ..Hello::default()
    }
}

fn envelope(seq: u64, payload: Payload) -> Envelope {
    Envelope {
        seq,
        payload: Some(payload),
        ..Envelope::default()
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
        assert_eq!(
            VersionRange::of_hello(&hello),
            Some(VersionRange::SUPPORTED)
        );
        assert_eq!(hello.min_protocol_version, PROTOCOL_VERSION);
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
async fn uses_the_highest_shared_version() {
    // A newer peer that still speaks this build's version.
    let hello = Hello {
        protocol_version: PROTOCOL_VERSION + 2,
        min_protocol_version: PROTOCOL_VERSION,
        ..phone_hello()
    };
    let session = handshake_against(envelope(1, Payload::Hello(hello)))
        .await
        .unwrap();
    assert_eq!(session.protocol_version(), PROTOCOL_VERSION);
}

/// Runs the desktop's handshake against a peer that answers with `hello`,
/// and returns the outcome with every frame the peer received after the
/// desktop's `Hello`, up to the end of the stream.
async fn handshake_and_collect(
    hello: Hello,
) -> (Result<Session<DuplexStream>, SessionError>, Vec<Envelope>) {
    let (io, mut peer) = pipe();
    let script = async move {
        peer.next().await.unwrap().unwrap();
        peer.send(&envelope(1, Payload::Hello(hello)))
            .await
            .unwrap();
        let mut received = Vec::new();
        while let Some(Ok(frame)) = peer.next().await {
            received.push(frame);
        }
        received
    };
    tokio::join!(
        Session::handshake(io, desktop(), SessionConfig::default()),
        script
    )
}

#[tokio::test]
async fn says_goodbye_when_no_version_is_shared() {
    let newer = Hello {
        protocol_version: PROTOCOL_VERSION + 2,
        min_protocol_version: PROTOCOL_VERSION + 1,
        ..phone_hello()
    };
    let (result, received) = handshake_and_collect(newer).await;
    let err = result.unwrap_err();
    let SessionError::VersionMismatch { local, remote } = err else {
        panic!("expected a version mismatch, got {err:?}");
    };
    assert_eq!(local, VersionRange::SUPPORTED);
    assert_eq!(
        Some(remote),
        VersionRange::new(PROTOCOL_VERSION + 1, PROTOCOL_VERSION + 2)
    );
    assert!(local.is_older_than(remote));

    // The peer got a Goodbye that says why, then the stream ended.
    let [goodbye] = received.as_slice() else {
        panic!("expected one Goodbye, got {received:?}");
    };
    assert_eq!(goodbye.seq, 2);
    let Some(Payload::Goodbye(goodbye)) = &goodbye.payload else {
        panic!("expected a Goodbye, got {goodbye:?}");
    };
    assert_eq!(goodbye.reason(), GoodbyeReason::VersionMismatch);
}

#[tokio::test]
async fn refuses_a_hello_without_a_version_range() {
    for (min, max) in [(0, 0), (PROTOCOL_VERSION + 1, PROTOCOL_VERSION)] {
        let hello = Hello {
            protocol_version: max,
            min_protocol_version: min,
            ..phone_hello()
        };
        let err = handshake_against(envelope(1, Payload::Hello(hello)))
            .await
            .unwrap_err();
        assert!(matches!(err, SessionError::InvalidHello(_)), "{err:?}");
    }
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

#[tokio::test]
async fn the_peers_goodbye_ends_the_session() {
    let (mut session, mut peer) = connected(SessionConfig::default()).await;
    let goodbye = Goodbye {
        reason: GoodbyeReason::UserDisconnect.into(),
        detail: "the user disconnected".into(),
    };
    peer.send(&envelope(2, Payload::Goodbye(goodbye)))
        .await
        .unwrap();
    // Nothing after the Goodbye is read, not even a broken frame.
    peer.send(&envelope(2, Payload::Ping(Ping { nonce: 1 })))
        .await
        .unwrap();

    let event = session.next_event().await.unwrap();
    let Some(SessionEvent::Goodbye(goodbye)) = event else {
        panic!("expected a Goodbye, got {event:?}");
    };
    assert_eq!(goodbye.reason, GoodbyeReason::UserDisconnect);
    assert_eq!(goodbye.detail, "the user disconnected");
    assert_eq!(goodbye.reconnect_policy(), ReconnectPolicy::WaitForUser);
    assert!(session.has_ended());
    assert!(matches!(session.next_event().await, Ok(None)));
    let err = session.ping().await.unwrap_err();
    assert!(matches!(err, SessionError::Ended), "{err:?}");
}

#[tokio::test]
async fn says_goodbye_on_purpose() {
    let (mut session, mut peer) = connected(SessionConfig::default()).await;
    let err = session
        .send(Payload::Goodbye(Goodbye::default()))
        .await
        .unwrap_err();
    assert!(matches!(err, SessionError::NotAllowed(_)), "{err:?}");

    session
        .goodbye(GoodbyeReason::Shutdown, "quitting")
        .await
        .unwrap();
    let frame = peer.next().await.unwrap().unwrap();
    let expected = Goodbye {
        reason: GoodbyeReason::Shutdown.into(),
        detail: "quitting".into(),
    };
    assert_eq!(frame, envelope(2, Payload::Goodbye(expected)));
    assert!(peer.next().await.is_none());
}

#[tokio::test]
async fn says_goodbye_on_protocol_errors() {
    let (mut session, mut peer) = connected(SessionConfig::default()).await;
    peer.send(&envelope(1, Payload::Ping(Ping { nonce: 1 })))
        .await
        .unwrap();
    let err = session.next_event().await.unwrap_err();
    assert!(err.is_protocol_error(), "{err:?}");
    assert_eq!(err.reconnect_policy(), ReconnectPolicy::Backoff);
    assert!(session.has_ended());

    let frame = peer.next().await.unwrap().unwrap();
    let Some(Payload::Goodbye(goodbye)) = frame.payload else {
        panic!("expected a Goodbye, got {frame:?}");
    };
    assert_eq!(goodbye.reason(), GoodbyeReason::ProtocolError);
    session.close().await.unwrap();
    assert!(peer.next().await.is_none());
}

#[tokio::test]
async fn a_closed_stream_ends_the_session_without_goodbye() {
    let (mut session, peer) = connected(SessionConfig::default()).await;
    drop(peer);
    assert!(matches!(session.next_event().await, Ok(None)));
    assert!(session.has_ended());
}

#[tokio::test]
async fn a_dropped_send_still_goes_out() {
    // A pipe too small for a Ping, so writing one has to wait for the peer.
    let (io, remote) = duplex(4);
    let mut peer = Framed::new(remote, EnvelopeCodec::new());
    let script = async {
        peer.next().await.unwrap().unwrap();
        peer.send(&envelope(1, Payload::Hello(phone_hello())))
            .await
            .unwrap();
    };
    let (session, ()) = tokio::join!(
        Session::handshake(io, desktop(), SessionConfig::default()),
        script
    );
    let mut session = session.unwrap();

    // The Ping is queued before the first await; then the caller gives up.
    assert!(session.ping().now_or_never().is_none());

    // Waiting for events writes it.
    let script = async move {
        let frame = peer.next().await.unwrap().unwrap();
        drop(peer);
        frame
    };
    let (event, frame) = tokio::join!(session.next_event(), script);
    assert!(matches!(event, Ok(None)), "{event:?}");
    assert_eq!(frame.seq, 2);
    assert!(matches!(frame.payload, Some(Payload::Ping(_))));
}

#[tokio::test]
async fn next_event_is_cancel_safe() {
    let (mut session, mut peer) = connected(SessionConfig::default()).await;
    let mut frame = BytesMut::new();
    EnvelopeCodec::new()
        .encode(&envelope(2, Payload::Ping(Ping { nonce: 9 })), &mut frame)
        .unwrap();
    let rest = frame.split_off(3);

    // Give up waiting while only part of a frame has arrived.
    peer.get_mut().write_all(&frame).await.unwrap();
    assert!(session.next_event().now_or_never().is_none());
    peer.get_mut().write_all(&rest).await.unwrap();

    // The frame is still read whole and answered.
    let script = async move { peer.next().await.unwrap().unwrap() };
    let (event, reply) = tokio::join!(session.next_event(), script);
    assert!(matches!(event, Ok(None)), "{event:?}");
    assert_eq!(reply, envelope(2, Payload::Pong(Pong { nonce: 9 })));
}
