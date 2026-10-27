//! The phone status and ring services on desktop sessions, against phones
//! each test scripts frame by frame.

mod session_common;

use futures_util::{FutureExt, SinkExt, StreamExt};
use link2linux_core::ring::{RING_FOR, RingEnd, RingFailure, RingPhase, RingService};
use link2linux_core::session::{Availability, SessionStart};
use link2linux_core::status::{
    Battery, Cellular, CellularNetwork, PhoneStatus, RingerMode, StatusService, StatusState, Wifi,
};
use link2linux_core::{DeviceInfo, DeviceKind, Feature, Session, SessionConfig, SessionEvent};
use link2linux_protocol::v1::{
    self as wire, Ack, ErrorCode, ErrorReply, FeatureAvailability, FeatureState, FeatureStatus,
    PhoneStatusChanged, PhoneStatusReply, RingEndReason, RingEnded,
};
use link2linux_protocol::{Envelope, Hello, PROTOCOL_VERSION, Payload};
use session_common::{RawPeer, answer, envelope, pipe, read_announcement};
use tokio::io::DuplexStream;
use tokio::time::{Duration, Instant};

use FeatureAvailability::{Disabled, Ready};

/// A desktop that does not ping, so only what a test scripts goes over the
/// pipe.
fn config() -> SessionConfig {
    SessionConfig {
        ping: None,
        ..SessionConfig::default()
    }
}

/// One phone's session, the phone's end of it, and the desktop's two
/// services for that phone.
struct Link {
    session: Session<DuplexStream>,
    phone: RawPeer,
    status: StatusService,
    ring: RingService,
    /// The phone's next sequence number.
    seq: u64,
}

impl Link {
    /// A session with a phone that lists `features` and has not reported
    /// them yet. The desktop shows the phone's status when `shown`.
    async fn connect(features: &[&str], shown: bool) -> Self {
        let desktop = DeviceInfo::new(
            "desktop-1",
            "Desk",
            DeviceKind::Desktop,
            [Feature::Status, Feature::Ring],
        );
        let mut start = SessionStart::new(desktop);
        if !shown {
            start
                .availability
                .insert(Feature::Status, Availability::Disabled);
        }
        let hello = Hello {
            protocol_version: PROTOCOL_VERSION,
            device_id: "phone-1".into(),
            device_name: "Pixel".into(),
            kind: DeviceKind::Phone.into(),
            features: features.iter().map(|&feature| feature.into()).collect(),
            ..Hello::default()
        };
        let (io, mut phone) = pipe();
        let script = async {
            phone.next().await.unwrap().unwrap();
            phone
                .send(&envelope(1, Payload::Hello(hello)))
                .await
                .unwrap();
            read_announcement(&mut phone).await;
        };
        let (session, ()) = tokio::join!(Session::handshake(io, start, config()), script);
        let mut session = session.unwrap();
        let mut status = StatusService::new();
        status.session_started(&mut session).await.unwrap();
        let mut ring = RingService::new();
        ring.session_started(&session);
        Self {
            session,
            phone,
            status,
            ring,
            seq: 2,
        }
    }

    /// A session with a phone that reports both features ready, and has
    /// answered the desktop's status request with [`full`].
    async fn following() -> Self {
        let mut link = Self::connect(&["status", "ring"], true).await;
        link.report(&[("status", Ready), ("ring", Ready)]).await;
        let asked = link.status_request().await;
        link.answer(asked, reply(full())).await;
        assert!(matches!(link.status.state(), StatusState::Current(_)));
        link
    }

    /// The desktop reads its next event and hands it to both services.
    /// Returns what neither took.
    async fn pump(&mut self) -> Option<SessionEvent> {
        let event = self.session.next_event().await.unwrap().unwrap();
        let rest = self
            .status
            .handle_event(&mut self.session, event)
            .await
            .unwrap()?;
        self.ring.handle_event(&self.session, rest)
    }

    /// The phone sends `frame`, and the desktop takes it.
    async fn send(&mut self, frame: Envelope) -> Option<SessionEvent> {
        self.phone.send(&frame).await.unwrap();
        self.seq += 1;
        self.pump().await
    }

    /// The phone sends a one-way `payload`.
    async fn phone_sends(&mut self, payload: Payload) -> Option<SessionEvent> {
        self.send(envelope(self.seq, payload)).await
    }

    /// The phone answers the desktop's request `request` with `payload`.
    async fn answer(&mut self, request: u64, payload: Payload) -> Option<SessionEvent> {
        self.send(answer(self.seq, request, payload)).await
    }

    /// The phone reports its features. The `FeatureStatus` comes back to
    /// the caller from both services.
    async fn report(&mut self, states: &[(&str, FeatureAvailability)]) {
        let status = FeatureStatus {
            features: states
                .iter()
                .map(|&(feature, availability)| FeatureState {
                    feature: feature.into(),
                    availability: availability.into(),
                    ..FeatureState::default()
                })
                .collect(),
        };
        let rest = self.phone_sends(Payload::FeatureStatus(status)).await;
        assert!(
            matches!(rest, Some(SessionEvent::FeatureStatus(_))),
            "{rest:?}"
        );
    }

    /// The next frame the desktop sent.
    async fn sent(&mut self) -> Envelope {
        self.phone.next().await.unwrap().unwrap()
    }

    /// Fails if the desktop sent anything the phone has not read.
    fn sent_nothing(&mut self) {
        if let Some(frame) = self.phone.next().now_or_never() {
            panic!("the desktop sent {frame:?}");
        }
    }

    /// Reads the desktop's `PhoneStatusRequest` and returns its sequence
    /// number.
    async fn status_request(&mut self) -> u64 {
        let frame = self.sent().await;
        assert!(
            matches!(frame.payload, Some(Payload::PhoneStatusRequest(_))) && frame.expects_reply,
            "{frame:?}"
        );
        frame.seq
    }

    /// Reads the desktop's next request, which must be `expected`, and
    /// returns its sequence number.
    async fn request(&mut self, expected: &str) -> u64 {
        let frame = self.sent().await;
        let name = match &frame.payload {
            Some(Payload::RingRequest(_)) => "RingRequest",
            Some(Payload::RingStop(_)) => "RingStop",
            other => panic!("expected {expected}, got {other:?}"),
        };
        assert_eq!(name, expected);
        assert!(frame.expects_reply);
        frame.seq
    }

    /// Reads the desktop's `FeatureStatus` and returns how it reports the
    /// status feature.
    async fn own_status(&mut self) -> FeatureAvailability {
        let frame = self.sent().await;
        let Some(Payload::FeatureStatus(status)) = frame.payload else {
            panic!("expected a FeatureStatus, got {frame:?}");
        };
        let state = status
            .features
            .iter()
            .find(|state| state.feature == "status")
            .unwrap();
        state.availability()
    }

    fn current(&self) -> &PhoneStatus {
        match self.status.state() {
            StatusState::Current(status) => status,
            other => panic!("expected a current status, got {other:?}"),
        }
    }
}

/// A status with every part set.
fn full() -> wire::PhoneStatus {
    wire::PhoneStatus {
        battery: Some(wire::Battery {
            percent: 64,
            plugged_in: false,
            power_saver: true,
        }),
        cellular: Some(wire::Cellular {
            level: 2,
            network: wire::CellularNetwork::CellularNetwork4g.into(),
            operator_name: "Carrier".into(),
        }),
        wifi: Some(wire::Wifi { level: 3 }),
        ringer_mode: wire::RingerMode::Silent.into(),
        do_not_disturb: false,
    }
}

fn reply(status: wire::PhoneStatus) -> Payload {
    Payload::PhoneStatusReply(PhoneStatusReply {
        status: Some(status),
    })
}

fn changed(status: wire::PhoneStatus) -> Payload {
    Payload::PhoneStatusChanged(PhoneStatusChanged {
        status: Some(status),
    })
}

fn charging(percent: u32) -> wire::PhoneStatus {
    let mut status = full();
    status.battery = Some(wire::Battery {
        percent,
        plugged_in: true,
        power_saver: false,
    });
    status
}

fn refusal(code: ErrorCode) -> Payload {
    Payload::ErrorReply(ErrorReply {
        code: code.into(),
        detail: "no".into(),
        ..ErrorReply::default()
    })
}

fn ack() -> Payload {
    Payload::Ack(Ack {})
}

fn ended(reason: RingEndReason) -> Payload {
    Payload::RingEnded(RingEnded {
        reason: reason.into(),
    })
}

#[tokio::test(start_paused = true)]
async fn the_status_is_asked_for_once_ready_and_then_followed() {
    let mut link = Link::connect(&["status", "ring"], true).await;
    // Nothing is asked before the phone says it shares its status.
    assert_eq!(link.status.state(), &StatusState::Waiting);
    link.sent_nothing();

    link.report(&[("status", Ready), ("ring", Ready)]).await;
    let asked = link.status_request().await;
    assert_eq!(link.status.state(), &StatusState::Waiting);

    assert!(link.answer(asked, reply(full())).await.is_none());
    assert_eq!(
        link.current(),
        &PhoneStatus {
            battery: Some(Battery {
                percent: Some(64),
                plugged_in: false,
                power_saver: true,
            }),
            cellular: Some(Cellular {
                level: Some(2),
                network: Some(CellularNetwork::FourG),
                operator_name: Some("Carrier".into()),
            }),
            wifi: Some(Wifi { level: Some(3) }),
            ringer: Some(RingerMode::Silent),
            do_not_disturb: false,
        }
    );

    // Each change replaces the whole status, the newest last.
    assert!(link.phone_sends(changed(charging(65))).await.is_none());
    assert!(link.phone_sends(changed(charging(66))).await.is_none());
    let battery = link.current().battery.unwrap();
    assert_eq!((battery.percent, battery.plugged_in), (Some(66), true));

    // Another feature changing asks nothing again.
    link.report(&[("status", Ready), ("ring", Disabled)]).await;
    link.sent_nothing();
    assert_eq!(link.current().battery.unwrap().percent, Some(66));

    link.status.session_ended();
    assert_eq!(link.status.state(), &StatusState::Disconnected);
}

#[tokio::test(start_paused = true)]
async fn the_status_is_asked_for_again_when_the_feature_is_ready_again() {
    let mut link = Link::following().await;

    link.report(&[("status", Disabled), ("ring", Ready)]).await;
    assert_eq!(link.status.state(), &StatusState::TurnedOff);
    // Changes no longer apply, and are not passed on.
    assert!(link.phone_sends(changed(charging(10))).await.is_none());
    assert_eq!(link.status.state(), &StatusState::TurnedOff);

    link.report(&[("status", Ready), ("ring", Ready)]).await;
    let asked = link.status_request().await;
    assert_eq!(link.status.state(), &StatusState::Waiting);
    link.answer(asked, reply(charging(12))).await;
    assert_eq!(link.current().battery.unwrap().percent, Some(12));
}

#[tokio::test(start_paused = true)]
async fn an_answer_to_an_earlier_request_is_not_taken() {
    let mut link = Link::connect(&["status", "ring"], true).await;
    link.report(&[("status", Ready)]).await;
    let first = link.status_request().await;
    link.report(&[("status", Disabled)]).await;
    link.report(&[("status", Ready)]).await;
    let second = link.status_request().await;
    assert_ne!(first, second);

    let rest = link.answer(first, reply(charging(1))).await;
    assert!(matches!(rest, Some(SessionEvent::Reply(_))), "{rest:?}");
    assert_eq!(link.status.state(), &StatusState::Waiting);
    link.answer(second, reply(charging(2))).await;
    assert_eq!(link.current().battery.unwrap().percent, Some(2));
}

#[tokio::test(start_paused = true)]
async fn a_status_hidden_on_the_desktop_is_not_asked_for() {
    let mut link = Link::connect(&["status", "ring"], false).await;
    link.report(&[("status", Ready), ("ring", Ready)]).await;
    assert_eq!(link.status.state(), &StatusState::Hidden);
    link.sent_nothing();

    // Shown: the phone hears it, and the desktop asks.
    link.status
        .set_shown(&mut link.session, true)
        .await
        .unwrap();
    assert_eq!(link.own_status().await, Ready);
    let asked = link.status_request().await;
    link.answer(asked, reply(full())).await;
    assert_eq!(link.current().battery.unwrap().percent, Some(64));

    // Hidden again: the phone hears it, and its changes no longer apply.
    link.status
        .set_shown(&mut link.session, false)
        .await
        .unwrap();
    assert_eq!(link.own_status().await, Disabled);
    assert_eq!(link.status.state(), &StatusState::Hidden);
    link.phone_sends(changed(charging(3))).await;
    assert_eq!(link.status.state(), &StatusState::Hidden);
    link.sent_nothing();
}

#[tokio::test(start_paused = true)]
async fn a_refused_request_says_why_and_waits_for_ready_again() {
    for (code, state) in [
        (ErrorCode::Disabled, StatusState::TurnedOff),
        (ErrorCode::Unsupported, StatusState::Unsupported),
    ] {
        let mut link = Link::connect(&["status"], true).await;
        link.report(&[("status", Ready)]).await;
        let asked = link.status_request().await;
        link.answer(asked, refusal(code)).await;
        assert_eq!(link.status.state(), &state, "{code:?}");
        // Not asked again, however long the feature stays ready.
        assert_eq!(link.status.next_deadline(), None);
        link.report(&[("status", Ready)]).await;
        link.sent_nothing();
    }
}

impl Link {
    /// Waits for the desktop's retry deadline and lets the service act on
    /// it. Returns how long it waited.
    async fn retry(&mut self) -> Duration {
        let started = Instant::now();
        let deadline = self.status.next_deadline().expect("a retry is planned");
        tokio::time::sleep_until(deadline).await;
        self.status
            .tick(&mut self.session, Instant::now())
            .await
            .unwrap();
        started.elapsed()
    }
}

#[tokio::test(start_paused = true)]
async fn an_unanswered_request_is_asked_again_with_backoff() {
    let mut link = Link::connect(&["status"], true).await;
    link.report(&[("status", Ready)]).await;
    link.status_request().await;
    let started = Instant::now();
    assert!(link.pump().await.is_none());
    assert_eq!(started.elapsed(), Duration::from_secs(30));
    assert_eq!(link.status.state(), &StatusState::Unavailable);

    for wait in [5, 30] {
        assert_eq!(link.retry().await, Duration::from_secs(wait));
        link.status_request().await;
        assert!(link.pump().await.is_none());
    }
    // From then on, every five minutes. The session would end for a phone
    // that has gone silent first, so the wait is read off the deadline.
    let wait = link.status.next_deadline().unwrap() - Instant::now();
    assert_eq!(wait, Duration::from_secs(300));
}

#[tokio::test(start_paused = true)]
async fn a_late_subscription_heals_with_the_next_change() {
    let mut link = Link::connect(&["status"], true).await;
    link.report(&[("status", Ready)]).await;
    link.status_request().await;
    assert!(link.pump().await.is_none());
    assert_eq!(link.status.state(), &StatusState::Unavailable);

    link.phone_sends(changed(charging(7))).await;
    assert_eq!(link.current().battery.unwrap().percent, Some(7));
    // Subscribed after all: no retry is left to send.
    assert_eq!(link.status.next_deadline(), None);
}

#[tokio::test(start_paused = true)]
async fn a_busy_phone_is_asked_again_after_its_retry_after() {
    let mut link = Link::connect(&["status"], true).await;
    link.report(&[("status", Ready)]).await;
    let asked = link.status_request().await;
    link.answer(
        asked,
        Payload::ErrorReply(ErrorReply {
            code: ErrorCode::Busy.into(),
            retry_after_ms: 2_000,
            ..ErrorReply::default()
        }),
    )
    .await;
    assert_eq!(link.status.state(), &StatusState::Unavailable);
    assert_eq!(link.retry().await, Duration::from_secs(2));
    let asked = link.status_request().await;
    link.answer(asked, reply(full())).await;
    assert_eq!(link.current().battery.unwrap().percent, Some(64));
    assert_eq!(link.status.next_deadline(), None);

    // Without retry_after_ms: the backoff.
    let mut link = Link::connect(&["status"], true).await;
    link.report(&[("status", Ready)]).await;
    let asked = link.status_request().await;
    link.answer(asked, refusal(ErrorCode::Unavailable)).await;
    assert_eq!(link.retry().await, Duration::from_secs(5));
    link.status_request().await;
}

#[tokio::test(start_paused = true)]
async fn no_retry_is_sent_once_the_feature_is_not_ready() {
    let mut link = Link::connect(&["status"], true).await;
    link.report(&[("status", Ready)]).await;
    let asked = link.status_request().await;
    link.answer(asked, refusal(ErrorCode::Busy)).await;
    assert!(link.status.next_deadline().is_some());

    link.report(&[("status", Disabled)]).await;
    assert_eq!(link.status.state(), &StatusState::TurnedOff);
    assert_eq!(link.status.next_deadline(), None);
    link.status
        .tick(&mut link.session, Instant::now() + Duration::from_secs(600))
        .await
        .unwrap();
    link.sent_nothing();

    // A session end plans none either.
    let mut link = Link::connect(&["status"], true).await;
    link.report(&[("status", Ready)]).await;
    let asked = link.status_request().await;
    link.answer(asked, refusal(ErrorCode::Busy)).await;
    link.status.session_ended();
    assert_eq!(link.status.next_deadline(), None);
}

#[tokio::test(start_paused = true)]
async fn a_status_out_of_range_shows_as_unknown() {
    let mut link = Link::following().await;
    let mut status = full();
    status.battery.as_mut().unwrap().percent = 150;
    status.wifi = Some(wire::Wifi { level: 9 });
    link.phone_sends(changed(status)).await;
    let current = link.current();
    assert_eq!(current.battery.unwrap().percent, None);
    assert_eq!(current.wifi, Some(Wifi { level: None }));
    assert_eq!(current.cellular.as_ref().unwrap().level, Some(2));

    // A change without a status: every part unknown.
    link.phone_sends(Payload::PhoneStatusChanged(PhoneStatusChanged::default()))
        .await;
    assert_eq!(link.current(), &PhoneStatus::default());
}

#[tokio::test(start_paused = true)]
async fn a_phone_without_the_features_shows_neither() {
    let mut link = Link::connect(&["clipboard"], true).await;
    link.report(&[("clipboard", Ready)]).await;
    assert_eq!(link.status.state(), &StatusState::Unsupported);
    assert_eq!(link.ring.state().blocked, Some(RingFailure::Unsupported));
    link.ring.ring(&mut link.session).await.unwrap();
    assert_eq!(link.ring.state().failure, Some(RingFailure::Unsupported));
    link.sent_nothing();
}

#[tokio::test(start_paused = true)]
async fn the_phone_rings_until_it_says_it_stopped() {
    let mut link = Link::following().await;
    let state = link.ring.state();
    assert_eq!((state.blocked, state.phase), (None, RingPhase::Idle));

    link.ring.ring(&mut link.session).await.unwrap();
    let asked = link.request("RingRequest").await;
    assert_eq!(link.ring.state().phase, RingPhase::Starting);
    assert!(link.answer(asked, ack()).await.is_none());
    assert_eq!(link.ring.state().phase, RingPhase::Ringing);

    assert!(
        link.phone_sends(ended(RingEndReason::StoppedOnPhone))
            .await
            .is_none()
    );
    let state = link.ring.state();
    assert_eq!(state.phase, RingPhase::Idle);
    assert_eq!(state.ended, Some(RingEnd::StoppedOnPhone));
    assert_eq!(link.ring.next_deadline(), None);

    // The next ring forgets how the last one ended.
    link.ring.ring(&mut link.session).await.unwrap();
    assert_eq!(link.ring.state().ended, None);
}

#[tokio::test(start_paused = true)]
async fn the_desktop_stops_the_ringing() {
    let mut link = Link::following().await;
    link.ring.ring(&mut link.session).await.unwrap();
    let asked = link.request("RingRequest").await;
    link.answer(asked, ack()).await;

    link.ring.stop(&mut link.session).await.unwrap();
    let stop = link.request("RingStop").await;
    assert_eq!(link.ring.state().phase, RingPhase::Stopping);
    // A second stop while one waits sends nothing.
    link.ring.stop(&mut link.session).await.unwrap();
    link.sent_nothing();
    // The phone says why it stopped before it answers: no word needed.
    link.phone_sends(ended(RingEndReason::StoppedFromDesktop))
        .await;
    assert_eq!(link.ring.state().phase, RingPhase::Stopping);
    link.answer(stop, ack()).await;
    let state = link.ring.state();
    assert_eq!(
        (state.phase, state.ended, state.failure),
        (RingPhase::Idle, None, None)
    );

    // Nothing to stop: nothing sent.
    link.ring.stop(&mut link.session).await.unwrap();
    link.sent_nothing();
}

#[tokio::test(start_paused = true)]
async fn a_stop_before_the_phone_answers_the_ring_ends_it() {
    let mut link = Link::following().await;
    link.ring.ring(&mut link.session).await.unwrap();
    let ring = link.request("RingRequest").await;
    link.ring.stop(&mut link.session).await.unwrap();
    let stop = link.request("RingStop").await;

    link.answer(ring, ack()).await;
    assert_eq!(link.ring.state().phase, RingPhase::Stopping);
    link.answer(stop, ack()).await;
    assert_eq!(link.ring.state().phase, RingPhase::Idle);
    assert_eq!(link.ring.next_deadline(), None);
}

#[tokio::test(start_paused = true)]
async fn a_minute_without_news_ends_the_ringing_and_a_new_ring_restarts_it() {
    let mut link = Link::following().await;
    link.ring.ring(&mut link.session).await.unwrap();
    let asked = link.request("RingRequest").await;
    link.answer(asked, ack()).await;
    let first = link.ring.next_deadline().unwrap();
    assert_eq!(first, Instant::now() + RING_FOR);

    // Half way, the user rings again: another full minute from the answer.
    tokio::time::advance(Duration::from_secs(30)).await;
    link.ring.tick(Instant::now());
    link.ring.ring(&mut link.session).await.unwrap();
    let again = link.request("RingRequest").await;
    assert_eq!(link.ring.state().phase, RingPhase::Ringing);
    link.answer(again, ack()).await;
    let second = link.ring.next_deadline().unwrap();
    assert_eq!(second, first + Duration::from_secs(30));

    tokio::time::advance(RING_FOR - Duration::from_secs(1)).await;
    link.ring.tick(Instant::now());
    assert_eq!(link.ring.state().phase, RingPhase::Ringing);
    tokio::time::advance(Duration::from_secs(1)).await;
    link.ring.tick(Instant::now());
    let state = link.ring.state();
    assert_eq!((state.phase, state.ended), (RingPhase::Idle, None));
    assert_eq!(link.ring.next_deadline(), None);

    // A late word from the phone changes nothing.
    link.phone_sends(ended(RingEndReason::TimedOut)).await;
    assert_eq!(link.ring.state().ended, None);
}

#[tokio::test(start_paused = true)]
async fn a_refused_ring_says_why() {
    for (code, failure) in [
        (ErrorCode::Disabled, RingFailure::TurnedOff),
        (ErrorCode::Unavailable, RingFailure::Unavailable),
        (ErrorCode::Busy, RingFailure::Unavailable),
        (ErrorCode::Unsupported, RingFailure::Unsupported),
        (ErrorCode::Internal, RingFailure::Failed),
    ] {
        let mut link = Link::following().await;
        link.ring.ring(&mut link.session).await.unwrap();
        let asked = link.request("RingRequest").await;
        link.answer(asked, refusal(code)).await;
        let state = link.ring.state();
        assert_eq!(
            (state.phase, state.failure),
            (RingPhase::Idle, Some(failure))
        );
    }

    // No answer in time.
    let mut link = Link::following().await;
    link.ring.ring(&mut link.session).await.unwrap();
    link.request("RingRequest").await;
    assert!(link.pump().await.is_none());
    let state = link.ring.state();
    assert_eq!(
        (state.phase, state.failure),
        (RingPhase::Idle, Some(RingFailure::Failed))
    );

    // A refused stop: the phone may still ring.
    link.ring.ring(&mut link.session).await.unwrap();
    let asked = link.request("RingRequest").await;
    link.answer(asked, ack()).await;
    link.ring.stop(&mut link.session).await.unwrap();
    let stop = link.request("RingStop").await;
    link.answer(stop, refusal(ErrorCode::Internal)).await;
    let state = link.ring.state();
    assert_eq!(
        (state.phase, state.failure),
        (RingPhase::Ringing, Some(RingFailure::Failed))
    );
}

#[tokio::test(start_paused = true)]
async fn a_phone_with_ringing_turned_off_is_not_asked() {
    let mut link = Link::connect(&["status", "ring"], true).await;
    link.report(&[("status", Disabled), ("ring", Disabled)])
        .await;
    assert_eq!(link.ring.state().blocked, Some(RingFailure::TurnedOff));
    link.ring.ring(&mut link.session).await.unwrap();
    assert_eq!(link.ring.state().failure, Some(RingFailure::TurnedOff));
    link.sent_nothing();

    // Turned on: the block goes, and the next ring clears the failure.
    link.report(&[("status", Disabled), ("ring", Ready)]).await;
    assert_eq!(link.ring.state().blocked, None);
    link.ring.ring(&mut link.session).await.unwrap();
    assert_eq!(link.ring.state().failure, None);
    link.request("RingRequest").await;
}

#[tokio::test(start_paused = true)]
async fn the_end_of_the_session_stops_showing_the_ringing() {
    let mut link = Link::following().await;
    link.ring.ring(&mut link.session).await.unwrap();
    let asked = link.request("RingRequest").await;
    link.answer(asked, ack()).await;
    assert_eq!(link.ring.state().phase, RingPhase::Ringing);

    link.ring.session_ended();
    let state = link.ring.state();
    assert_eq!(
        (state.blocked, state.phase, state.failure),
        (Some(RingFailure::NotConnected), RingPhase::Idle, None)
    );
    assert_eq!(link.ring.next_deadline(), None);

    link.ring.not_connected();
    assert_eq!(link.ring.state().failure, Some(RingFailure::NotConnected));
}
