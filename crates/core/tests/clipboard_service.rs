//! The clipboard service on desktop sessions, against phones each test
//! scripts frame by frame, and a desktop clipboard in memory.

mod session_common;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use link2linux_core::clipboard::{
    Clip, ClipboardEvent, ClipboardHub, ClipboardService, ClipboardSettings, MAX_TEXT_BYTES,
    NotShared, Origin, PhoneClipboard, PushFailure, RECENT_COPY,
};
use link2linux_core::session::SessionEvent;
use link2linux_core::{Session, SessionConfig};
use link2linux_platform::{Clipboard, ClipboardAccess, ClipboardText, PlatformError};
use link2linux_protocol::v1::{
    Ack, ClipboardPush, ErrorCode, ErrorReply, FeatureAvailability, FeatureState, FeatureStatus,
};
use link2linux_protocol::{Envelope, Hello, Payload};
use session_common::{RawPeer, answer, connected_to, envelope, fast, phone_hello, request};
use tokio::io::DuplexStream;
use tokio::sync::broadcast;
use tokio::time::Instant;

/// The desktop clipboard, in memory.
#[derive(Clone)]
struct FakeClipboard {
    access: ClipboardAccess,
    content: Arc<Mutex<Option<ClipboardText>>>,
    broken: Arc<AtomicBool>,
}

impl FakeClipboard {
    fn new(access: ClipboardAccess) -> Self {
        Self {
            access,
            content: Arc::default(),
            broken: Arc::default(),
        }
    }

    fn text(&self) -> Option<ClipboardText> {
        self.content.lock().unwrap().clone()
    }

    fn break_it(&self) {
        self.broken.store(true, Ordering::Relaxed);
    }
}

impl Clipboard for FakeClipboard {
    fn access(&self) -> ClipboardAccess {
        self.access
    }

    async fn read(&self) -> Result<Option<ClipboardText>, PlatformError> {
        Ok(self.text())
    }

    async fn write(&self, text: &ClipboardText) -> Result<(), PlatformError> {
        if self.broken.load(Ordering::Relaxed) {
            return Err(PlatformError::Unsupported("clipboard"));
        }
        *self.content.lock().unwrap() = Some(text.clone());
        Ok(())
    }

    async fn changed(&self) -> Result<(), PlatformError> {
        std::future::pending().await
    }
}

fn text(text: &str) -> ClipboardText {
    ClipboardText {
        text: text.to_owned(),
        sensitive: false,
    }
}

/// A `FeatureStatus` that reports the clipboard as `availability`.
fn clipboard_status(availability: FeatureAvailability) -> Payload {
    Payload::FeatureStatus(FeatureStatus {
        features: vec![FeatureState {
            feature: "clipboard".into(),
            availability: availability.into(),
            ..FeatureState::default()
        }],
    })
}

fn error_code(frame: &Envelope) -> ErrorCode {
    match &frame.payload {
        Some(Payload::ErrorReply(error)) => error.code(),
        other => panic!("expected an ErrorReply, got {other:?}"),
    }
}

/// One phone's session, the phone's end of it, and the desktop's service
/// for that phone.
struct Link {
    session: Session<DuplexStream>,
    phone: RawPeer,
    service: ClipboardService<FakeClipboard>,
    /// The phone's next sequence number.
    seq: u64,
}

impl Link {
    /// A session with a phone that sends `hello` and has not reported its
    /// features yet.
    async fn connect(
        hello: Hello,
        config: SessionConfig,
        hub: &ClipboardHub,
        clipboard: &FakeClipboard,
    ) -> Self {
        let (mut session, phone) = connected_to(hello.clone(), config).await;
        let mut service = ClipboardService::new(hello.device_id, hub.clone(), clipboard.clone());
        service.session_started(&mut session).await.unwrap();
        Self {
            session,
            phone,
            service,
            seq: 2,
        }
    }

    /// A session with phone `id`, which reports the clipboard ready.
    async fn ready(id: &str, hub: &ClipboardHub, clipboard: &FakeClipboard) -> Self {
        let hello = Hello {
            device_id: id.into(),
            ..phone_hello()
        };
        let mut link = Self::connect(hello, fast(), hub, clipboard).await;
        link.status(FeatureAvailability::Ready).await;
        link
    }

    /// The phone reports the clipboard as `availability`.
    async fn status(&mut self, availability: FeatureAvailability) {
        self.send(envelope(self.seq, clipboard_status(availability)))
            .await;
        let rest = self.step().await;
        assert!(
            matches!(rest, Some(SessionEvent::FeatureStatus(_))),
            "{rest:?}"
        );
    }

    async fn send(&mut self, frame: Envelope) {
        self.phone.send(&frame).await.unwrap();
        self.seq += 1;
    }

    /// Hands the desktop's next event to the service and returns what it
    /// left.
    async fn step(&mut self) -> Option<SessionEvent> {
        let event = self.session.next_event().await.unwrap().expect("an event");
        self.service
            .handle_event(&mut self.session, event)
            .await
            .unwrap()
    }

    /// The next frame from the desktop, skipping its pings.
    async fn frame(&mut self) -> Envelope {
        loop {
            let frame = self.phone.next().await.unwrap().unwrap();
            if !matches!(frame.payload, Some(Payload::Ping(_))) {
                return frame;
            }
        }
    }

    /// The phone pushes `text` and reads the desktop's answer.
    async fn push(&mut self, text: &str, sensitive: bool) -> Envelope {
        let frame = self.push_frame(text, sensitive);
        let Self {
            session,
            phone,
            service,
            ..
        } = self;
        // Long text fills the pipe: the phone writes while the desktop
        // reads.
        let (answer, rest) = tokio::join!(send_and_read(phone, &frame), async {
            let event = session.next_event().await.unwrap().unwrap();
            service.handle_event(session, event).await.unwrap()
        });
        assert!(rest.is_none(), "{rest:?}");
        answer
    }

    /// The phone pushes `text` while the session refuses the clipboard by
    /// itself, and reads the answer.
    async fn refused_push(&mut self, text: &str) -> Envelope {
        let frame = self.push_frame(text, false);
        let Self { session, phone, .. } = self;
        let (answer, nothing) = tokio::join!(send_and_read(phone, &frame), async {
            tokio::time::timeout(Duration::from_millis(200), session.next_event()).await
        });
        assert!(nothing.is_err(), "{nothing:?}");
        answer
    }

    fn push_frame(&mut self, text: &str, sensitive: bool) -> Envelope {
        let push = Payload::ClipboardPush(ClipboardPush {
            text: text.to_owned(),
            sensitive,
        });
        let frame = request(self.seq, push);
        self.seq += 1;
        frame
    }

    /// The push the desktop sent next.
    async fn pushed(&mut self) -> (u64, ClipboardPush) {
        let frame = self.frame().await;
        assert!(frame.expects_reply);
        match frame.payload {
            Some(Payload::ClipboardPush(push)) => (frame.seq, push),
            other => panic!("expected a ClipboardPush, got {other:?}"),
        }
    }

    /// The phone answers the push `request`.
    async fn answer(&mut self, request: u64, payload: Payload) {
        self.send(answer(self.seq, request, payload)).await;
        assert!(self.step().await.is_none());
    }

    async fn ack(&mut self, request: u64) {
        self.answer(request, Payload::Ack(Ack {})).await;
    }

    async fn share(&mut self, clip: Arc<Clip>, origin: Origin) {
        self.service
            .share(&mut self.session, clip, origin)
            .await
            .unwrap();
    }

    /// Checks that the desktop sent no push the phone has not read yet.
    async fn quiet(&mut self) {
        self.session.ping().await.unwrap();
        let next = self.phone.next().await.unwrap().unwrap().payload;
        assert!(
            matches!(next, Some(Payload::Ping(_))),
            "expected nothing more, got {next:?}"
        );
    }
}

/// Sends `frame` from the phone and returns the desktop's answer to it.
async fn send_and_read(phone: &mut RawPeer, frame: &Envelope) -> Envelope {
    phone.send(frame).await.unwrap();
    loop {
        let answer = phone.next().await.unwrap().unwrap();
        if !matches!(answer.payload, Some(Payload::Ping(_))) {
            assert_eq!(answer.reply_to, frame.seq);
            return answer;
        }
    }
}

fn drain(events: &mut broadcast::Receiver<ClipboardEvent>) -> Vec<ClipboardEvent> {
    std::iter::from_fn(|| events.try_recv().ok()).collect()
}

fn clip(text: &str) -> Arc<Clip> {
    Arc::new(Clip::new(self::text(text)))
}

#[tokio::test]
async fn a_phones_push_goes_on_the_clipboard_with_its_mark_and_is_acked() {
    let hub = ClipboardHub::new(ClipboardSettings::default());
    let mut events = hub.subscribe();
    let clipboard = FakeClipboard::new(ClipboardAccess::Background);
    let mut link = Link::ready("phone-1", &hub, &clipboard).await;
    assert_eq!(
        drain(&mut events),
        [ClipboardEvent::Phone {
            device_id: "phone-1".into(),
            state: PhoneClipboard::Ready
        }]
    );

    let answer = link.push("hunter2", true).await;
    assert!(matches!(answer.payload, Some(Payload::Ack(_))));
    let written = clipboard.text().unwrap();
    assert_eq!(
        (written.text.as_str(), written.sensitive),
        ("hunter2", true)
    );
    assert_eq!(
        drain(&mut events),
        [ClipboardEvent::Received {
            device_id: "phone-1".into(),
            bytes: 7,
            sensitive: true,
            held: false
        }]
    );

    // The longest text there is goes too.
    let longest = "a".repeat(MAX_TEXT_BYTES);
    let answer = link.push(&longest, false).await;
    assert!(matches!(answer.payload, Some(Payload::Ack(_))));
    assert_eq!(clipboard.text().unwrap().text.len(), MAX_TEXT_BYTES);
}

#[tokio::test]
async fn empty_and_too_long_pushes_are_refused() {
    let hub = ClipboardHub::new(ClipboardSettings::default());
    let clipboard = FakeClipboard::new(ClipboardAccess::Background);
    let mut link = Link::ready("phone-1", &hub, &clipboard).await;

    let answer = link.push("", false).await;
    assert_eq!(error_code(&answer), ErrorCode::InvalidArgument);
    let longer = "a".repeat(MAX_TEXT_BYTES + 1);
    let answer = link.push(&longer, false).await;
    assert_eq!(error_code(&answer), ErrorCode::InvalidArgument);
    assert!(clipboard.text().is_none());
}

#[tokio::test]
async fn a_clipboard_that_cannot_be_set_answers_unavailable() {
    let hub = ClipboardHub::new(ClipboardSettings::default());
    let mut events = hub.subscribe();
    let clipboard = FakeClipboard::new(ClipboardAccess::Background);
    let mut link = Link::ready("phone-1", &hub, &clipboard).await;
    clipboard.break_it();
    drain(&mut events);

    let answer = link.push("x", false).await;
    assert_eq!(error_code(&answer), ErrorCode::Unavailable);
    assert!(drain(&mut events).is_empty());
    // The text is not taken for the phone's: copying it here shares it.
    assert!(hub.copied(text("x"), Instant::now()).is_some());
}

#[tokio::test]
async fn pushes_are_refused_while_sharing_is_off_and_the_phone_is_told() {
    let hub = ClipboardHub::new(ClipboardSettings::default());
    let clipboard = FakeClipboard::new(ClipboardAccess::Background);
    let mut link = Link::ready("phone-1", &hub, &clipboard).await;

    // Turned off, but the phone has not been told yet.
    hub.set_settings(ClipboardSettings {
        sharing: false,
        ..ClipboardSettings::default()
    });
    let answer = link.push("x", false).await;
    assert_eq!(error_code(&answer), ErrorCode::Disabled);

    // Then it is.
    link.service
        .sharing_changed(&mut link.session)
        .await
        .unwrap();
    let frame = link.frame().await;
    let Some(Payload::FeatureStatus(status)) = frame.payload else {
        panic!("expected a FeatureStatus, got {frame:?}");
    };
    let clipboard_state = status
        .features
        .iter()
        .find(|state| state.feature == "clipboard")
        .unwrap();
    assert_eq!(
        clipboard_state.availability(),
        FeatureAvailability::Disabled
    );
    let answer = link.refused_push("y").await;
    assert_eq!(error_code(&answer), ErrorCode::Disabled);
    assert!(clipboard.text().is_none());
    // Nothing goes out either.
    link.share(clip("z"), Origin::Explicit).await;
    link.quiet().await;

    hub.set_settings(ClipboardSettings::default());
    link.service
        .sharing_changed(&mut link.session)
        .await
        .unwrap();
    let frame = link.frame().await;
    assert!(matches!(frame.payload, Some(Payload::FeatureStatus(_))));
    let answer = link.push("w", false).await;
    assert!(matches!(answer.payload, Some(Payload::Ack(_))));
}

#[tokio::test]
async fn a_focus_only_desktop_holds_the_newest_push_until_the_window_has_focus() {
    let hub = ClipboardHub::new(ClipboardSettings::default());
    let mut events = hub.subscribe();
    let clipboard = FakeClipboard::new(ClipboardAccess::FocusOnly);
    let mut first = Link::ready("phone-1", &hub, &clipboard).await;
    let mut second = Link::ready("phone-2", &hub, &clipboard).await;
    drain(&mut events);

    // In the background: held, and acked.
    let answer = first.push("older", false).await;
    assert!(matches!(answer.payload, Some(Payload::Ack(_))));
    let answer = second.push("newer", true).await;
    assert!(matches!(answer.payload, Some(Payload::Ack(_))));
    assert!(clipboard.text().is_none());
    let held = hub.held().unwrap();
    assert_eq!(held.clip.text().text, "newer");
    assert_eq!(held.device_id, "phone-2");
    assert_eq!(
        drain(&mut events),
        [
            ClipboardEvent::Received {
                device_id: "phone-1".into(),
                bytes: 5,
                sensitive: false,
                held: true
            },
            ClipboardEvent::Received {
                device_id: "phone-2".into(),
                bytes: 5,
                sensitive: true,
                held: true
            }
        ]
    );

    // The window gains focus: the newest goes on the clipboard, and is not
    // shared on.
    hub.set_focused(true);
    assert!(hub.focused(&clipboard, Instant::now()).await.is_none());
    let written = clipboard.text().unwrap();
    assert_eq!((written.text.as_str(), written.sensitive), ("newer", true));
    assert!(hub.held().is_none());

    // While the window has focus, a push goes on the clipboard at once.
    let answer = first.push("now", false).await;
    assert!(matches!(answer.payload, Some(Payload::Ack(_))));
    assert_eq!(clipboard.text().unwrap().text, "now");
    assert!(hub.held().is_none());

    // A clipboard that fails even then holds it.
    clipboard.break_it();
    let answer = first.push("later", false).await;
    assert!(matches!(answer.payload, Some(Payload::Ack(_))));
    assert_eq!(hub.held().unwrap().clip.text().text, "later");

    // Text that goes on the clipboard at once replaces older text that
    // waits, which focus must not put back.
    clipboard.broken.store(false, Ordering::Relaxed);
    let answer = second.push("newest", false).await;
    assert!(matches!(answer.payload, Some(Payload::Ack(_))));
    assert!(hub.held().is_none());
    assert!(hub.focused(&clipboard, Instant::now()).await.is_none());
    assert_eq!(clipboard.text().unwrap().text, "newest");
}

#[tokio::test]
async fn a_phones_text_is_neither_pushed_back_nor_relayed_by_itself() {
    let hub = ClipboardHub::new(ClipboardSettings::default());
    let clipboard = FakeClipboard::new(ClipboardAccess::Background);
    let mut first = Link::ready("phone-1", &hub, &clipboard).await;
    let mut second = Link::ready("phone-2", &hub, &clipboard).await;

    first.push("from one", false).await;
    // The clipboard reports the text the desktop just put there.
    assert!(
        hub.copied(clipboard.text().unwrap(), Instant::now())
            .is_none()
    );

    // Shared by itself anyway, it goes back to neither.
    first.share(clip("from one"), Origin::Automatic).await;
    first.quiet().await;

    // The user asks to send it: it goes to both.
    let sent = hub
        .send_clipboard(&clipboard, Instant::now())
        .await
        .unwrap();
    first.share(sent.clone(), Origin::Explicit).await;
    second.share(sent, Origin::Explicit).await;
    assert_eq!(first.pushed().await.1.text, "from one");
    assert_eq!(second.pushed().await.1.text, "from one");
}

#[tokio::test]
async fn a_copy_the_phone_already_got_is_not_pushed_again_by_itself() {
    let hub = ClipboardHub::new(ClipboardSettings::default());
    let mut events = hub.subscribe();
    let clipboard = FakeClipboard::new(ClipboardAccess::Background);
    let mut link = Link::ready("phone-1", &hub, &clipboard).await;
    drain(&mut events);

    let copy = hub.copied(text("a"), Instant::now()).unwrap();
    link.share(copy.clone(), Origin::Automatic).await;
    let (seq, push) = link.pushed().await;
    assert_eq!((push.text.as_str(), push.sensitive), ("a", false));
    link.ack(seq).await;
    assert_eq!(
        drain(&mut events),
        [ClipboardEvent::Sent {
            device_id: "phone-1".into(),
            bytes: 1,
            sensitive: false
        }]
    );
    link.share(copy.clone(), Origin::Automatic).await;
    link.quiet().await;
    // Asked for, it goes again.
    link.share(copy, Origin::Explicit).await;
    assert_eq!(link.pushed().await.1.text, "a");
}

#[tokio::test]
async fn one_push_waits_at_a_time_and_only_the_newest_copy_follows() {
    let hub = ClipboardHub::new(ClipboardSettings::default());
    let clipboard = FakeClipboard::new(ClipboardAccess::Background);
    let mut link = Link::ready("phone-1", &hub, &clipboard).await;
    let now = Instant::now();

    let a = hub.copied(text("a"), now).unwrap();
    link.share(a, Origin::Automatic).await;
    for copy in ["b", "c", "d"] {
        let copy = hub.copied(text(copy), now).unwrap();
        link.share(copy, Origin::Automatic).await;
    }
    let (seq, push) = link.pushed().await;
    assert_eq!(push.text, "a");
    link.quiet().await;

    link.ack(seq).await;
    let (seq, push) = link.pushed().await;
    assert_eq!(push.text, "d");
    link.ack(seq).await;
    link.quiet().await;

    // A copy back to the text that waits is not pushed twice.
    let e = hub.copied(text("e"), now).unwrap();
    link.share(e, Origin::Automatic).await;
    let (seq, _) = link.pushed().await;
    let d = hub.copied(text("d"), now).unwrap();
    link.share(d, Origin::Automatic).await;
    let e = hub.copied(text("e"), now).unwrap();
    link.share(e, Origin::Automatic).await;
    link.ack(seq).await;
    link.quiet().await;
}

#[tokio::test]
async fn the_phones_answers_are_reported() {
    let hub = ClipboardHub::new(ClipboardSettings::default());
    let mut events = hub.subscribe();
    let clipboard = FakeClipboard::new(ClipboardAccess::Background);
    let mut link = Link::ready("phone-1", &hub, &clipboard).await;
    drain(&mut events);

    for (code, reason) in [
        (ErrorCode::Disabled, PushFailure::TurnedOff),
        (ErrorCode::Unavailable, PushFailure::Unavailable),
        (ErrorCode::InvalidArgument, PushFailure::Failed),
    ] {
        link.share(clip("x"), Origin::Explicit).await;
        let (seq, _) = link.pushed().await;
        let refusal = ErrorReply {
            code: code.into(),
            ..ErrorReply::default()
        };
        link.answer(seq, Payload::ErrorReply(refusal)).await;
        assert_eq!(
            drain(&mut events),
            [ClipboardEvent::Failed {
                device_id: "phone-1".into(),
                reason
            }]
        );
    }

    // A push in flight when the session ends failed too.
    link.share(clip("y"), Origin::Explicit).await;
    link.service.session_ended();
    assert_eq!(
        drain(&mut events),
        [
            ClipboardEvent::Failed {
                device_id: "phone-1".into(),
                reason: PushFailure::Failed
            },
            ClipboardEvent::Phone {
                device_id: "phone-1".into(),
                state: PhoneClipboard::Disconnected
            }
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn a_push_without_an_answer_in_time_failed_and_the_next_one_follows() {
    let hub = ClipboardHub::new(ClipboardSettings::default());
    let mut events = hub.subscribe();
    let clipboard = FakeClipboard::new(ClipboardAccess::Background);
    let config = SessionConfig {
        request_timeout: Duration::from_secs(2),
        ..fast()
    };
    let mut link = Link::connect(phone_hello(), config, &hub, &clipboard).await;
    link.status(FeatureAvailability::Ready).await;
    drain(&mut events);

    link.share(clip("a"), Origin::Explicit).await;
    link.share(clip("b"), Origin::Explicit).await;
    // The phone stays silent until the push times out.
    assert!(link.step().await.is_none());
    assert_eq!(
        drain(&mut events),
        [ClipboardEvent::Failed {
            device_id: "phone-1".into(),
            reason: PushFailure::Failed
        }]
    );
    assert_eq!(link.pushed().await.1.text, "a");
    assert_eq!(link.pushed().await.1.text, "b");
}

#[tokio::test]
async fn nothing_goes_out_until_the_phone_reports_the_clipboard_ready() {
    let hub = ClipboardHub::new(ClipboardSettings::default());
    let mut events = hub.subscribe();
    let clipboard = FakeClipboard::new(ClipboardAccess::Background);
    let mut link = Link::connect(phone_hello(), fast(), &hub, &clipboard).await;

    // Before the phone's FeatureStatus.
    let copy = hub.copied(text("a"), Instant::now()).unwrap();
    link.share(copy.clone(), Origin::Automatic).await;
    link.quiet().await;

    // Turned off on the phone.
    link.status(FeatureAvailability::Disabled).await;
    link.share(copy, Origin::Explicit).await;
    link.quiet().await;

    // Turned on: the copy the user just made follows.
    link.status(FeatureAvailability::Ready).await;
    let (seq, push) = link.pushed().await;
    assert_eq!(push.text, "a");
    link.ack(seq).await;
    // Reported again as ready: it has the copy.
    link.status(FeatureAvailability::Disabled).await;
    link.status(FeatureAvailability::Ready).await;
    link.quiet().await;

    let states: Vec<_> = drain(&mut events)
        .into_iter()
        .filter_map(|event| match event {
            ClipboardEvent::Phone { state, .. } => Some(state),
            _ => None,
        })
        .collect();
    assert_eq!(
        states,
        [
            PhoneClipboard::TurnedOff,
            PhoneClipboard::Ready,
            PhoneClipboard::TurnedOff,
            PhoneClipboard::Ready
        ]
    );
}

#[tokio::test]
async fn a_recent_copy_goes_to_a_phone_whose_session_starts_and_an_old_one_does_not() {
    let hub = ClipboardHub::new(ClipboardSettings::default());
    let clipboard = FakeClipboard::new(ClipboardAccess::Background);

    let long_ago = Instant::now().checked_sub(RECENT_COPY).unwrap();
    hub.copied(text("old"), long_ago).unwrap();
    let mut link = Link::ready("phone-1", &hub, &clipboard).await;
    link.quiet().await;

    hub.copied(text("new"), Instant::now()).unwrap();
    let mut link = Link::ready("phone-1", &hub, &clipboard).await;
    let (seq, push) = link.pushed().await;
    assert_eq!(push.text, "new");
    link.ack(seq).await;

    // The same phone's service in a new session: it has the copy.
    link.service.session_ended();
    let (mut session, phone) = connected_to(phone_hello(), fast()).await;
    link.service.session_started(&mut session).await.unwrap();
    let mut link = Link {
        session,
        phone,
        service: link.service,
        seq: 2,
    };
    link.status(FeatureAvailability::Ready).await;
    link.quiet().await;
}

#[tokio::test]
async fn a_phone_that_does_not_share_a_clipboard_gets_nothing() {
    let hub = ClipboardHub::new(ClipboardSettings::default());
    let mut events = hub.subscribe();
    let clipboard = FakeClipboard::new(ClipboardAccess::Background);
    let mut hello = phone_hello();
    hello.features.retain(|feature| feature != "clipboard");
    let mut link = Link::connect(hello, fast(), &hub, &clipboard).await;
    assert_eq!(
        drain(&mut events),
        [ClipboardEvent::Phone {
            device_id: "phone-1".into(),
            state: PhoneClipboard::Unsupported
        }]
    );
    link.share(clip("a"), Origin::Explicit).await;
    link.quiet().await;

    // Its push is answered by the session itself.
    let frame = link.refused_push("x").await;
    assert_eq!(error_code(&frame), ErrorCode::Unsupported);
    assert!(clipboard.text().is_none());
}

#[tokio::test]
async fn secrets_and_long_copies_stay_here_and_the_user_is_told() {
    let hub = ClipboardHub::new(ClipboardSettings::default());
    let mut events = hub.subscribe();
    let clipboard = FakeClipboard::new(ClipboardAccess::Background);
    clipboard
        .write(&ClipboardText {
            text: "hunter2".into(),
            sensitive: true,
        })
        .await
        .unwrap();
    assert_eq!(
        hub.send_clipboard(&clipboard, Instant::now())
            .await
            .unwrap_err(),
        NotShared::Secret
    );
    assert_eq!(
        drain(&mut events),
        [ClipboardEvent::NotShared(NotShared::Secret)]
    );
}
