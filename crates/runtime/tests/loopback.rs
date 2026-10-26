//! The runtime against a stand-in phone over real TLS on the loopback
//! interface: pairing by QR code, reconnecting as a paired phone,
//! mirroring a notification to the desktop and dismissing it again,
//! unpairing, and telling the phone's refusal of this desktop's key from a
//! forged one.

#[path = "../../core/tests/support/mod.rs"]
mod support;

use std::net::{IpAddr, Ipv4Addr};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use link2linux_core::device::{DeviceInfo, DeviceKind, Feature};
use link2linux_core::discovery::LocalAddress;
use link2linux_core::identity::{CertificateFingerprint, Identity};
use link2linux_core::pairing::QrPayload;
use link2linux_core::session::{Session, SessionConfig, SessionEvent};
use link2linux_platform::{
    DesktopNotification, NotificationCapabilities, NotificationId, NotificationResponse,
    NotificationSink, PlatformError,
};
use link2linux_protocol::Payload;
use link2linux_protocol::v1::{
    GoodbyeReason, NotificationCategory, NotificationContent, NotificationImportance,
    NotificationPosted, NotificationSyncBatch, PhoneNotification, SyncCursor,
};
use link2linux_runtime::notify::ServiceFactory;
use link2linux_runtime::paths::DataDir;
use link2linux_runtime::view::{
    Device, DeviceStatus, EventSink, NotificationEvent, PairingMethod, PairingState, PhoneTarget,
};
use link2linux_runtime::{Runtime, RuntimeConfig, Shell};
use support::{PhoneTls, TestPhone};
use tokio::net::TcpStream;
use tokio::sync::{Notify, mpsc};
use tokio_rustls::server::TlsStream;

const LOCALHOST: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);
const WAIT: Duration = Duration::from_secs(20);

/// A notification service that records what it is asked and lets the test
/// act on the banners.
struct FakeSink {
    shown: Mutex<Vec<(NotificationId, DesktopNotification)>>,
    closed: Mutex<Vec<NotificationId>>,
    next_id: AtomicU32,
    responses: tokio::sync::Mutex<mpsc::UnboundedReceiver<NotificationResponse>>,
    changed: Notify,
}

impl FakeSink {
    fn new() -> (Arc<Self>, mpsc::UnboundedSender<NotificationResponse>) {
        let (respond, responses) = mpsc::unbounded_channel();
        let sink = Arc::new(Self {
            shown: Mutex::default(),
            closed: Mutex::default(),
            next_id: AtomicU32::new(1),
            responses: tokio::sync::Mutex::new(responses),
            changed: Notify::new(),
        });
        (sink, respond)
    }

    async fn wait_for_banners(&self, count: usize) -> Vec<(NotificationId, DesktopNotification)> {
        wait(&self.changed, || {
            let shown = self.shown.lock().unwrap();
            (shown.len() >= count).then(|| shown.clone())
        })
        .await
    }
}

impl NotificationSink for FakeSink {
    async fn show(
        &self,
        notification: &DesktopNotification,
        _replaces: Option<NotificationId>,
    ) -> Result<NotificationId, PlatformError> {
        let id = NotificationId(self.next_id.fetch_add(1, Ordering::Relaxed));
        self.shown.lock().unwrap().push((id, notification.clone()));
        self.changed.notify_waiters();
        Ok(id)
    }

    async fn close(&self, id: NotificationId) -> Result<(), PlatformError> {
        self.closed.lock().unwrap().push(id);
        self.changed.notify_waiters();
        Ok(())
    }

    async fn capabilities(&self) -> Result<NotificationCapabilities, PlatformError> {
        Ok(NotificationCapabilities {
            actions: true,
            inline_reply: true,
            images: false,
            persistence: false,
        })
    }

    async fn next_response(&self) -> Result<NotificationResponse, PlatformError> {
        match self.responses.lock().await.recv().await {
            Some(response) => Ok(response),
            None => std::future::pending().await,
        }
    }
}

/// Collects what the runtime reports.
#[derive(Default)]
struct Events {
    pairing: Mutex<Vec<PairingState>>,
    devices: Mutex<Vec<Vec<Device>>>,
    notifications: Mutex<Vec<(String, NotificationEvent)>>,
    changed: Notify,
}

impl EventSink for Events {
    fn pairing_changed(&self, state: &PairingState) {
        self.pairing.lock().unwrap().push(state.clone());
        self.changed.notify_waiters();
    }

    fn devices_changed(&self, devices: &[Device]) {
        self.devices.lock().unwrap().push(devices.to_vec());
        self.changed.notify_waiters();
    }

    fn notification_changed(&self, device_id: &str, event: &NotificationEvent) {
        self.notifications
            .lock()
            .unwrap()
            .push((device_id.to_owned(), event.clone()));
        self.changed.notify_waiters();
    }
}

impl Events {
    async fn pairing_matching<T>(&self, pick: impl Fn(&PairingState) -> Option<T>) -> T {
        wait(&self.changed, || {
            self.pairing.lock().unwrap().iter().find_map(&pick)
        })
        .await
    }

    async fn devices_where(&self, check: impl Fn(&[Device]) -> bool) -> Vec<Device> {
        wait(&self.changed, || {
            self.devices
                .lock()
                .unwrap()
                .iter()
                .rev()
                .find(|devices| check(devices))
                .cloned()
        })
        .await
    }
}

/// Waits until `check` finds something, woken by `changed`.
async fn wait<T>(changed: &Notify, check: impl Fn() -> Option<T>) -> T {
    let result = tokio::time::timeout(WAIT, async {
        loop {
            let notified = changed.notified();
            if let Some(found) = check() {
                return found;
            }
            notified.await;
        }
    })
    .await;
    result.expect("timed out waiting for the runtime")
}

struct Rig {
    runtime: Runtime,
    events: Arc<Events>,
    sink: Arc<FakeSink>,
    respond: mpsc::UnboundedSender<NotificationResponse>,
    _data: tempfile::TempDir,
}

fn start() -> Rig {
    let data = tempfile::tempdir().unwrap();
    let (sink, respond) = FakeSink::new();
    let events = Arc::new(Events::default());
    let mut config = RuntimeConfig::new(DataDir::new(data.path().join("link2linux")));
    config.discovery = None;
    config.desktop_name = Some("Desk".into());
    config.extra_locals = vec![LocalAddress {
        ip: LOCALHOST,
        prefix_len: 8,
        interface_index: None,
    }];
    let presenter: Arc<dyn link2linux_runtime::notify::Presenter> = sink.clone();
    let shell = Shell {
        identity: Identity::generate().unwrap(),
        drivers: Arc::new(ServiceFactory::new(
            presenter.clone(),
            Some(data.path().join("images")),
        )),
        presenter,
        events: events.clone(),
    };
    let runtime = Runtime::start(tokio::runtime::Handle::current(), &config, shell).unwrap();
    Rig {
        runtime,
        events,
        sink,
        respond,
        _data: data,
    }
}

/// Pairs the stand-in phone by QR code and returns the desktop's key.
async fn pair(rig: &Rig, phone: &Arc<TestPhone>) -> (CertificateFingerprint, Device) {
    rig.runtime.start_pairing(PairingMethod::QrCode).unwrap();
    let uri = rig
        .events
        .pairing_matching(|state| match state {
            PairingState::QrOffer { uri, svg, .. } => {
                assert!(svg.contains("<svg"), "the code is drawn");
                Some(uri.clone())
            }
            _ => None,
        })
        .await;
    let payload = QrPayload::parse(&uri).unwrap();

    let scanned = phone.clone();
    let desktop = payload.desktop;
    let secret = payload.secret.clone();
    let phone_side = tokio::spawn(async move {
        let mut connection = scanned.accept(PhoneTls::pinned(desktop)).await.unwrap();
        let report = connection.pair_by_qr(&scanned, &secret).await;
        // The connection goes on as a session: the desktop announces its
        // features and presence.
        let first = connection.wire.recv_payload().await.unwrap();
        assert!(
            matches!(first, Some(Payload::FeatureStatus(_))),
            "{first:?}"
        );
        connection.wire.finish().await;
        report
    });
    rig.runtime
        .pair_with(PhoneTarget::Address {
            address: phone.addr().to_string(),
        })
        .unwrap();

    let device = rig
        .events
        .pairing_matching(|state| match state {
            PairingState::Paired { device } => Some(device.clone()),
            PairingState::Failed { reason, .. } => panic!("pairing failed: {reason:?}"),
            _ => None,
        })
        .await;
    let report = phone_side.await.unwrap();
    assert_eq!(
        report.desktop_outcome,
        Some(link2linux_core::pairing::PairingOutcome::Accepted)
    );
    (desktop, device)
}

/// The phone side of a session with the paired desktop.
async fn phone_session(
    phone: &TestPhone,
    desktop: CertificateFingerprint,
) -> Session<TlsStream<TcpStream>> {
    let connection = phone.accept(PhoneTls::pinned(desktop)).await.unwrap();
    let info = DeviceInfo::new(
        phone.id.clone(),
        phone.name.clone(),
        DeviceKind::Phone,
        [Feature::Notifications],
    );
    Session::handshake(connection.wire.into_inner(), info, SessionConfig::phone())
        .await
        .unwrap()
}

/// Pairs the stand-in phone and runs one session, in which the desktop
/// confirms the record. The phone then ends it as a restarting phone does.
async fn pair_and_confirm(rig: &Rig, phone: &Arc<TestPhone>) -> CertificateFingerprint {
    let (desktop, _) = pair(rig, phone).await;
    let mut session = phone_session(phone, desktop).await;
    // The desktop asks for the notifications once the phone's
    // FeatureStatus is in, and confirms the record right after.
    loop {
        if let SessionEvent::Request {
            payload: Payload::NotificationSyncRequest(_),
            ..
        } = next_event(&mut session).await
        {
            break;
        }
    }
    session
        .goodbye(GoodbyeReason::Shutdown, "restarting")
        .await
        .unwrap();
    desktop
}

async fn next_event(session: &mut Session<TlsStream<TcpStream>>) -> SessionEvent {
    tokio::time::timeout(WAIT, session.next_event())
        .await
        .expect("timed out waiting for the desktop")
        .unwrap()
        .expect("the desktop ended the session")
}

fn lunch() -> PhoneNotification {
    PhoneNotification {
        key: "k1".into(),
        revision: 2,
        app_name: "Chat".into(),
        title: "Ana".into(),
        text: "Lunch?".into(),
        category: NotificationCategory::Message.into(),
        importance: NotificationImportance::High.into(),
        content: NotificationContent::Full.into(),
        dismissible: true,
        ..PhoneNotification::default()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_phone_pairs_reconnects_and_mirrors_a_notification() {
    let rig = start();
    let phone = Arc::new(TestPhone::bind(LOCALHOST, "phone-1").await);

    // Pairing by QR code through the real runtime.
    let (desktop, device) = pair(&rig, &phone).await;
    assert_eq!(device.name, "Phone phone-1");
    assert_eq!(device.id, "phone-1");
    // The phone ended the pairing connection at once, so the state may
    // already be "reconnecting".
    let listed = rig.runtime.devices();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, device.id);

    // The pairing connection ended; the desktop dials the phone again at
    // the address it remembered, as the paired phone it now is.
    let mut session = phone_session(&phone, desktop).await;

    // It syncs first, and the phone answers.
    let request = loop {
        if let SessionEvent::Request {
            seq,
            payload: Payload::NotificationSyncRequest(_),
        } = next_event(&mut session).await
        {
            break seq;
        }
    };
    let batch = NotificationSyncBatch {
        reset: true,
        cursor: Some(SyncCursor {
            epoch: 1,
            revision: 1,
        }),
        ..NotificationSyncBatch::default()
    };
    session
        .reply(request, Payload::NotificationSyncBatch(batch))
        .await
        .unwrap();

    // A new notification becomes a desktop banner.
    session
        .send(Payload::NotificationPosted(NotificationPosted {
            notification: Some(lunch()),
            alert: true,
        }))
        .await
        .unwrap();
    let banners = rig.sink.wait_for_banners(1).await;
    let (id, banner) = &banners[0];
    assert_eq!(banner.summary, "Chat · Ana");
    assert_eq!(banner.body, "Lunch?");
    assert_eq!(banner.app_name, "Chat");

    // The notifications page is told, and can list it.
    let key =
        wait(&rig.events.changed, || {
            rig.events.notifications.lock().unwrap().iter().find_map(
                |(device, event)| match event {
                    NotificationEvent::Posted {
                        notification,
                        alert,
                    } => {
                        assert_eq!((device.as_str(), *alert), ("phone-1", true));
                        Some(notification.key.clone())
                    }
                    NotificationEvent::Removed { .. } => None,
                },
            )
        })
        .await;
    assert_eq!(key, "k1");
    let listed = rig.runtime.notifications("phone-1").unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].title, "Ana");
    assert!(rig.runtime.notifications("nobody").is_err());

    // The device shows as connected.
    let connected = rig
        .events
        .devices_where(|devices| devices.iter().all(|d| d.status == DeviceStatus::Connected))
        .await;
    assert_eq!(connected.len(), 1);

    // Dismissing the banner dismisses the notification on the phone.
    rig.respond
        .send(NotificationResponse::Closed {
            id: *id,
            reason: link2linux_platform::CloseReason::Dismissed,
        })
        .unwrap();
    let (seq, dismissed) = loop {
        if let SessionEvent::Request {
            seq,
            payload: Payload::NotificationDismissRequest(request),
        } = next_event(&mut session).await
        {
            break (seq, request);
        }
    };
    assert_eq!(dismissed.key, "k1");
    assert_eq!(dismissed.idempotency_key.len(), 16);
    session.ack(seq).await.unwrap();

    // The page can run an action, and it reaches the phone.
    rig.runtime
        .run_notification_action("phone-1", "k1", "mark", Some("on my way"))
        .unwrap();
    let (seq, action) = loop {
        if let SessionEvent::Request {
            seq,
            payload: Payload::NotificationActionRequest(request),
        } = next_event(&mut session).await
        {
            break (seq, request);
        }
    };
    assert_eq!(
        (
            action.key.as_str(),
            action.action_id.as_str(),
            action.reply_text.as_str()
        ),
        ("k1", "mark", "on my way")
    );
    session.ack(seq).await.unwrap();

    // Unpairing says goodbye and forgets the phone.
    rig.runtime.unpair("phone-1").unwrap();
    loop {
        if let SessionEvent::Goodbye(goodbye) = next_event(&mut session).await {
            assert_eq!(goodbye.reason, GoodbyeReason::Unpaired);
            break;
        }
    }
    assert!(rig.runtime.devices().is_empty());
    rig.events.devices_where(<[Device]>::is_empty).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_phone_pairs_by_comparing_codes_and_a_wrong_answer_costs_an_attempt() {
    use link2linux_core::pairing::SasDecision;
    use link2linux_runtime::view::PairingFailure;

    let rig = start();
    let phone = Arc::new(TestPhone::bind(LOCALHOST, "phone-2").await);
    rig.runtime.start_pairing(PairingMethod::Sas).unwrap();
    rig.events
        .pairing_matching(|state| matches!(state, PairingState::Searching { .. }).then_some(()))
        .await;
    let target = PhoneTarget::Address {
        address: phone.addr().to_string(),
    };

    // First attempt: the user says the codes differ.
    let first = phone.clone();
    let phone_side = tokio::spawn(async move {
        let mut connection = first.accept(PhoneTls::sas_mode()).await.unwrap();
        connection
            .pair_by_sas(&first, false, SasDecision::Declined)
            .await
    });
    rig.runtime.pair_with(target.clone()).unwrap();
    rig.events
        .pairing_matching(|state| matches!(state, PairingState::SasCode { .. }).then_some(()))
        .await;
    rig.runtime.answer_code(false).unwrap();
    phone_side.await.unwrap();
    let attempts_left = rig
        .events
        .pairing_matching(|state| match state {
            PairingState::Searching {
                attempts_left,
                last_failure: Some(PairingFailure::VerificationFailed),
                ..
            } => Some(*attempts_left),
            _ => None,
        })
        .await;
    assert_eq!(attempts_left, 2);

    // Second attempt: the codes match.
    let second = phone.clone();
    let phone_side = tokio::spawn(async move {
        let mut connection = second.accept(PhoneTls::sas_mode()).await.unwrap();
        let report = connection
            .pair_by_sas(&second, false, SasDecision::Confirmed)
            .await;
        let first = connection.wire.recv_payload().await.unwrap();
        assert!(
            matches!(first, Some(Payload::FeatureStatus(_))),
            "{first:?}"
        );
        connection.wire.finish().await;
        report
    });
    rig.runtime.pair_with(target).unwrap();
    let shown = loop {
        let state = rig.runtime.pairing();
        if let PairingState::SasCode { code, .. } = state {
            break code;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    rig.runtime.answer_code(true).unwrap();
    let device = rig
        .events
        .pairing_matching(|state| match state {
            PairingState::Paired { device } => Some(device.clone()),
            _ => None,
        })
        .await;
    assert_eq!(device.id, "phone-2");
    let report = phone_side.await.unwrap();
    assert_eq!(
        report.code.unwrap().as_str(),
        shown,
        "both screens show one code"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_alert_from_whoever_holds_the_address_keeps_the_pairing() {
    let rig = start();
    let phone = Arc::new(TestPhone::bind(LOCALHOST, "phone-3").await);
    let desktop = pair_and_confirm(&rig, &phone).await;

    // The next dials reach a host that answers with a plaintext alert. Each
    // is an ordinary failed attempt, after which the desktop dials again.
    for _ in 0..2 {
        tokio::time::timeout(WAIT, phone.forge_refusal())
            .await
            .expect("the desktop dials again")
            .unwrap();
    }
    let listed = rig.runtime.devices();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].status, DeviceStatus::Reconnecting);

    // The phone itself still gets a session.
    rig.runtime.reconnect("phone-3");
    phone_session(&phone, desktop).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_phone_that_refuses_the_key_after_the_handshake_removed_the_pairing() {
    let rig = start();
    let phone = Arc::new(TestPhone::bind(LOCALHOST, "phone-4").await);
    pair_and_confirm(&rig, &phone).await;

    // The phone forgot this desktop: its TLS stack refuses the key once the
    // handshake has checked the phone's.
    let stranger = Identity::generate().unwrap().fingerprint();
    assert!(phone.accept(PhoneTls::pinned(stranger)).await.is_err());
    let devices = rig
        .events
        .devices_where(|devices| devices.iter().any(|d| d.status == DeviceStatus::Revoked))
        .await;
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].id, "phone-4");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unfinished_pairing_the_phone_refuses_is_deleted() {
    let rig = start();
    let phone = Arc::new(TestPhone::bind(LOCALHOST, "phone-5").await);
    // The phone ended the pairing connection before storing the pairing,
    // so the record stays unconfirmed.
    pair(&rig, &phone).await;

    let stranger = Identity::generate().unwrap().fingerprint();
    assert!(phone.accept(PhoneTls::pinned(stranger)).await.is_err());
    rig.events.devices_where(<[Device]>::is_empty).await;
    assert!(rig.runtime.devices().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_code_pairing_never_replaces_a_phone_with_the_same_id_on_its_own() {
    use link2linux_core::pairing::{PairingOutcome, SasDecision};
    use link2linux_runtime::view::PairingFailure;

    let rig = start();
    let phone = Arc::new(TestPhone::bind(LOCALHOST, "phone-6").await);
    let desktop = pair_and_confirm(&rig, &phone).await;

    // Another key claims the phone's device id, and the codes match.
    let claimant = Arc::new(TestPhone::bind(LOCALHOST, "phone-6").await);
    rig.runtime.start_pairing(PairingMethod::Sas).unwrap();
    let side = claimant.clone();
    let phone_side = tokio::spawn(async move {
        let mut connection = side.accept(PhoneTls::sas_mode()).await.unwrap();
        connection
            .pair_by_sas(&side, false, SasDecision::Confirmed)
            .await
    });
    rig.runtime
        .pair_with(PhoneTarget::Address {
            address: claimant.addr().to_string(),
        })
        .unwrap();
    rig.events
        .pairing_matching(|state| matches!(state, PairingState::SasCode { .. }).then_some(()))
        .await;
    rig.runtime.answer_code(true).unwrap();

    // The screen cannot ask about replacing the paired phone, so nothing is.
    let report = phone_side.await.unwrap();
    assert_eq!(report.desktop_outcome, Some(PairingOutcome::Declined));
    let failure = rig
        .events
        .pairing_matching(|state| match state {
            PairingState::Searching {
                last_failure: Some(failure),
                ..
            } => Some(*failure),
            _ => None,
        })
        .await;
    assert_eq!(failure, PairingFailure::AlreadyPaired);
    assert_eq!(rig.runtime.devices().len(), 1);

    // The paired phone keeps its record and its task, which dials it.
    rig.runtime.reconnect("phone-6");
    tokio::time::timeout(WAIT, phone_session(&phone, desktop))
        .await
        .expect("the desktop dials the paired phone");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn nothing_is_paired_with_a_phone_that_is_not_there() {
    use link2linux_runtime::view::PairingFailure;

    let rig = start();
    rig.runtime.start_pairing(PairingMethod::QrCode).unwrap();
    // Nobody listens on this loopback port.
    let unused = TestPhone::bind(LOCALHOST, "gone").await;
    let address = unused.addr().to_string();
    drop(unused);
    rig.runtime
        .pair_with(PhoneTarget::Address { address })
        .unwrap();
    let reason = rig
        .events
        .pairing_matching(|state| match state {
            PairingState::Failed { reason, .. } => Some(*reason),
            _ => None,
        })
        .await;
    assert_eq!(reason, PairingFailure::PhoneNotFound);
    assert!(rig.runtime.devices().is_empty());
    // Dismissing the failure returns to idle.
    rig.runtime.cancel_pairing();
    assert_eq!(rig.runtime.pairing(), PairingState::Idle);
}
