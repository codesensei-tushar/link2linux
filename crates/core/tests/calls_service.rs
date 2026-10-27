//! The calls service on a desktop session, against a phone each test
//! scripts frame by frame.

mod session_common;

use futures_util::{SinkExt, StreamExt};
use link2linux_core::calls::{CallsService, Handled, PhoneAnswer};
use link2linux_core::session::{Availability, SessionEvent, SessionStart};
use link2linux_core::{DeviceInfo, DeviceKind, Feature, Session};
use link2linux_protocol::v1::{
    Ack, BluetoothPairingRequest, ErrorCode, ErrorReply, FeatureAvailability, FeatureState,
    FeatureStatus,
};
use link2linux_protocol::{Envelope, Hello, Payload};
use session_common::{
    RawPeer, answer, envelope, fast, phone_hello, pipe, read_announcement, request,
};
use tokio::io::DuplexStream;

/// A desktop that offers calls, and a phone that does too.
struct Link {
    session: Session<DuplexStream>,
    phone: RawPeer,
    service: CallsService,
    /// The phone's next sequence number.
    seq: u64,
}

impl Link {
    /// A session in which the desktop starts with calls `availability`,
    /// and the phone has not reported its features yet.
    async fn connect(availability: Availability) -> Self {
        let desktop = DeviceInfo::new("desktop-1", "Desk", DeviceKind::Desktop, [Feature::Calls]);
        let mut start = SessionStart::new(desktop);
        start.availability.insert(Feature::Calls, availability.clone());
        let hello = Hello {
            features: vec!["calls".into()],
            ..phone_hello()
        };
        let (io, mut phone) = pipe();
        let script = async move {
            phone.next().await.unwrap().unwrap();
            phone
                .send(&envelope(1, Payload::Hello(hello)))
                .await
                .unwrap();
            let (status, _) = read_announcement(&mut phone).await;
            (phone, status)
        };
        let (session, (phone, _)) = tokio::join!(Session::handshake(io, start, fast()), script);
        let mut session = session.unwrap();
        let mut service = CallsService::new();
        service
            .session_started(&mut session, availability)
            .await
            .unwrap();
        Self {
            session,
            phone,
            service,
            seq: 2,
        }
    }

    async fn send(&mut self, frame: Envelope) {
        self.phone.send(&frame).await.unwrap();
        self.seq += 1;
    }

    /// The phone reports calls as `availability`.
    async fn status(&mut self, availability: FeatureAvailability) {
        let status = Payload::FeatureStatus(FeatureStatus {
            features: vec![FeatureState {
                feature: "calls".into(),
                availability: availability.into(),
                ..FeatureState::default()
            }],
        });
        self.send(envelope(self.seq, status)).await;
        assert!(matches!(
            self.step().await,
            Handled::NotMine(SessionEvent::FeatureStatus(_))
        ));
    }

    /// Hands the desktop's next event to the service.
    async fn step(&mut self) -> Handled {
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

    async fn ask(&mut self) -> bool {
        self.service
            .ask_to_pair(&mut self.session, request_to_pair())
            .await
            .unwrap()
    }
}

fn request_to_pair() -> BluetoothPairingRequest {
    BluetoothPairingRequest {
        name: "workstation".into(),
        address: "A0:B1:C2:D3:E4:F5".into(),
    }
}

fn calls_state(frame: &Envelope) -> FeatureAvailability {
    match &frame.payload {
        Some(Payload::FeatureStatus(status)) => status
            .features
            .iter()
            .find(|f| f.feature == "calls")
            .map(FeatureState::availability)
            .unwrap(),
        other => panic!("expected a FeatureStatus, got {other:?}"),
    }
}

#[tokio::test]
async fn reports_the_desktops_availability_as_it_changes() {
    let mut link = Link::connect(Availability::Unavailable).await;
    link.service
        .availability_changed(&mut link.session, Availability::Ready)
        .await
        .unwrap();
    assert_eq!(calls_state(&link.frame().await), FeatureAvailability::Ready);
    link.service
        .availability_changed(&mut link.session, Availability::Disabled)
        .await
        .unwrap();
    assert_eq!(
        calls_state(&link.frame().await),
        FeatureAvailability::Disabled
    );
}

#[tokio::test]
async fn asks_the_phone_to_pair_once_calls_are_ready_on_both_devices() {
    let mut link = Link::connect(Availability::Ready).await;
    // The phone has not said whether it can.
    assert!(!link.ask().await);
    link.status(FeatureAvailability::Ready).await;
    assert!(link.ask().await);
    assert!(link.service.is_asking());
    let sent = link.frame().await;
    assert!(sent.expects_reply);
    assert_eq!(
        sent.payload,
        Some(Payload::BluetoothPairingRequest(request_to_pair()))
    );
    // One request at a time.
    assert!(!link.ask().await);

    link.send(answer(link.seq, sent.seq, Payload::Ack(Ack::default())))
        .await;
    assert!(matches!(
        link.step().await,
        Handled::Answered(PhoneAnswer::Asked)
    ));
    assert!(!link.service.is_asking());
}

#[tokio::test]
async fn a_phone_without_bluetooth_refuses() {
    let mut link = Link::connect(Availability::Ready).await;
    link.status(FeatureAvailability::Ready).await;
    assert!(link.ask().await);
    let sent = link.frame().await;
    let refused = Payload::ErrorReply(ErrorReply {
        code: ErrorCode::Unavailable.into(),
        ..ErrorReply::default()
    });
    link.send(answer(link.seq, sent.seq, refused)).await;
    assert!(matches!(
        link.step().await,
        Handled::Answered(PhoneAnswer::Refused(ErrorCode::Unavailable))
    ));
}

#[tokio::test]
async fn a_request_that_waits_when_the_session_ends_got_no_answer() {
    let mut link = Link::connect(Availability::Ready).await;
    assert_eq!(link.service.session_ended(), None);
    link.status(FeatureAvailability::Ready).await;
    assert!(link.ask().await);
    assert_eq!(link.service.session_ended(), Some(PhoneAnswer::NoAnswer));
    assert!(!link.service.is_asking());
}

#[tokio::test]
async fn the_desktop_is_never_asked_to_pair() {
    let mut link = Link::connect(Availability::Ready).await;
    link.status(FeatureAvailability::Ready).await;
    link.send(request(
        link.seq,
        Payload::BluetoothPairingRequest(request_to_pair()),
    ))
    .await;
    assert!(matches!(link.step().await, Handled::Taken));
    let refused = link.frame().await;
    match refused.payload {
        Some(Payload::ErrorReply(error)) => assert_eq!(error.code(), ErrorCode::Unsupported),
        other => panic!("expected an ErrorReply, got {other:?}"),
    }
}

#[tokio::test]
async fn a_desktop_that_cannot_take_calls_does_not_ask() {
    let mut link = Link::connect(Availability::Disabled).await;
    link.status(FeatureAvailability::Ready).await;
    assert!(!link.ask().await);
}
