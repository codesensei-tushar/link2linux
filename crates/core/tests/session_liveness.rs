//! Liveness on the spec's cadence: the desktop pings every 15 seconds while
//! active and every 60 while idle, and the phone only answers.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use link2linux_core::{Session, SessionConfig, SessionError};
use link2linux_protocol::{Envelope, Payload};
use tokio::io::DuplexStream;
use tokio::time::Instant;

mod session_common;

use session_common::*;

/// Waits for the session to give up on its silent peer, and returns how
/// long that took, the limit it reports, and the pings the peer received.
async fn until_lost(
    session: &mut Session<DuplexStream>,
    peer: RawPeer,
) -> (Duration, Duration, usize) {
    let start = Instant::now();
    let err = session.next_event().await.unwrap_err();
    let SessionError::PeerSilent(limit) = err else {
        panic!("expected PeerSilent, got {err:?}");
    };
    (start.elapsed(), limit, pings_in(peer).await)
}

/// Counts the pings the session wrote, up to what is buffered now.
async fn pings_in(mut peer: RawPeer) -> usize {
    let mut pings = 0;
    while let Some(Some(Ok(Envelope {
        payload: Some(Payload::Ping(_)),
        ..
    }))) = futures_util::FutureExt::now_or_never(peer.next())
    {
        pings += 1;
    }
    pings
}

#[tokio::test(start_paused = true)]
async fn the_desktop_pings_every_minute_while_idle() {
    let (mut session, peer) = connected(SessionConfig::default()).await;
    assert!(!session.is_active());
    let (elapsed, limit, pings) = until_lost(&mut session, peer).await;
    // Lost after 3 silent intervals, having pinged at 60 and 120 seconds.
    assert_eq!(limit, Duration::from_secs(180));
    assert!(elapsed >= limit && elapsed < limit + Duration::from_secs(1));
    assert!((2..=3).contains(&pings), "{pings} pings");
}

#[tokio::test(start_paused = true)]
async fn activity_pings_at_once_and_every_15_seconds() {
    let (mut session, mut peer) = connected(SessionConfig::default()).await;
    tokio::time::advance(Duration::from_secs(50)).await;
    session.set_active(true).await.unwrap();
    assert!(session.is_active());
    // The first Ping goes out right away.
    let ping = peer.next().await.unwrap().unwrap();
    assert!(matches!(ping.payload, Some(Payload::Ping(_))), "{ping:?}");

    // Silence is counted from the switch, so 50 idle seconds do not count
    // against the shorter limit.
    let (elapsed, limit, pings) = until_lost(&mut session, peer).await;
    assert_eq!(limit, Duration::from_secs(45));
    assert!(elapsed >= limit && elapsed < limit + Duration::from_secs(1));
    assert!((2..=3).contains(&pings), "{pings} pings after the first");
}

#[tokio::test(start_paused = true)]
async fn any_frame_keeps_the_session_alive() {
    let (mut session, mut peer) = connected(SessionConfig::default()).await;
    let start = Instant::now();
    let script = async {
        // Something from the phone every 100 seconds, never a Pong.
        for seq in 2..=4 {
            tokio::time::sleep(Duration::from_secs(100)).await;
            let status = all_ready(&["clipboard"]);
            peer.send(&envelope(seq, status)).await.unwrap();
        }
    };
    let (_, ()) = tokio::join!(
        async {
            for _ in 0..3 {
                session.next_event().await.unwrap();
            }
        },
        script
    );
    assert!(start.elapsed() >= Duration::from_secs(300));
}

#[tokio::test(start_paused = true)]
async fn the_phone_never_pings_and_waits_four_minutes() {
    let (io, mut desk) = pipe();
    let script = async move {
        desk.next().await.unwrap().unwrap();
        let hello = link2linux_protocol::Hello {
            device_id: "desktop-1".into(),
            ..phone_hello()
        };
        desk.send(&envelope(1, Payload::Hello(hello)))
            .await
            .unwrap();
        read_announcement(&mut desk).await;
        desk
    };
    let (session, desk) = tokio::join!(
        Session::handshake(io, phone(), SessionConfig::phone()),
        script
    );
    let mut session = session.unwrap();
    session.set_active(true).await.unwrap();

    let (elapsed, limit, pings) = until_lost(&mut session, desk).await;
    assert_eq!(limit, Duration::from_secs(240));
    assert!(elapsed >= limit && elapsed < limit + Duration::from_secs(1));
    assert_eq!(pings, 0);
}
