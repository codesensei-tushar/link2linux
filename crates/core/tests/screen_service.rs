//! The screen service on desktop sessions, against phones each test scripts
//! frame by frame.

mod session_common;

use futures_util::{FutureExt, SinkExt, StreamExt};
use link2linux_core::screen::{
    Codec, FRAMES_WAITING, Input, InputProblem, KEY_FRAME_EVERY, Key, Point, ScreenEnd,
    ScreenPhase, ScreenProblem, ScreenRequest, ScreenService, VideoFrame, hand_off,
};
use link2linux_core::session::SessionStart;
use link2linux_core::{DeviceInfo, DeviceKind, Feature, Session, SessionConfig, SessionEvent};
use link2linux_protocol::v1::{
    self as wire, Ack, ErrorCode, ErrorReply, FeatureAvailability, FeatureState, FeatureStatus,
    ScreenCodec, ScreenEndReason, screen_input,
};
use link2linux_protocol::{Envelope, Hello, PROTOCOL_VERSION, Payload};
use session_common::{RawPeer, answer, envelope, pipe, read_announcement};
use tokio::io::DuplexStream;
use tokio::sync::mpsc::Receiver;
use tokio::sync::mpsc::error::TryRecvError;
use tokio::time::Instant;

use FeatureAvailability::{Disabled, Ready};

/// A desktop that does not ping, so only what a test scripts goes over the
/// pipe.
fn config() -> SessionConfig {
    SessionConfig {
        ping: None,
        ..SessionConfig::default()
    }
}

/// One phone's session, the phone's end of it, and the desktop's screen
/// service for that phone.
struct Link {
    session: Session<DuplexStream>,
    phone: RawPeer,
    screen: ScreenService,
    /// The phone's next sequence number.
    seq: u64,
}

impl Link {
    /// A session with a phone that lists `features` and has not reported
    /// them yet.
    async fn connect(features: &[&str]) -> Self {
        let desktop = DeviceInfo::new("desktop-1", "Desk", DeviceKind::Desktop, [Feature::Screen]);
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
        let (session, ()) = tokio::join!(
            Session::handshake(io, SessionStart::new(desktop), config()),
            script
        );
        let session = session.unwrap();
        let mut screen = ScreenService::new();
        screen.session_started(&session);
        Self {
            session,
            phone,
            screen,
            seq: 2,
        }
    }

    /// A session with a phone that reports the screen ready.
    async fn ready() -> Self {
        let mut link = Self::connect(&["screen"]).await;
        link.report(Ready).await;
        assert_eq!(link.screen.state().blocked, None);
        link
    }

    /// A stream that shows `format`, with nothing sent to the viewer yet.
    async fn showing(format: Payload) -> (Self, Receiver<VideoFrame>) {
        let mut link = Self::ready().await;
        let viewer = link.start();
        let start = link.request("ScreenStart").await;
        link.answer(start, ack()).await;
        assert_eq!(link.screen.state().phase, ScreenPhase::Asking);
        assert_eq!(link.phone_sends(format).await, None);
        assert_eq!(link.screen.state().phase, ScreenPhase::Showing);
        (link, viewer)
    }

    /// The desktop asks for the screen in H.264 or JPEG, up to 1920
    /// pixels.
    fn start(&mut self) -> Receiver<VideoFrame> {
        let (frames, viewer) = hand_off();
        let request = ScreenRequest {
            codecs: vec![Codec::H264, Codec::Jpeg],
            max_size: 1920,
        };
        self.screen
            .start(&mut self.session, &request, frames)
            .unwrap();
        viewer
    }

    /// The desktop reads its next event and hands it to the service.
    /// Returns what it did not take.
    async fn pump(&mut self) -> Option<SessionEvent> {
        let event = self.session.next_event().await.unwrap().unwrap();
        self.screen.handle_event(&mut self.session, event).unwrap()
    }

    /// The phone sends a one-way `payload`, and the desktop takes it.
    async fn phone_sends(&mut self, payload: Payload) -> Option<SessionEvent> {
        self.phone.send(&envelope(self.seq, payload)).await.unwrap();
        self.seq += 1;
        self.pump().await
    }

    /// The phone answers the desktop's request `request` with `payload`.
    async fn answer(&mut self, request: u64, payload: Payload) -> Option<SessionEvent> {
        self.phone
            .send(&answer(self.seq, request, payload))
            .await
            .unwrap();
        self.seq += 1;
        self.pump().await
    }

    /// The phone reports the screen feature as `availability`.
    async fn report(&mut self, availability: FeatureAvailability) {
        let status = FeatureStatus {
            features: vec![FeatureState {
                feature: "screen".into(),
                availability: availability.into(),
                ..FeatureState::default()
            }],
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

    /// Reads the desktop's next request, which must be `expected`, and
    /// returns its sequence number.
    async fn request(&mut self, expected: &str) -> u64 {
        let frame = self.sent().await;
        let name = match &frame.payload {
            Some(Payload::ScreenStart(_)) => "ScreenStart",
            Some(Payload::ScreenStop(_)) => "ScreenStop",
            Some(Payload::ScreenInput(_)) => "ScreenInput",
            other => panic!("expected {expected}, got {other:?}"),
        };
        assert_eq!(name, expected);
        assert!(frame.expects_reply);
        frame.seq
    }

    /// Reads the desktop's next `ScreenInput` and returns its sequence
    /// number and input.
    async fn input(&mut self) -> (u64, screen_input::Input) {
        let frame = self.sent().await;
        let Some(Payload::ScreenInput(input)) = frame.payload else {
            panic!("expected a ScreenInput, got {frame:?}");
        };
        assert!(frame.expects_reply);
        (frame.seq, input.input.unwrap())
    }

    /// Reads the desktop's `ScreenKeyFrameRequest`.
    async fn key_frame_request(&mut self) {
        let frame = self.sent().await;
        assert!(
            matches!(frame.payload, Some(Payload::ScreenKeyFrameRequest(_)))
                && !frame.expects_reply,
            "{frame:?}"
        );
    }
}

fn ack() -> Payload {
    Payload::Ack(Ack {})
}

fn refusal(code: ErrorCode) -> Payload {
    Payload::ErrorReply(ErrorReply {
        code: code.into(),
        ..ErrorReply::default()
    })
}

fn jpeg(id: u32, input: bool) -> Payload {
    Payload::ScreenFormat(wire::ScreenFormat {
        id,
        codec: ScreenCodec::Jpeg.into(),
        width: 360,
        height: 780,
        codec_config: Vec::new(),
        input,
    })
}

fn h264(id: u32) -> Payload {
    Payload::ScreenFormat(wire::ScreenFormat {
        id,
        codec: ScreenCodec::H264.into(),
        width: 720,
        height: 1560,
        codec_config: vec![1, 0x64, 0, 0x1f, 0xff],
        input: false,
    })
}

fn frame(timestamp_us: u64, key_frame: bool) -> Payload {
    Payload::ScreenFrame(wire::ScreenFrame {
        timestamp_us,
        key_frame,
        data: vec![7; 64],
    })
}

fn ended(reason: ScreenEndReason) -> Payload {
    Payload::ScreenEnded(wire::ScreenEnded {
        reason: reason.into(),
    })
}

/// The timestamps of the frames waiting for `viewer`.
fn shown(viewer: &mut Receiver<VideoFrame>) -> Vec<u64> {
    let mut timestamps = Vec::new();
    while let Ok(frame) = viewer.try_recv() {
        timestamps.push(frame.timestamp_us);
    }
    timestamps
}

#[tokio::test(start_paused = true)]
async fn the_phone_asks_its_user_and_then_its_frames_reach_the_viewer() {
    let mut link = Link::ready().await;
    let mut viewer = link.start();
    let frame_sent = link.sent().await;
    let Some(Payload::ScreenStart(start)) = &frame_sent.payload else {
        panic!("expected a ScreenStart, got {frame_sent:?}");
    };
    assert_eq!(
        start.codecs,
        [ScreenCodec::H264 as i32, ScreenCodec::Jpeg as i32]
    );
    assert_eq!(start.max_size, 1920);
    assert_eq!(link.screen.state().phase, ScreenPhase::Starting);

    link.answer(frame_sent.seq, ack()).await;
    assert_eq!(link.screen.state().phase, ScreenPhase::Asking);

    link.phone_sends(jpeg(4, false)).await;
    let state = link.screen.state();
    assert_eq!(state.phase, ScreenPhase::Showing);
    let format = state.format.unwrap();
    assert_eq!(
        (format.id, format.codec, format.width, format.height),
        (4, Codec::Jpeg, 360, 780)
    );

    link.phone_sends(frame(10, true)).await;
    link.phone_sends(frame(20, true)).await;
    let first = viewer.try_recv().unwrap();
    // The viewer gets the format with its first frame, and only then.
    assert_eq!(first.format.as_ref().map(|format| format.id), Some(4));
    assert_eq!((first.format_id, first.timestamp_us), (4, 10));
    assert_eq!(first.data, [7; 64]);
    let second = viewer.try_recv().unwrap();
    assert_eq!((second.format, second.timestamp_us), (None, 20));
    link.sent_nothing();
}

#[tokio::test(start_paused = true)]
async fn frames_wait_for_the_key_frame_of_a_new_format() {
    let (mut link, mut viewer) = Link::showing(h264(1)).await;
    // A delta frame cannot be decoded without the key frame before it.
    link.phone_sends(frame(10, false)).await;
    link.key_frame_request().await;
    link.phone_sends(frame(20, true)).await;
    link.phone_sends(frame(30, false)).await;
    assert_eq!(shown(&mut viewer), [20, 30]);

    // The phone rotates: the frames that follow need the new format's key
    // frame, which comes next without asking.
    link.phone_sends(h264(2)).await;
    link.phone_sends(frame(40, true)).await;
    let rotated = viewer.try_recv().unwrap();
    assert_eq!(rotated.format.map(|format| format.id), Some(2));
    assert_eq!(rotated.format_id, 2);
    link.sent_nothing();
}

#[tokio::test(start_paused = true)]
async fn a_viewer_that_falls_behind_skips_to_a_key_frame_asked_for_once_a_second() {
    let (mut link, mut viewer) = Link::showing(h264(1)).await;
    let waiting = u64::try_from(FRAMES_WAITING).unwrap();
    link.phone_sends(frame(0, true)).await;
    for timestamp in 1..waiting {
        link.phone_sends(frame(timestamp, false)).await;
    }
    link.sent_nothing();
    // The queue is full: this frame and the ones up to a key frame go.
    link.phone_sends(frame(waiting, false)).await;
    link.key_frame_request().await;
    link.phone_sends(frame(waiting + 1, false)).await;
    // Within the second, no second request.
    link.sent_nothing();
    let due = link.screen.next_deadline().unwrap();
    assert_eq!(shown(&mut viewer).len(), FRAMES_WAITING);

    link.phone_sends(frame(waiting + 2, false)).await;
    assert!(viewer.try_recv().is_err());
    tokio::time::advance(KEY_FRAME_EVERY).await;
    assert!(due <= Instant::now());
    link.screen.tick(&mut link.session, Instant::now()).unwrap();
    link.key_frame_request().await;
    assert_eq!(link.screen.next_deadline(), None);

    link.phone_sends(frame(100, true)).await;
    link.phone_sends(frame(101, false)).await;
    assert_eq!(shown(&mut viewer), [100, 101]);
}

#[tokio::test(start_paused = true)]
async fn the_viewer_asks_for_a_key_frame_when_it_cannot_decode() {
    let (mut link, mut viewer) = Link::showing(h264(1)).await;
    link.phone_sends(frame(10, true)).await;
    link.screen.request_key_frame(&mut link.session).unwrap();
    link.key_frame_request().await;
    link.phone_sends(frame(20, false)).await;
    // Another request so soon waits for its second.
    link.screen.request_key_frame(&mut link.session).unwrap();
    link.sent_nothing();
    link.phone_sends(frame(30, true)).await;
    assert_eq!(link.screen.next_deadline(), None);
    assert_eq!(shown(&mut viewer), [10, 30]);
}

#[tokio::test(start_paused = true)]
async fn a_viewer_that_is_gone_stops_the_stream() {
    let (mut link, viewer) = Link::showing(jpeg(1, false)).await;
    drop(viewer);
    link.phone_sends(frame(10, true)).await;
    let stop = link.request("ScreenStop").await;
    assert_eq!(link.screen.state().phase, ScreenPhase::Stopping);
    link.phone_sends(frame(20, true)).await;
    link.answer(stop, ack()).await;
    let state = link.screen.state();
    assert_eq!((state.phase, state.ended), (ScreenPhase::Idle, None));
}

#[tokio::test(start_paused = true)]
async fn what_arrives_between_a_stop_and_its_answer_is_dropped() {
    let (mut link, mut viewer) = Link::showing(jpeg(1, false)).await;
    link.screen.stop(&mut link.session).unwrap();
    let stop = link.request("ScreenStop").await;
    assert_eq!(viewer.try_recv().unwrap_err(), TryRecvError::Disconnected);
    assert_eq!(link.screen.state().phase, ScreenPhase::Stopping);

    // Left over from the stopped stream.
    link.phone_sends(jpeg(1, true)).await;
    link.phone_sends(frame(10, true)).await;
    link.phone_sends(ended(ScreenEndReason::StoppedOnPhone))
        .await;
    let state = link.screen.state();
    assert_eq!((state.phase, state.ended), (ScreenPhase::Stopping, None));
    link.sent_nothing();

    link.answer(stop, ack()).await;
    assert_eq!(link.screen.state().phase, ScreenPhase::Idle);
    // Stopping again sends nothing.
    link.screen.stop(&mut link.session).unwrap();
    link.sent_nothing();
}

#[tokio::test(start_paused = true)]
async fn a_new_start_right_after_a_stop_shows_only_the_new_stream() {
    let (mut link, _old) = Link::showing(jpeg(1, false)).await;
    link.screen.stop(&mut link.session).unwrap();
    let stop = link.request("ScreenStop").await;
    let mut viewer = link.start();
    let start = link.request("ScreenStart").await;
    // The old stream's last frame, then the phone takes the stop and the
    // new start.
    link.phone_sends(frame(10, true)).await;
    link.answer(stop, ack()).await;
    link.answer(start, ack()).await;
    assert_eq!(link.screen.state().phase, ScreenPhase::Asking);
    link.phone_sends(jpeg(2, false)).await;
    link.phone_sends(frame(20, true)).await;
    assert_eq!(shown(&mut viewer), [20]);
}

#[tokio::test(start_paused = true)]
async fn the_phone_ending_the_stream_says_why() {
    let mut link = Link::ready().await;
    let mut viewer = link.start();
    let start = link.request("ScreenStart").await;
    link.answer(start, ack()).await;
    link.phone_sends(ended(ScreenEndReason::Declined)).await;
    let state = link.screen.state();
    assert_eq!(state.phase, ScreenPhase::Idle);
    assert_eq!(state.ended, Some(ScreenEnd::Declined));
    assert_eq!(viewer.try_recv().unwrap_err(), TryRecvError::Disconnected);
    link.sent_nothing();

    // The next start clears it.
    let _viewer = link.start();
    assert_eq!(link.screen.state().ended, None);
}

#[tokio::test(start_paused = true)]
async fn a_refused_start_says_why() {
    let cases = [
        (ErrorCode::Busy, ScreenProblem::Busy),
        (ErrorCode::NotAllowed, ScreenProblem::CannotAsk),
        (ErrorCode::InvalidArgument, ScreenProblem::NoCodec),
        (ErrorCode::Unavailable, ScreenProblem::Unavailable),
        (ErrorCode::Disabled, ScreenProblem::TurnedOff),
    ];
    let mut link = Link::ready().await;
    for (code, problem) in cases {
        let mut viewer = link.start();
        let start = link.request("ScreenStart").await;
        link.answer(start, refusal(code)).await;
        let state = link.screen.state();
        assert_eq!(state.phase, ScreenPhase::Idle, "{code:?}");
        assert_eq!(state.failure, Some(problem), "{code:?}");
        assert_eq!(viewer.try_recv().unwrap_err(), TryRecvError::Disconnected);
        // A phone that did not start needs no stop.
        link.sent_nothing();
    }
}

#[tokio::test(start_paused = true)]
async fn a_start_without_an_answer_fails_and_stops_the_phone() {
    let mut link = Link::ready().await;
    let _viewer = link.start();
    link.request("ScreenStart").await;
    // The paused clock runs to the request's deadline.
    assert_eq!(link.pump().await, None);
    let state = link.screen.state();
    assert_eq!(
        (state.phase, state.failure),
        (ScreenPhase::Stopping, Some(ScreenProblem::Failed))
    );
    // The phone may still ask its user.
    link.request("ScreenStop").await;
}

#[tokio::test(start_paused = true)]
async fn a_phone_that_cannot_share_its_screen_is_not_asked() {
    let mut link = Link::connect(&["ring"]).await;
    assert_eq!(
        link.screen.state().blocked,
        Some(ScreenProblem::Unsupported)
    );
    let mut viewer = link.start();
    assert_eq!(
        link.screen.state().failure,
        Some(ScreenProblem::Unsupported)
    );
    assert_eq!(viewer.try_recv().unwrap_err(), TryRecvError::Disconnected);
    link.sent_nothing();

    let mut link = Link::connect(&["screen"]).await;
    assert_eq!(link.screen.state().blocked, Some(ScreenProblem::Waiting));
    link.report(Disabled).await;
    let _viewer = link.start();
    assert_eq!(link.screen.state().failure, Some(ScreenProblem::TurnedOff));
    link.sent_nothing();
}

#[tokio::test(start_paused = true)]
async fn turning_the_feature_off_on_the_phone_ends_the_stream() {
    let (mut link, mut viewer) = Link::showing(jpeg(1, false)).await;
    link.report(Disabled).await;
    let state = link.screen.state();
    assert_eq!(state.phase, ScreenPhase::Idle);
    assert_eq!(state.ended, Some(ScreenEnd::TurnedOff));
    assert_eq!(state.blocked, Some(ScreenProblem::TurnedOff));
    assert_eq!(viewer.try_recv().unwrap_err(), TryRecvError::Disconnected);
    // The phone stops by itself.
    link.sent_nothing();
}

#[tokio::test(start_paused = true)]
async fn the_stream_ends_with_its_session() {
    let (mut link, mut viewer) = Link::showing(jpeg(1, false)).await;
    link.screen.session_ended();
    let state = link.screen.state();
    assert_eq!(state.phase, ScreenPhase::Idle);
    assert_eq!(state.ended, Some(ScreenEnd::Disconnected));
    assert_eq!(state.blocked, Some(ScreenProblem::NotConnected));
    assert_eq!(viewer.try_recv().unwrap_err(), TryRecvError::Disconnected);
    link.screen.not_connected();
    assert_eq!(
        link.screen.state().failure,
        Some(ScreenProblem::NotConnected)
    );
}

#[tokio::test(start_paused = true)]
async fn video_nobody_asked_for_is_stopped() {
    let mut link = Link::ready().await;
    link.phone_sends(jpeg(1, false)).await;
    let stop = link.request("ScreenStop").await;
    link.phone_sends(jpeg(1, false)).await;
    link.phone_sends(frame(10, true)).await;
    link.sent_nothing();
    link.answer(stop, ack()).await;
    assert_eq!(link.screen.state().phase, ScreenPhase::Idle);
}

#[tokio::test(start_paused = true)]
async fn a_format_the_viewer_cannot_use_ends_the_stream() {
    let (mut link, mut viewer) = Link::showing(jpeg(1, false)).await;
    let broken = Payload::ScreenFormat(wire::ScreenFormat {
        id: 2,
        codec: ScreenCodec::H264.into(),
        width: 720,
        height: 1560,
        codec_config: Vec::new(),
        input: false,
    });
    link.phone_sends(broken).await;
    link.request("ScreenStop").await;
    assert_eq!(link.screen.state().ended, Some(ScreenEnd::Interrupted));
    assert_eq!(viewer.try_recv().unwrap_err(), TryRecvError::Disconnected);
}

#[tokio::test(start_paused = true)]
async fn a_second_start_hands_the_stream_to_the_new_viewer() {
    let (mut link, mut old) = Link::showing(h264(1)).await;
    link.phone_sends(frame(10, true)).await;
    assert_eq!(shown(&mut old), [10]);

    let mut viewer = link.start();
    let start = link.request("ScreenStart").await;
    assert_eq!(old.try_recv().unwrap_err(), TryRecvError::Disconnected);
    // The new viewer cannot start from a delta frame.
    link.phone_sends(frame(20, false)).await;
    link.key_frame_request().await;
    link.answer(start, ack()).await;
    assert_eq!(link.screen.state().phase, ScreenPhase::Showing);
    link.phone_sends(h264(2)).await;
    link.phone_sends(frame(30, true)).await;
    let first = viewer.try_recv().unwrap();
    assert_eq!(first.format.map(|format| format.id), Some(2));
    assert_eq!(first.timestamp_us, 30);
}

#[tokio::test(start_paused = true)]
async fn input_goes_to_the_phone_only_while_it_takes_it() {
    let (mut link, mut viewer) = Link::showing(jpeg(1, false)).await;
    link.screen
        .input(&mut link.session, Input::Key(Key::Back))
        .unwrap();
    assert_eq!(
        link.screen.state().input_problem,
        Some(InputProblem::ControlOff)
    );
    link.sent_nothing();

    // Control turned on: the format keeps its id, and the video goes on.
    link.phone_sends(jpeg(1, true)).await;
    assert!(link.screen.state().format.unwrap().input);
    link.phone_sends(frame(10, true)).await;
    assert_eq!(viewer.try_recv().unwrap().format.map(|f| f.id), Some(1));
    link.phone_sends(jpeg(1, true)).await;
    link.phone_sends(frame(20, true)).await;
    assert_eq!(viewer.try_recv().unwrap().format, None);

    let tap = Input::Gesture {
        format_id: 1,
        points: vec![Point {
            x: 100,
            y: 5000,
            time_ms: 0,
        }],
    };
    link.screen.input(&mut link.session, tap).unwrap();
    let (seq, sent) = link.input().await;
    let screen_input::Input::Gesture(gesture) = sent else {
        panic!("expected a gesture, got {sent:?}");
    };
    assert_eq!(gesture.format_id, 1);
    assert_eq!(
        gesture
            .points
            .iter()
            .map(|p| (p.x, p.y, p.time_ms))
            .collect::<Vec<_>>(),
        [(100, 779, 0)]
    );
    link.answer(seq, refusal(ErrorCode::Expired)).await;
    assert_eq!(
        link.screen.state().input_problem,
        Some(InputProblem::ScreenChanged)
    );

    // A gesture on frames of another format is not sent.
    let stale = Input::Gesture {
        format_id: 9,
        points: vec![Point {
            x: 1,
            y: 1,
            time_ms: 0,
        }],
    };
    link.screen.input(&mut link.session, stale).unwrap();
    link.sent_nothing();

    // Inputs go in order, without waiting for answers.
    let long = "x".repeat(5000);
    link.screen
        .input(&mut link.session, Input::Text(long))
        .unwrap();
    link.screen
        .input(&mut link.session, Input::Key(Key::Enter))
        .unwrap();
    let (first, part) = link.input().await;
    assert_eq!(part, screen_input::Input::Text("x".repeat(4096)));
    let (second, part) = link.input().await;
    assert_eq!(part, screen_input::Input::Text("x".repeat(904)));
    let (third, key) = link.input().await;
    assert_eq!(key, screen_input::Input::Key(wire::ScreenKey::Enter.into()));
    link.answer(first, refusal(ErrorCode::NotFound)).await;
    assert_eq!(
        link.screen.state().input_problem,
        Some(InputProblem::NoTextField)
    );
    link.answer(second, ack()).await;
    link.answer(third, ack()).await;
    assert_eq!(link.screen.state().input_problem, None);

    // Control turned off on the phone.
    link.phone_sends(jpeg(1, false)).await;
    link.screen
        .input(&mut link.session, Input::Key(Key::Home))
        .unwrap();
    link.sent_nothing();
    assert_eq!(
        link.screen.state().input_problem,
        Some(InputProblem::ControlOff)
    );
}

#[tokio::test(start_paused = true)]
async fn an_unanswered_input_fails() {
    let (mut link, _viewer) = Link::showing(jpeg(1, true)).await;
    link.screen
        .input(&mut link.session, Input::Key(Key::Recents))
        .unwrap();
    let (_, key) = link.input().await;
    assert_eq!(
        key,
        screen_input::Input::Key(wire::ScreenKey::Recents.into())
    );
    assert_eq!(link.pump().await, None);
    assert_eq!(
        link.screen.state().input_problem,
        Some(InputProblem::Failed)
    );
}
