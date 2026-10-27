//! The runtime against a stand-in phone over real TLS on the loopback
//! interface: pairing by QR code, reconnecting as a paired phone,
//! mirroring a notification to the desktop and dismissing it again,
//! sharing the clipboard both ways, showing the phone's status and ringing
//! it, unpairing, telling the phone's refusal
//! of this desktop's key from a forged one, and keeping a QR code up after
//! an impostor failed.

#[path = "../../core/tests/support/mod.rs"]
mod support;

use std::net::{IpAddr, Ipv4Addr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use link2linux_core::device::{DeviceInfo, DeviceKind, Feature};
use link2linux_core::discovery::LocalAddress;
use link2linux_core::files::{
    Direction, FileService, Transfer, TransferEvent, TransferId, TransferState,
};
use link2linux_core::identity::{CertificateFingerprint, Identity};
use link2linux_core::notifications::present::OPEN_KEY;
use link2linux_core::notifications::{Answer, Change};
use link2linux_core::pairing::{QrPayload, StoreKey};
use link2linux_core::session::{Availability, Reply, Session, SessionConfig, SessionEvent};
use link2linux_platform::{
    BluetoothAdapter, BluetoothAddress, BluetoothDevice, CallState, Clipboard, ClipboardAccess,
    ClipboardText, DesktopNotification, FileStore, Gateway, GatewayCall, NotificationCapabilities,
    NotificationId, NotificationResponse, NotificationSink, PairingEvent, PairingPrompt,
    PlatformError,
};
use link2linux_protocol::Payload;
use link2linux_protocol::v1::{
    BluetoothPairingRequest, ClipboardPush, ErrorCode, FileAccept, FileChunk, FileInfo, FileOffer,
    GoodbyeReason, NotificationAction, NotificationCategory, NotificationContent,
    NotificationImportance, NotificationPosted, NotificationSyncBatch, PairingProof,
    PhoneNotification, PhoneStatusChanged, PhoneStatusReply, RingEndReason, RingEnded, SyncCursor,
};
use link2linux_runtime::calls::CallsPort;
use link2linux_runtime::clipboard::ClipboardPort;
use link2linux_runtime::notify::{
    BoxFuture, DriverError, DriverFactory, NotificationsDriver, PhoneSession, ServiceFactory,
};
use link2linux_runtime::paths::DataDir;
use link2linux_runtime::view::{
    CallLink, CallStateView, CallsDesktop, CallsStatus, ClipboardDirection, ClipboardMode,
    ClipboardPhoneState, ClipboardProblemKind, ClipboardStatus, Device, DeviceStatus, EventSink,
    FileTransfer, FileTransferEvent, NotificationEvent, PairingMethod, PairingRequestState,
    PairingState, PhoneStatus, PhoneTarget, RingEnd, RingPhase, RingProblem, RingStatus,
    TransferDirection, TransferStatus,
};
use link2linux_runtime::{Runtime, RuntimeConfig, Shell};
use support::{PhoneTls, TestPhone};
use tokio::net::TcpStream;
use tokio::sync::{Notify, broadcast, mpsc, oneshot, watch};
use tokio::task::JoinHandle;
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

/// The desktop clipboard, in memory. The test copies on it as the user
/// would.
struct FakeClipboard {
    access: Mutex<ClipboardAccess>,
    /// The display server went away, and `changed` has not said so yet.
    lost: AtomicBool,
    content: Mutex<Option<ClipboardText>>,
    /// Counts the changes; `changed` reports each one once, also one made
    /// while nobody waited.
    generation: watch::Sender<u64>,
    reported: AtomicU64,
    reads: AtomicUsize,
    touched: Notify,
    /// Writes fail, as when the clipboard cannot be set.
    fail_writes: AtomicBool,
    /// The next read takes the content, says so through `read_started`,
    /// and returns it once `release_read` is told: a slow read that a
    /// phone's write can overtake.
    stall_next_read: AtomicBool,
    read_started: Notify,
    release_read: Notify,
    /// Likewise for the next write, which takes effect once released.
    stall_next_write: AtomicBool,
    write_started: Notify,
    release_write: Notify,
}

impl FakeClipboard {
    fn new(access: ClipboardAccess) -> Arc<Self> {
        Arc::new(Self {
            access: Mutex::new(access),
            lost: AtomicBool::new(false),
            content: Mutex::default(),
            generation: watch::Sender::new(0),
            reported: AtomicU64::new(0),
            reads: AtomicUsize::new(0),
            touched: Notify::new(),
            fail_writes: AtomicBool::new(false),
            stall_next_read: AtomicBool::new(false),
            read_started: Notify::new(),
            release_read: Notify::new(),
            stall_next_write: AtomicBool::new(false),
            write_started: Notify::new(),
            release_write: Notify::new(),
        })
    }

    fn text(&self) -> Option<ClipboardText> {
        self.content.lock().unwrap().clone()
    }

    fn set(&self, text: ClipboardText) {
        *self.content.lock().unwrap() = Some(text);
        self.generation.send_modify(|generation| *generation += 1);
        self.touched.notify_waiters();
    }

    /// The user copies `text` in another app.
    fn copy(&self, text: &str) {
        self.set(ClipboardText {
            text: text.into(),
            sensitive: false,
        });
    }

    /// The display server goes away: the clipboard turns focus-only, as
    /// the platform's does, and `changed` fails once.
    fn lose(&self) {
        *self.access.lock().unwrap() = ClipboardAccess::FocusOnly;
        self.lost.store(true, Ordering::Relaxed);
        self.generation.send_modify(|generation| *generation += 1);
    }

    async fn wait_for(&self, check: impl Fn(&Self) -> bool) {
        wait(&self.touched, || check(self).then_some(())).await;
    }
}

impl Clipboard for FakeClipboard {
    fn access(&self) -> ClipboardAccess {
        *self.access.lock().unwrap()
    }

    async fn read(&self) -> Result<Option<ClipboardText>, PlatformError> {
        let text = self.text();
        if self.stall_next_read.swap(false, Ordering::Relaxed) {
            self.read_started.notify_one();
            self.release_read.notified().await;
        }
        self.reads.fetch_add(1, Ordering::Relaxed);
        self.touched.notify_waiters();
        Ok(text)
    }

    async fn write(&self, text: &ClipboardText) -> Result<(), PlatformError> {
        if self.fail_writes.load(Ordering::Relaxed) {
            return Err(PlatformError::Unavailable {
                service: "clipboard",
                reason: "the clipboard cannot be set".into(),
            });
        }
        if self.stall_next_write.swap(false, Ordering::Relaxed) {
            self.write_started.notify_one();
            self.release_write.notified().await;
        }
        self.set(text.clone());
        Ok(())
    }

    async fn changed(&self) -> Result<(), PlatformError> {
        let mut generations = self.generation.subscribe();
        loop {
            if self.lost.swap(false, Ordering::Relaxed) {
                return Err(PlatformError::Unavailable {
                    service: "clipboard",
                    reason: "the display server went away".into(),
                });
            }
            if Clipboard::access(self) == ClipboardAccess::FocusOnly {
                std::future::pending::<()>().await;
            }
            let current = *generations.borrow_and_update();
            if current > self.reported.load(Ordering::Relaxed) {
                self.reported.store(current, Ordering::Relaxed);
                return Ok(());
            }
            generations.changed().await.unwrap();
        }
    }
}

/// Notifications that fail on every `FeatureStatus`, as when their store
/// cannot be written.
struct FailingDrivers;

impl DriverFactory for FailingDrivers {
    fn create(&self, _device: &DeviceInfo) -> Box<dyn NotificationsDriver> {
        Box::new(FailingDriver(broadcast::channel(1).0))
    }
}

struct FailingDriver(broadcast::Sender<Change>);

impl NotificationsDriver for FailingDriver {
    fn session_started<'a>(
        &'a mut self,
        _session: &'a mut PhoneSession,
    ) -> BoxFuture<'a, Result<(), DriverError>> {
        Box::pin(async { Ok(()) })
    }

    fn handle_event<'a>(
        &'a mut self,
        _session: &'a mut PhoneSession,
        event: SessionEvent,
    ) -> BoxFuture<'a, Result<Option<SessionEvent>, DriverError>> {
        Box::pin(async move {
            match event {
                SessionEvent::FeatureStatus(_) => {
                    Err(DriverError::Failed("the store is full".into()))
                }
                other => Ok(Some(other)),
            }
        })
    }

    fn handle_response<'a>(
        &'a mut self,
        _session: Option<&'a mut PhoneSession>,
        _response: NotificationResponse,
    ) -> BoxFuture<'a, Result<Option<Answer>, DriverError>> {
        Box::pin(async { Ok(None) })
    }

    fn session_ended(&mut self) {}

    fn subscribe(&self) -> broadcast::Receiver<Change> {
        self.0.subscribe()
    }

    fn dismiss<'a>(
        &'a mut self,
        _session: Option<&'a mut PhoneSession>,
        _key: &'a str,
    ) -> BoxFuture<'a, Result<(), DriverError>> {
        Box::pin(async { Ok(()) })
    }

    fn run_action<'a>(
        &'a mut self,
        _session: Option<&'a mut PhoneSession>,
        _key: &'a str,
        _action_id: &'a str,
        _reply: Option<&'a str>,
    ) -> BoxFuture<'a, Result<(), DriverError>> {
        Box::pin(async { Ok(()) })
    }
}

/// Collects what the runtime reports.
#[derive(Default)]
struct Events {
    pairing: Mutex<Vec<PairingState>>,
    devices: Mutex<Vec<Vec<Device>>>,
    notifications: Mutex<Vec<(String, NotificationEvent)>>,
    clipboard: Mutex<Vec<ClipboardStatus>>,
    transfers: Mutex<Vec<(String, FileTransferEvent)>>,
    phone_status: Mutex<Vec<(String, PhoneStatus)>>,
    rings: Mutex<Vec<(String, RingStatus)>>,
    calls: Mutex<Vec<CallsStatus>>,
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

    fn transfer_changed(&self, device_id: &str, event: &FileTransferEvent) {
        self.transfers
            .lock()
            .unwrap()
            .push((device_id.to_owned(), event.clone()));
        self.changed.notify_waiters();
    }

    fn clipboard_changed(&self, status: &ClipboardStatus) {
        self.clipboard.lock().unwrap().push(status.clone());
        self.changed.notify_waiters();
    }

    fn phone_status_changed(&self, device_id: &str, status: &PhoneStatus) {
        self.phone_status
            .lock()
            .unwrap()
            .push((device_id.to_owned(), status.clone()));
        self.changed.notify_waiters();
    }

    fn ring_changed(&self, device_id: &str, ring: &RingStatus) {
        self.rings
            .lock()
            .unwrap()
            .push((device_id.to_owned(), *ring));
        self.changed.notify_waiters();
    }

    fn calls_changed(&self, status: &CallsStatus) {
        self.calls.lock().unwrap().push(status.clone());
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

    async fn calls_where(&self, check: impl Fn(&CallsStatus) -> bool) -> CallsStatus {
        wait(&self.changed, || {
            self.calls
                .lock()
                .unwrap()
                .iter()
                .rev()
                .find(|status| check(status))
                .cloned()
        })
        .await
    }

    async fn clipboard_where(&self, check: impl Fn(&ClipboardStatus) -> bool) -> ClipboardStatus {
        wait(&self.changed, || {
            self.clipboard
                .lock()
                .unwrap()
                .iter()
                .rev()
                .find(|status| check(status))
                .cloned()
        })
        .await
    }

    /// Waits until the latest status of phone `id` passes `check`.
    async fn status_where(&self, id: &str, check: impl Fn(&PhoneStatus) -> bool) -> PhoneStatus {
        wait(&self.changed, || {
            let reported = self.phone_status.lock().unwrap();
            let (_, latest) = reported.iter().rev().find(|(device, _)| device == id)?;
            check(latest).then(|| latest.clone())
        })
        .await
    }

    /// Waits until the latest ring status of phone `id` passes `check`.
    async fn ring_where(&self, id: &str, check: impl Fn(&RingStatus) -> bool) -> RingStatus {
        wait(&self.changed, || {
            let reported = self.rings.lock().unwrap();
            let (_, latest) = reported.iter().rev().find(|(device, _)| device == id)?;
            check(latest).then_some(*latest)
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

/// Waits for something the desktop is expected to do, such as dialing.
async fn within<T>(expected: impl Future<Output = T>) -> T {
    tokio::time::timeout(WAIT, expected)
        .await
        .expect("timed out waiting for the desktop")
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

/// The desktop's files: a Downloads directory of the test's own, and a
/// record of what the user opened.
struct FakeFiles {
    downloads: PathBuf,
    opened: Mutex<Vec<(&'static str, PathBuf)>>,
    changed: Notify,
}

impl FileStore for FakeFiles {
    fn downloads_dir(&self) -> Result<PathBuf, PlatformError> {
        Ok(self.downloads.clone())
    }

    async fn open(&self, path: &Path) -> Result<(), PlatformError> {
        self.opened.lock().unwrap().push(("open", path.to_owned()));
        self.changed.notify_waiters();
        Ok(())
    }

    async fn show_in_folder(&self, path: &Path) -> Result<(), PlatformError> {
        self.opened.lock().unwrap().push(("show", path.to_owned()));
        self.changed.notify_waiters();
        Ok(())
    }
}

struct Rig {
    runtime: Runtime,
    events: Arc<Events>,
    sink: Arc<FakeSink>,
    respond: mpsc::UnboundedSender<NotificationResponse>,
    files: Arc<FakeFiles>,
    _data: tempfile::TempDir,
}

fn start() -> Rig {
    start_with(None)
}

/// A runtime with `clipboard` as the desktop clipboard.
fn start_with(clipboard: Option<Arc<dyn ClipboardPort>>) -> Rig {
    start_full(clipboard, None)
}

/// A runtime with `clipboard` as the desktop clipboard, and `drivers` for
/// the phones' notifications instead of the real ones.
fn start_full(
    clipboard: Option<Arc<dyn ClipboardPort>>,
    drivers: Option<Arc<dyn DriverFactory>>,
) -> Rig {
    start_rig(clipboard, drivers, None)
}

/// A runtime as [`start_full`] makes it, with `calls` as the desktop's
/// Bluetooth and telephony.
fn start_rig(
    clipboard: Option<Arc<dyn ClipboardPort>>,
    drivers: Option<Arc<dyn DriverFactory>>,
    calls: Option<Arc<dyn CallsPort>>,
) -> Rig {
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
    let files = Arc::new(FakeFiles {
        downloads: data.path().join("Downloads"),
        opened: Mutex::default(),
        changed: Notify::new(),
    });
    let shell = Shell {
        identity: Identity::generate().unwrap(),
        store_key: Some(StoreKey::generate()),
        drivers: drivers.unwrap_or_else(|| {
            Arc::new(ServiceFactory::new(
                presenter.clone(),
                Some(data.path().join("images")),
            ))
        }),
        presenter,
        clipboard,
        files: Some(files.clone()),
        calls,
        events: events.clone(),
    };
    let runtime = Runtime::start(tokio::runtime::Handle::current(), &config, shell).unwrap();
    Rig {
        runtime,
        events,
        sink,
        respond,
        files,
        _data: data,
    }
}

/// Shows a QR code and returns what a phone reads from it.
async fn show_code(rig: &Rig) -> QrPayload {
    rig.runtime.start_pairing(PairingMethod::QrCode).unwrap();
    let svg = rig
        .events
        .pairing_matching(|state| match state {
            PairingState::QrOffer { svg, .. } => Some(svg.0.clone()),
            _ => None,
        })
        .await;
    let uri = scan(&svg);
    // The code holds the secret: debug output shows neither it nor the URI.
    let printed = format!("{:?}", rig.runtime.pairing());
    assert!(
        !printed.contains(&uri) && !printed.contains("<svg"),
        "{printed}"
    );
    QrPayload::parse(&uri).unwrap()
}

/// Reads the code in an SVG from the runtime, as a phone's camera does.
/// The window only ever gets the image, not the URI with the secret.
fn scan(svg: &str) -> String {
    fn attr<'a>(svg: &'a str, name: &str) -> &'a str {
        let key = format!("{name}=\"");
        let start = svg.find(&key).unwrap() + key.len();
        &svg[start..start + svg[start..].find('"').unwrap()]
    }
    let modules: usize = attr(svg, "viewBox")
        .rsplit(' ')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    let mut dark = vec![false; modules * modules];
    // One run of dark modules each: M<x> <y>h<length>v1h-<length>z
    for run in attr(svg, "d").split('z').filter(|run| !run.is_empty()) {
        let (origin, rest) = run.trim_start_matches('M').split_once('h').unwrap();
        let (x, y) = origin.split_once(' ').unwrap();
        let (length, _) = rest.split_once('v').unwrap();
        let (x, y, length): (usize, usize, usize) = (
            x.parse().unwrap(),
            y.parse().unwrap(),
            length.parse().unwrap(),
        );
        dark[y * modules + x..y * modules + x + length].fill(true);
    }
    let scale = 4;
    let size = modules * scale;
    let mut image = rqrr::PreparedImage::prepare_from_greyscale(size, size, |x, y| {
        if dark[(y / scale) * modules + x / scale] {
            0
        } else {
            255
        }
    });
    let grids = image.detect_grids();
    let (_, content) = grids[0].decode().expect("the code decodes");
    content
}

/// Pairs the stand-in phone by QR code and returns the desktop's key.
async fn pair(rig: &Rig, phone: &Arc<TestPhone>) -> (CertificateFingerprint, Device) {
    let payload = show_code(rig).await;

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

/// The phone side of a session with the paired desktop, for a phone that
/// shares only its clipboard, once both sides report it ready.
async fn clipboard_session(
    phone: &TestPhone,
    desktop: CertificateFingerprint,
) -> Session<TlsStream<TcpStream>> {
    let connection = phone.accept(PhoneTls::pinned(desktop)).await.unwrap();
    let info = DeviceInfo::new(
        phone.id.clone(),
        phone.name.clone(),
        DeviceKind::Phone,
        [Feature::Clipboard],
    );
    let mut session =
        Session::handshake(connection.wire.into_inner(), info, SessionConfig::phone())
            .await
            .unwrap();
    // The desktop announces the clipboard, and reports it ready.
    assert!(session.peer().features.contains(&Feature::Clipboard));
    while !session.is_ready(Feature::Clipboard) {
        next_event(&mut session).await;
    }
    session
}

/// The phone side of a session with the paired desktop, for a phone that
/// shares its status and rings, once both sides report them ready.
async fn status_session(
    phone: &TestPhone,
    desktop: CertificateFingerprint,
) -> Session<TlsStream<TcpStream>> {
    let connection = phone.accept(PhoneTls::pinned(desktop)).await.unwrap();
    let info = DeviceInfo::new(
        phone.id.clone(),
        phone.name.clone(),
        DeviceKind::Phone,
        [Feature::Status, Feature::Ring],
    );
    let mut session =
        Session::handshake(connection.wire.into_inner(), info, SessionConfig::phone())
            .await
            .unwrap();
    while !(session.is_ready(Feature::Status) && session.is_ready(Feature::Ring)) {
        next_event(&mut session).await;
    }
    session
}

/// The desktop's next request, which the phone takes; returns its
/// sequence number and payload.
async fn next_request(session: &mut Session<TlsStream<TcpStream>>) -> (u64, Payload) {
    loop {
        if let SessionEvent::Request { seq, payload } = next_event(session).await {
            return (seq, payload);
        }
    }
}

/// The phone pushes `text` and returns the desktop's answer. A push from
/// the desktop meanwhile fails the test: nothing is to go to the phone.
async fn push_from_phone(
    session: &mut Session<TlsStream<TcpStream>>,
    text: &str,
    sensitive: bool,
) -> Reply {
    let push = ClipboardPush {
        text: text.into(),
        sensitive,
    };
    let seq = session
        .start_request(Payload::ClipboardPush(push))
        .await
        .unwrap();
    reply_to(session, seq).await
}

/// The desktop's answer to the phone's request `seq`. A push from the
/// desktop meanwhile fails the test.
async fn reply_to(session: &mut Session<TlsStream<TcpStream>>, seq: u64) -> Reply {
    loop {
        match next_event(session).await {
            SessionEvent::Reply(reply) if reply.request == seq => return reply,
            SessionEvent::Request {
                payload: Payload::ClipboardPush(push),
                ..
            } => panic!("the desktop pushed {} bytes", push.text.len()),
            _ => {}
        }
    }
}

/// Waits for the desktop's `Goodbye`.
async fn goodbye(session: &mut Session<TlsStream<TcpStream>>) -> GoodbyeReason {
    loop {
        if let SessionEvent::Goodbye(goodbye) = next_event(session).await {
            return goodbye.reason;
        }
    }
}

/// The next push from the desktop, which the phone takes.
async fn take_push(session: &mut Session<TlsStream<TcpStream>>) -> ClipboardPush {
    loop {
        if let SessionEvent::Request {
            seq,
            payload: Payload::ClipboardPush(push),
        } = next_event(session).await
        {
            session.ack(seq).await.unwrap();
            return push;
        }
    }
}

/// Pairs the stand-in phone and runs one session, in which the desktop
/// confirms the record. The phone then ends it as a restarting phone does.
async fn pair_and_confirm(rig: &Rig, phone: &Arc<TestPhone>) -> CertificateFingerprint {
    let (desktop, _) = pair(rig, phone).await;
    let mut session = within(phone_session(phone, desktop)).await;
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
        // The page only asks for actions a notification offers.
        actions: vec![NotificationAction {
            id: "mark".into(),
            label: "Reply".into(),
            accepts_reply: true,
            ..NotificationAction::default()
        }],
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
        within(phone.forge_refusal()).await.unwrap();
    }
    let listed = rig.runtime.devices();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].status, DeviceStatus::Reconnecting);

    // The phone itself still gets a session.
    rig.runtime.reconnect("phone-3");
    within(phone_session(&phone, desktop)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_phone_that_refuses_the_key_after_the_handshake_removed_the_pairing() {
    let rig = start();
    let phone = Arc::new(TestPhone::bind(LOCALHOST, "phone-4").await);
    pair_and_confirm(&rig, &phone).await;

    // The phone forgot this desktop: its TLS stack refuses the key once the
    // handshake has checked the phone's.
    let stranger = Identity::generate().unwrap().fingerprint();
    assert!(
        within(phone.accept(PhoneTls::pinned(stranger)))
            .await
            .is_err()
    );
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
    assert!(
        within(phone.accept(PhoneTls::pinned(stranger)))
            .await
            .is_err()
    );
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
        let mut connection = within(side.accept(PhoneTls::sas_mode())).await.unwrap();
        within(connection.pair_by_sas(&side, false, SasDecision::Confirmed)).await
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
    within(phone_session(&phone, desktop)).await;
}

/// Another loopback address, so a test phone can stand at its own address.
fn loopback(last: u8) -> IpAddr {
    Ipv4Addr::new(127, 0, 0, last).into()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_attempt_leaves_the_code_to_the_phone_that_scanned_it() {
    use link2linux_core::pairing::PairingOutcome;
    use link2linux_runtime::view::PairingFailure;

    let rig = start();
    let payload = show_code(&rig).await;

    // A host at another address answers and sends a proof without the
    // secret.
    let impostor = TestPhone::bind(loopback(2), "impostor").await;
    rig.runtime
        .pair_with(PhoneTarget::Address {
            address: impostor.addr().to_string(),
        })
        .unwrap();
    let mut forged = within(impostor.accept(PhoneTls::sas_mode())).await.unwrap();
    within(forged.wire.hello(impostor.hello())).await.unwrap();
    let proof = PairingProof { mac: vec![0; 32] };
    forged
        .wire
        .send_payload(Payload::PairingProof(proof))
        .await
        .unwrap();
    within(forged.wire.finish()).await;

    // The same code stays up, with the reason that attempt failed.
    let (svg, reason) = rig
        .events
        .pairing_matching(|state| match state {
            PairingState::QrOffer {
                svg,
                last_failure: Some(reason),
                ..
            } => Some((svg.clone(), *reason)),
            _ => None,
        })
        .await;
    assert_eq!(reason, PairingFailure::VerificationFailed);
    let PairingState::QrOffer { svg: first, .. } = rig.events.pairing.lock().unwrap()[0].clone()
    else {
        panic!("the first state is the code");
    };
    assert_eq!(svg, first);

    // The phone that scanned it pairs with it.
    let phone = Arc::new(TestPhone::bind(LOCALHOST, "phone-7").await);
    let scanned = phone.clone();
    let phone_side = tokio::spawn(async move {
        let accepted = scanned.accept(PhoneTls::pinned(payload.desktop));
        let mut connection = within(accepted).await.unwrap();
        let report = within(connection.pair_by_qr(&scanned, &payload.secret)).await;
        within(connection.wire.finish()).await;
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
            _ => None,
        })
        .await;
    assert_eq!(device.id, "phone-7");
    let report = phone_side.await.unwrap();
    assert_eq!(report.desktop_outcome, Some(PairingOutcome::Accepted));
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
            PairingState::QrOffer {
                last_failure: Some(reason),
                ..
            } => Some(*reason),
            _ => None,
        })
        .await;
    assert_eq!(reason, PairingFailure::PhoneNotFound);
    assert!(rig.runtime.devices().is_empty());
    // The code stays up for the phone that scans it; cancelling ends it.
    assert!(matches!(
        rig.runtime.pairing(),
        PairingState::QrOffer { .. }
    ));
    rig.runtime.cancel_pairing();
    assert_eq!(rig.runtime.pairing(), PairingState::Idle);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_clipboard_goes_both_ways_and_never_comes_back_by_itself() {
    let clipboard = FakeClipboard::new(ClipboardAccess::Background);
    let rig = start_with(Some(clipboard.clone()));
    let phone = Arc::new(TestPhone::bind(LOCALHOST, "clip-1").await);
    let (desktop, _) = pair(&rig, &phone).await;
    let mut session = within(clipboard_session(&phone, desktop)).await;

    // From the phone: on the desktop clipboard with its mark, and acked.
    let reply = within(push_from_phone(&mut session, "from the phone", true)).await;
    assert!(matches!(reply.payload, Payload::Ack(_)), "{reply:?}");
    let written = clipboard.text().unwrap();
    assert_eq!(
        (written.text.as_str(), written.sensitive),
        ("from the phone", true)
    );
    // The desktop sees its own write on the clipboard, and keeps it.
    clipboard
        .wait_for(|clipboard| clipboard.reads.load(Ordering::Relaxed) >= 1)
        .await;

    // The user copies on the desktop: that goes to the phone, and it is
    // the first push the phone gets.
    clipboard.copy("from the desktop");
    let push = within(take_push(&mut session)).await;
    assert_eq!(
        (push.text.as_str(), push.sensitive),
        ("from the desktop", false)
    );

    // The page shows both, newest first, without the text.
    let status = rig
        .events
        .clipboard_where(|status| status.activity.len() >= 2)
        .await;
    assert_eq!(status.mode, ClipboardMode::Automatic);
    let activity: Vec<_> = status
        .activity
        .iter()
        .map(|entry| (entry.direction, entry.bytes, entry.phone_name.as_str()))
        .collect();
    assert_eq!(
        activity,
        [
            (ClipboardDirection::Sent, 16, "Phone clip-1"),
            (ClipboardDirection::Received, 14, "Phone clip-1")
        ]
    );
    assert_eq!(status.phones[0].state, ClipboardPhoneState::Ready);
    assert!(status.problem.is_none());
    assert!(!format!("{status:?}").contains("from the"));
    assert_eq!(rig.runtime.clipboard(), status);

    // Asked for, the same text goes again.
    rig.runtime.send_clipboard();
    let push = within(take_push(&mut session)).await;
    assert_eq!(push.text, "from the desktop");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_focus_only_desktop_offers_the_phones_clipboard_until_its_window_has_focus() {
    let clipboard = FakeClipboard::new(ClipboardAccess::FocusOnly);
    let rig = start_with(Some(clipboard.clone()));
    let phone = Arc::new(TestPhone::bind(LOCALHOST, "clip-2").await);
    let (desktop, _) = pair(&rig, &phone).await;
    let mut session = within(clipboard_session(&phone, desktop)).await;

    // In the background, the text waits, and a pop-up offers it.
    let reply = within(push_from_phone(&mut session, "waits", false)).await;
    assert!(matches!(reply.payload, Payload::Ack(_)), "{reply:?}");
    assert!(clipboard.text().is_none());
    let banners = rig.sink.wait_for_banners(1).await;
    let (id, banner) = &banners[0];
    assert_eq!(banner.summary, "Clipboard from Phone clip-2");
    assert!(!banner.body.contains("waits"), "{}", banner.body);
    let status = rig
        .events
        .clipboard_where(|status| status.held.is_some())
        .await;
    assert_eq!(status.mode, ClipboardMode::WhenFocused);
    assert_eq!(status.held.unwrap().bytes, 5);

    // The window gains focus: the text goes on the clipboard, and the
    // pop-up goes away.
    rig.runtime.set_active(true);
    clipboard
        .wait_for(|clipboard| clipboard.text().is_some_and(|text| text.text == "waits"))
        .await;
    wait(&rig.sink.changed, || {
        rig.sink.closed.lock().unwrap().contains(id).then_some(())
    })
    .await;
    rig.events
        .clipboard_where(|status| status.held.is_none() && !status.activity.is_empty())
        .await;

    // A copy made while the window was away goes when it gains focus.
    rig.runtime.set_active(false);
    clipboard.copy("copied meanwhile");
    rig.runtime.set_active(true);
    let push = within(take_push(&mut session)).await;
    assert_eq!(push.text, "copied meanwhile");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_clipboard_that_loses_its_display_server_is_shared_while_the_window_has_focus() {
    let clipboard = FakeClipboard::new(ClipboardAccess::Background);
    let rig = start_with(Some(clipboard.clone()));
    let phone = Arc::new(TestPhone::bind(LOCALHOST, "clip-3").await);
    let (desktop, _) = pair(&rig, &phone).await;
    let mut session = within(clipboard_session(&phone, desktop)).await;

    clipboard.copy("before");
    assert_eq!(within(take_push(&mut session)).await.text, "before");

    // The page says the clipboard now follows the window.
    clipboard.lose();
    rig.events
        .clipboard_where(|status| status.mode == ClipboardMode::WhenFocused)
        .await;

    // A copy made since goes when the window gains focus.
    clipboard.copy("after");
    rig.runtime.set_active(true);
    assert_eq!(within(take_push(&mut session)).await.text, "after");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_feature_failing_on_the_phones_feature_status_keeps_it_from_no_one() {
    let clipboard = FakeClipboard::new(ClipboardAccess::Background);
    let rig = start_full(Some(clipboard.clone()), Some(Arc::new(FailingDrivers)));
    let phone = Arc::new(TestPhone::bind(LOCALHOST, "clip-4").await);
    let (desktop, _) = pair(&rig, &phone).await;
    let session = within(clipboard_session(&phone, desktop)).await;

    // The clipboard learned that the phone takes it...
    rig.events
        .clipboard_where(|status| {
            status
                .phones
                .iter()
                .any(|phone| phone.state == ClipboardPhoneState::Ready)
        })
        .await;
    // ...and the loop confirmed the pairing: the phone refusing this
    // desktop's key now means it removed the pairing.
    session
        .goodbye(GoodbyeReason::Shutdown, "restarting")
        .await
        .unwrap();
    let stranger = Identity::generate().unwrap().fingerprint();
    assert!(
        within(phone.accept(PhoneTls::pinned(stranger)))
            .await
            .is_err()
    );
    rig.events
        .devices_where(|devices| devices.iter().any(|d| d.status == DeviceStatus::Revoked))
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn on_focus_the_phones_text_wins_and_what_the_clipboard_held_stays_here() {
    let clipboard = FakeClipboard::new(ClipboardAccess::FocusOnly);
    clipboard.copy("before");
    let rig = start_with(Some(clipboard.clone()));
    let phone = Arc::new(TestPhone::bind(LOCALHOST, "clip-5").await);
    let (desktop, _) = pair(&rig, &phone).await;
    let mut session = within(clipboard_session(&phone, desktop)).await;

    // The window had focus once, with "before" on the clipboard.
    rig.runtime.set_active(true);
    assert_eq!(within(take_push(&mut session)).await.text, "before");
    rig.runtime.set_active(false);

    // The user copies something in another app, then shares from the
    // phone.
    clipboard.copy("copied elsewhere");
    let reply = within(push_from_phone(&mut session, "from the phone", false)).await;
    assert!(matches!(reply.payload, Payload::Ack(_)), "{reply:?}");
    let (id, _) = rig.sink.wait_for_banners(1).await[0].clone();

    // The window gains focus: the phone's text goes on the clipboard, and
    // the pop-up goes.
    rig.runtime.set_active(true);
    clipboard
        .wait_for(|clipboard| {
            clipboard
                .text()
                .is_some_and(|text| text.text == "from the phone")
        })
        .await;
    wait(&rig.sink.changed, || {
        rig.sink.closed.lock().unwrap().contains(&id).then_some(())
    })
    .await;

    // What the clipboard held never went to the phone: the next push it
    // gets is the next copy.
    rig.runtime.set_active(false);
    clipboard.copy("later");
    rig.runtime.set_active(true);
    assert_eq!(within(take_push(&mut session)).await.text, "later");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_click_on_the_popup_puts_the_text_on_while_the_window_has_focus() {
    let clipboard = FakeClipboard::new(ClipboardAccess::FocusOnly);
    let rig = start_with(Some(clipboard.clone()));
    let phone = Arc::new(TestPhone::bind(LOCALHOST, "clip-11").await);
    let (desktop, _) = pair(&rig, &phone).await;
    let mut session = within(clipboard_session(&phone, desktop)).await;
    rig.runtime.set_active(true);
    clipboard
        .wait_for(|clipboard| clipboard.reads.load(Ordering::Relaxed) >= 1)
        .await;

    // The window has focus, but the phone's text cannot go on the clipboard
    // right now: it waits, and a pop-up offers it.
    clipboard.fail_writes.store(true, Ordering::Relaxed);
    let reply = within(push_from_phone(&mut session, "waits", false)).await;
    assert!(matches!(reply.payload, Payload::Ack(_)), "{reply:?}");
    let (id, _) = rig.sink.wait_for_banners(1).await[0].clone();

    clipboard.fail_writes.store(false, Ordering::Relaxed);
    rig.respond
        .send(NotificationResponse::Action {
            id,
            key: OPEN_KEY.into(),
        })
        .unwrap();
    clipboard
        .wait_for(|clipboard| clipboard.text().is_some_and(|text| text.text == "waits"))
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unpairing_a_phone_drops_its_text_that_waits() {
    let clipboard = FakeClipboard::new(ClipboardAccess::FocusOnly);
    let rig = start_with(Some(clipboard.clone()));
    let phone = Arc::new(TestPhone::bind(LOCALHOST, "clip-6").await);
    let (desktop, _) = pair(&rig, &phone).await;
    let mut session = within(clipboard_session(&phone, desktop)).await;

    let reply = within(push_from_phone(&mut session, "waits", false)).await;
    assert!(matches!(reply.payload, Payload::Ack(_)), "{reply:?}");
    let (id, _) = rig.sink.wait_for_banners(1).await[0].clone();

    rig.runtime.unpair("clip-6").unwrap();
    wait(&rig.sink.changed, || {
        rig.sink.closed.lock().unwrap().contains(&id).then_some(())
    })
    .await;
    rig.events
        .clipboard_where(|status| status.held.is_none() && status.phones.is_empty())
        .await;

    // Focus finds nothing to put on the clipboard.
    rig.runtime.set_active(true);
    clipboard
        .wait_for(|clipboard| clipboard.reads.load(Ordering::Relaxed) >= 1)
        .await;
    assert!(clipboard.text().is_none());
}

/// The phone side of a session with the paired desktop, for a phone that
/// sends and receives files, once both sides report files ready.
async fn files_session(
    phone: &TestPhone,
    desktop: CertificateFingerprint,
) -> Session<TlsStream<TcpStream>> {
    let connection = phone.accept(PhoneTls::pinned(desktop)).await.unwrap();
    let info = DeviceInfo::new(
        phone.id.clone(),
        phone.name.clone(),
        DeviceKind::Phone,
        [Feature::Files],
    );
    let mut session =
        Session::handshake(connection.wire.into_inner(), info, SessionConfig::phone())
            .await
            .unwrap();
    assert!(session.peer().features.contains(&Feature::Files));
    while !session.is_ready(Feature::Files) {
        next_event(&mut session).await;
    }
    session
}

/// When the stand-in phone stops on its own, after the bytes of a
/// transfer passed a mark.
#[derive(Clone, Copy)]
enum Stop {
    /// It drops the connection once it received this many bytes.
    DropAfterReceiving(u64),
    /// It cancels what it sends once this many bytes went.
    CancelAfterSending(u64),
    /// It drops the connection once it sent this many bytes.
    DropAfterSending(u64),
}

/// The stand-in phone's side of files: the core's own service on its
/// session, run until the session ends or the phone stops.
struct PhoneFiles {
    sends: mpsc::UnboundedSender<(Vec<PathBuf>, oneshot::Sender<TransferId>)>,
    events: broadcast::Receiver<TransferEvent>,
    task: JoinHandle<FileService>,
}

impl PhoneFiles {
    fn start(
        mut session: Session<TlsStream<TcpStream>>,
        mut service: FileService,
        stop: Option<Stop>,
    ) -> Self {
        let events = service.subscribe();
        let (sends, mut inbox) =
            mpsc::unbounded_channel::<(Vec<PathBuf>, oneshot::Sender<TransferId>)>();
        let task = tokio::spawn(async move {
            service.session_started(&mut session).await.unwrap();
            loop {
                tokio::select! {
                    send = inbox.recv() => {
                        let Some((paths, sent)) = send else { break };
                        let _ = sent.send(service.send(paths).await.unwrap());
                        if service.run(&mut session).await.is_err() {
                            break;
                        }
                    }
                    event = session.next_event() => match event {
                        Ok(Some(event)) => {
                            if service.handle_event(&mut session, event).await.is_err() {
                                break;
                            }
                        }
                        Ok(None) | Err(_) => break,
                    },
                }
                let passed = |direction: Direction, mark: u64| {
                    service.transfers().into_iter().find(|t| {
                        t.direction == direction && t.done >= mark && !t.state.is_finished()
                    })
                };
                match stop {
                    Some(Stop::DropAfterReceiving(mark)) => {
                        if passed(Direction::Incoming, mark).is_some() {
                            break;
                        }
                    }
                    Some(Stop::DropAfterSending(mark)) => {
                        if passed(Direction::Outgoing, mark).is_some() {
                            break;
                        }
                    }
                    Some(Stop::CancelAfterSending(mark)) => {
                        if let Some(transfer) = passed(Direction::Outgoing, mark) {
                            service.cancel(transfer.id).await;
                            let _ = service.run(&mut session).await;
                        }
                    }
                    None => {}
                }
            }
            service.session_ended();
            service
        });
        Self {
            sends,
            events,
            task,
        }
    }

    async fn send(&self, paths: Vec<PathBuf>) -> TransferId {
        let (sent, id) = oneshot::channel();
        self.sends.send((paths, sent)).unwrap();
        id.await.unwrap()
    }

    /// Waits for a change of a transfer to a state `check` accepts.
    async fn reaches(&mut self, check: impl Fn(&Transfer) -> bool) -> Transfer {
        within(async {
            loop {
                match self.events.recv().await {
                    Ok(TransferEvent::Changed(transfer)) if check(&transfer) => return transfer,
                    Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => panic!("the phone stopped"),
                }
            }
        })
        .await
    }
}

impl Events {
    /// Waits for the page to hear that a transfer with phone `device_id`
    /// changed to a state `check` accepts.
    async fn transfer_where(
        &self,
        device_id: &str,
        check: impl Fn(&FileTransfer) -> bool,
    ) -> FileTransfer {
        wait(&self.changed, || {
            self.transfers
                .lock()
                .unwrap()
                .iter()
                .find_map(|(device, event)| match event {
                    FileTransferEvent::Changed { transfer }
                        if device == device_id && check(transfer) =>
                    {
                        Some(transfer.clone())
                    }
                    _ => None,
                })
        })
        .await
    }
}

impl FakeFiles {
    async fn opened(&self, what: &'static str, path: &Path) {
        wait(&self.changed, || {
            self.opened
                .lock()
                .unwrap()
                .iter()
                .any(|(done, opened)| *done == what && opened == path)
                .then_some(())
        })
        .await;
    }
}

/// Bytes that differ from one position to the next.
fn file_data(len: usize, seed: u8) -> Vec<u8> {
    (0..len)
        .map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed) ^ (i >> 8) as u8)
        .collect()
}

fn completed(direction: TransferDirection) -> impl Fn(&FileTransfer) -> bool {
    move |t| t.direction == direction && t.status == TransferStatus::Completed
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn files_go_both_ways_and_arrive_with_a_banner_that_opens_them() {
    let rig = start();
    let phone = Arc::new(TestPhone::bind(LOCALHOST, "files-1").await);
    let desktop = pair_and_confirm(&rig, &phone).await;
    let on_phone = tempfile::tempdir().unwrap();
    let sources = tempfile::tempdir().unwrap();
    let session = within(files_session(&phone, desktop)).await;
    let mut phone_files =
        PhoneFiles::start(session, FileService::new(on_phone.path().into()), None);

    // To the phone, and from it at the same time.
    let report = file_data(600 * 1024, 1);
    let photo = file_data(300 * 1024, 2);
    std::fs::write(sources.path().join("report.pdf"), &report).unwrap();
    std::fs::write(sources.path().join("photo.jpg"), &photo).unwrap();
    rig.runtime
        .send_files("files-1", vec![sources.path().join("report.pdf")])
        .unwrap();
    phone_files
        .send(vec![sources.path().join("photo.jpg")])
        .await;

    phone_files
        .reaches(|t| t.direction == Direction::Incoming && t.state == TransferState::Completed)
        .await;
    assert_eq!(
        std::fs::read(on_phone.path().join("report.pdf")).unwrap(),
        report
    );
    rig.events
        .transfer_where("files-1", completed(TransferDirection::Outgoing))
        .await;
    let received = rig
        .events
        .transfer_where("files-1", completed(TransferDirection::Incoming))
        .await;
    assert_eq!(received.files[0].name, "photo.jpg");
    assert!(received.files[0].saved);
    let saved = rig.files.downloads.join("photo.jpg");
    assert_eq!(std::fs::read(&saved).unwrap(), photo);

    // A banner says so, without the file's name, and shows it in its folder.
    let banners = rig.sink.wait_for_banners(1).await;
    let (id, banner) = banners
        .iter()
        .find(|(_, banner)| banner.summary == "Received a file from Phone files-1")
        .expect("a banner for the file")
        .clone();
    assert!(!format!("{banner:?}").contains("photo.jpg"), "{banner:?}");
    let keys: Vec<&str> = banner.actions.iter().map(|a| a.key.as_str()).collect();
    // A click on a banner never opens a file, as the banner names none.
    assert_eq!(keys, ["files-show"]);
    rig.respond
        .send(NotificationResponse::Action {
            id,
            key: "files-show".into(),
        })
        .unwrap();
    rig.files.opened("show", &saved).await;

    // The page lists both, and opens the received file by its place.
    let listed = rig.runtime.transfers("files-1").unwrap();
    assert_eq!(listed.len(), 2);
    let incoming = listed
        .iter()
        .find(|t| t.direction == TransferDirection::Incoming)
        .unwrap();
    rig.runtime
        .open_received_file("files-1", &incoming.id, 0)
        .unwrap();
    rig.files.opened("open", &saved).await;
    assert_eq!(
        rig.runtime.open_received_file("files-1", &incoming.id, 1),
        Err(link2linux_runtime::view::BackendError::NotFound)
    );

    // Folders are refused at once, and nothing goes out.
    assert_eq!(
        rig.runtime
            .send_files("files-1", vec![sources.path().to_owned()]),
        Err(link2linux_runtime::view::BackendError::FolderNotSendable)
    );
    drop(phone_files);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_transfer_goes_on_after_the_connection_drops() {
    let rig = start();
    let phone = Arc::new(TestPhone::bind(LOCALHOST, "files-2").await);
    let desktop = pair_and_confirm(&rig, &phone).await;
    let on_phone = tempfile::tempdir().unwrap();
    let sources = tempfile::tempdir().unwrap();
    let big = file_data(3 * 1024 * 1024, 3);
    std::fs::write(sources.path().join("big.bin"), &big).unwrap();

    // The phone drops the connection once a megabyte is in.
    let session = within(files_session(&phone, desktop)).await;
    let first = PhoneFiles::start(
        session,
        FileService::new(on_phone.path().into()),
        Some(Stop::DropAfterReceiving(1024 * 1024)),
    );
    rig.runtime
        .send_files("files-2", vec![sources.path().join("big.bin")])
        .unwrap();
    let service = within(first.task).await.unwrap();
    let partial = service.transfers()[0].clone();
    assert_eq!(partial.state, TransferState::Interrupted);
    assert!(partial.done >= 1024 * 1024 && partial.done < big.len() as u64);
    rig.events
        .transfer_where("files-2", |t| t.status == TransferStatus::Interrupted)
        .await;

    // The desktop comes back, offers it again, and goes on from there.
    let session = within(files_session(&phone, desktop)).await;
    let mut second = PhoneFiles::start(session, service, None);
    let resumed = second.reaches(|t| t.state == TransferState::Running).await;
    assert!(resumed.done >= partial.done, "{resumed:?}");
    second
        .reaches(|t| t.state == TransferState::Completed)
        .await;
    assert_eq!(std::fs::read(on_phone.path().join("big.bin")).unwrap(), big);
    rig.events
        .transfer_where("files-2", completed(TransferDirection::Outgoing))
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_transfer_the_phone_cancels_leaves_nothing_behind() {
    let rig = start();
    let phone = Arc::new(TestPhone::bind(LOCALHOST, "files-3").await);
    let desktop = pair_and_confirm(&rig, &phone).await;
    let sources = tempfile::tempdir().unwrap();
    let big = file_data(3 * 1024 * 1024, 4);
    std::fs::write(sources.path().join("video.mp4"), &big).unwrap();
    let session = within(files_session(&phone, desktop)).await;
    let phone_dir = tempfile::tempdir().unwrap();
    let mut phone_files = PhoneFiles::start(
        session,
        FileService::new(phone_dir.path().into()),
        Some(Stop::CancelAfterSending(512 * 1024)),
    );

    phone_files
        .send(vec![sources.path().join("video.mp4")])
        .await;
    phone_files
        .reaches(|t| t.state == TransferState::Cancelled { by_peer: false })
        .await;
    rig.events
        .transfer_where("files-3", |t| {
            t.status == TransferStatus::Cancelled { by_phone: true }
        })
        .await;
    let banners = rig.sink.wait_for_banners(1).await;
    assert!(
        banners
            .iter()
            .any(|(_, b)| b.summary == "Phone files-3 stopped sending files"),
        "{banners:?}"
    );
    // The partial file is gone: nothing at all is in Downloads.
    let left: Vec<_> = std::fs::read_dir(&rig.files.downloads)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert!(left.is_empty(), "{left:?}");
}

/// SHA-256 of nothing, and of "abc".
const EMPTY_SHA256: [u8; 32] = [
    0xe3, 0xb0, 0xc4, 0x42, 0x98, 0xfc, 0x1c, 0x14, 0x9a, 0xfb, 0xf4, 0xc8, 0x99, 0x6f, 0xb9, 0x24,
    0x27, 0xae, 0x41, 0xe4, 0x64, 0x9b, 0x93, 0x4c, 0xa4, 0x95, 0x99, 0x1b, 0x78, 0x52, 0xb8, 0x55,
];
const ABC_SHA256: [u8; 32] = [
    0xba, 0x78, 0x16, 0xbf, 0x8f, 0x01, 0xcf, 0xea, 0x41, 0x41, 0x40, 0xde, 0x5d, 0xae, 0x22, 0x23,
    0xb0, 0x03, 0x61, 0xa3, 0x96, 0x17, 0x7a, 0x9c, 0xb4, 0x10, 0xff, 0x61, 0xf2, 0x00, 0x15, 0xad,
];

/// Offers `files` (name, size) as transfer `id` and returns the answer.
async fn offer(
    session: &mut Session<TlsStream<TcpStream>>,
    id: u8,
    files: &[(&str, u64)],
) -> Result<Reply, link2linux_core::session::RequestError> {
    let offer = FileOffer {
        transfer_id: vec![id; 16],
        created_unix_ms: 1_700_000_000_000,
        files: files
            .iter()
            .map(|(name, size)| FileInfo {
                name: (*name).into(),
                size: *size,
                ..FileInfo::default()
            })
            .collect(),
    };
    within(session.send_request(Payload::FileOffer(offer))).await
}

async fn chunk(
    session: &mut Session<TlsStream<TcpStream>>,
    id: u8,
    index: u32,
    data: &[u8],
    sha256: [u8; 32],
) -> Result<Reply, link2linux_core::session::RequestError> {
    let chunk = FileChunk {
        transfer_id: vec![id; 16],
        file_index: index,
        offset: 0,
        data: data.to_vec(),
        sha256: sha256.to_vec(),
    };
    within(session.send_request(Payload::FileChunk(chunk))).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn received_names_are_made_safe_and_damaged_files_are_refused() {
    let rig = start();
    let phone = Arc::new(TestPhone::bind(LOCALHOST, "files-4").await);
    let desktop = pair_and_confirm(&rig, &phone).await;
    let downloads = rig.files.downloads.clone();
    std::fs::create_dir_all(&downloads).unwrap();
    std::fs::write(downloads.join("photo.jpg"), b"mine").unwrap();
    let mut session = within(files_session(&phone, desktop)).await;
    let accept = |reply: Reply| match reply.payload {
        Payload::FileAccept(FileAccept { file_index, offset }) => (file_index, offset),
        other => panic!("expected FileAccept, got {other:?}"),
    };

    // A name that climbs out of Downloads stays in it, and a damaged file
    // is refused.
    let files = [("../../.evil.sh", 0), ("notes.txt", 3)];
    assert_eq!(
        accept(offer(&mut session, 1, &files).await.unwrap()),
        (0, 0)
    );
    assert!(chunk(&mut session, 1, 0, b"", EMPTY_SHA256).await.is_ok());
    let damaged = chunk(&mut session, 1, 1, b"abd", ABC_SHA256).await;
    match damaged {
        Err(link2linux_core::session::RequestError::Refused(error)) => {
            assert_eq!(error.code(), ErrorCode::InvalidArgument);
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
    rig.events
        .transfer_where("files-4", |t| {
            matches!(t.status, TransferStatus::Failed { .. })
        })
        .await;

    // A name that is taken gets a number.
    assert_eq!(
        accept(offer(&mut session, 2, &[("photo.jpg", 3)]).await.unwrap()),
        (0, 0)
    );
    assert!(chunk(&mut session, 2, 0, b"abc", ABC_SHA256).await.is_ok());
    rig.events
        .transfer_where("files-4", completed(TransferDirection::Incoming))
        .await;

    let mut names: Vec<String> = std::fs::read_dir(&downloads)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names, ["_.._.evil.sh", "photo (1).jpg", "photo.jpg"]);
    assert_eq!(std::fs::read(downloads.join("photo.jpg")).unwrap(), b"mine");
    assert_eq!(
        std::fs::read(downloads.join("photo (1).jpg")).unwrap(),
        b"abc"
    );
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(downloads.join("_.._.evil.sh"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o111, 0, "{mode:o}");
    let banners = rig.sink.wait_for_banners(2).await;
    assert!(
        banners.iter().any(
            |(_, b)| b.summary == "Couldn't receive files from Phone files-4"
                && b.body == "A file arrived damaged, so it was deleted."
        ),
        "{banners:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_phones_text_written_while_the_desktop_reads_is_not_mistaken_for_a_copy() {
    let clipboard = FakeClipboard::new(ClipboardAccess::Background);
    let rig = start_with(Some(clipboard.clone()));
    let phone = Arc::new(TestPhone::bind(LOCALHOST, "clip-7").await);
    let (desktop, _) = pair(&rig, &phone).await;
    let mut session = within(clipboard_session(&phone, desktop)).await;

    // The desktop starts reading a copy, slowly...
    clipboard.stall_next_read.store(true, Ordering::Relaxed);
    clipboard.copy("copied here");
    within(clipboard.read_started.notified()).await;
    // ...while the phone's text goes on the clipboard.
    let reply = within(push_from_phone(&mut session, "from the phone", false)).await;
    assert!(matches!(reply.payload, Payload::Ack(_)), "{reply:?}");
    clipboard.release_read.notify_one();

    // The read was overtaken: neither it nor the phone's text goes to the
    // phone, and the next push is the next copy.
    clipboard
        .wait_for(|clipboard| clipboard.reads.load(Ordering::Relaxed) >= 2)
        .await;
    clipboard.copy("later");
    assert_eq!(within(take_push(&mut session)).await.text, "later");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_copy_read_while_a_phones_text_fails_to_go_on_is_still_shared() {
    let clipboard = FakeClipboard::new(ClipboardAccess::Background);
    let rig = start_with(Some(clipboard.clone()));
    let phone = Arc::new(TestPhone::bind(LOCALHOST, "clip-8").await);
    let (desktop, _) = pair(&rig, &phone).await;
    let mut session = within(clipboard_session(&phone, desktop)).await;

    // The desktop starts reading the user's copy, slowly, and the phone's
    // text cannot go on the clipboard meanwhile.
    clipboard.stall_next_read.store(true, Ordering::Relaxed);
    clipboard.copy("just copied");
    within(clipboard.read_started.notified()).await;
    clipboard.fail_writes.store(true, Ordering::Relaxed);
    let reply = within(push_from_phone(&mut session, "from the phone", false)).await;
    assert!(
        matches!(&reply.payload, Payload::ErrorReply(error) if error.code() == ErrorCode::Unavailable),
        "{reply:?}"
    );
    clipboard.release_read.notify_one();

    // The copy is read again and shared.
    assert_eq!(within(take_push(&mut session)).await.text, "just copied");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unpairing_while_a_push_waits_for_its_answer_names_no_unknown_phone() {
    let clipboard = FakeClipboard::new(ClipboardAccess::Background);
    let rig = start_with(Some(clipboard.clone()));
    let phone = Arc::new(TestPhone::bind(LOCALHOST, "clip-9").await);
    let (desktop, _) = pair(&rig, &phone).await;
    let mut session = within(clipboard_session(&phone, desktop)).await;

    // The phone never answers this push, and the user unpairs it.
    clipboard.copy("unanswered");
    loop {
        if let SessionEvent::Request {
            payload: Payload::ClipboardPush(_),
            ..
        } = next_event(&mut session).await
        {
            break;
        }
    }
    rig.runtime.unpair("clip-9").unwrap();
    assert_eq!(within(goodbye(&mut session)).await, GoodbyeReason::Unpaired);

    // A later change reaches the page after the failed push did.
    clipboard.set(ClipboardText {
        text: "hunter2".into(),
        sensitive: true,
    });
    rig.events
        .clipboard_where(|status| {
            status
                .problem
                .as_ref()
                .is_some_and(|problem| problem.kind == ClipboardProblemKind::PasswordNotShared)
        })
        .await;
    let statuses = rig.events.clipboard.lock().unwrap().clone();
    assert!(
        statuses.iter().all(|status| status
            .problem
            .as_ref()
            .is_none_or(|problem| problem.device_id.as_deref() != Some("clip-9"))),
        "{statuses:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_push_taken_after_its_phone_was_unpaired_is_not_offered() {
    let clipboard = FakeClipboard::new(ClipboardAccess::FocusOnly);
    let rig = start_with(Some(clipboard.clone()));
    let phone = Arc::new(TestPhone::bind(LOCALHOST, "clip-10").await);
    let (desktop, _) = pair(&rig, &phone).await;
    let mut session = within(clipboard_session(&phone, desktop)).await;

    // The phone pushes as the user unpairs it: the desktop may take the
    // push before or after the unpairing.
    let push = ClipboardPush {
        text: "late".into(),
        sensitive: false,
    };
    session
        .start_request(Payload::ClipboardPush(push))
        .await
        .unwrap();
    rig.runtime.unpair("clip-10").unwrap();
    assert_eq!(within(goodbye(&mut session)).await, GoodbyeReason::Unpaired);

    // Nothing waits, and focus puts nothing on the clipboard.
    rig.events
        .clipboard_where(|status| status.phones.is_empty() && status.held.is_none())
        .await;
    rig.runtime.set_active(true);
    clipboard
        .wait_for(|clipboard| clipboard.reads.load(Ordering::Relaxed) >= 1)
        .await;
    assert!(clipboard.text().is_none());
    // No pop-up names the phone by its raw id, and any pop-up went.
    let shown = rig.sink.shown.lock().unwrap().clone();
    assert!(
        shown
            .iter()
            .all(|(_, popup)| popup.summary != "Clipboard from clip-10"),
        "{shown:?}"
    );
    let closed = rig.sink.closed.lock().unwrap().clone();
    assert!(shown.iter().all(|(id, _)| closed.contains(id)), "{shown:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_read_during_a_phones_write_neither_echoes_nor_relays_the_phones_text() {
    let clipboard = FakeClipboard::new(ClipboardAccess::Background);
    let rig = start_with(Some(clipboard.clone()));
    let sender = Arc::new(TestPhone::bind(LOCALHOST, "clip-12").await);
    let (desktop, _) = pair(&rig, &sender).await;
    let mut sending = within(clipboard_session(&sender, desktop)).await;
    // The pairing helpers look through every state so far: start afresh.
    rig.events.pairing.lock().unwrap().clear();
    let other = Arc::new(TestPhone::bind(LOCALHOST, "clip-13").await);
    let (desktop, _) = pair(&rig, &other).await;
    let mut watching = within(clipboard_session(&other, desktop)).await;

    // The sender's text is being written, slowly, when the user copies
    // something; the phone's text then replaces it.
    clipboard.stall_next_write.store(true, Ordering::Relaxed);
    let push = ClipboardPush {
        text: "from the phone".into(),
        sensitive: false,
    };
    let seq = sending
        .start_request(Payload::ClipboardPush(push))
        .await
        .unwrap();
    within(clipboard.write_started.notified()).await;
    clipboard.copy("copied here");
    // Time for a read that does not wait for the write to run.
    tokio::time::sleep(Duration::from_millis(200)).await;
    clipboard.release_write.notify_one();
    let reply = within(reply_to(&mut sending, seq)).await;
    assert!(matches!(reply.payload, Payload::Ack(_)), "{reply:?}");

    // The phone's text is not relayed to the other phone, nor is what the
    // read saw: the next push either phone gets is the next copy.
    clipboard
        .wait_for(|clipboard| clipboard.reads.load(Ordering::Relaxed) >= 1)
        .await;
    clipboard.copy("later");
    assert_eq!(within(take_push(&mut watching)).await.text, "later");
    assert_eq!(within(take_push(&mut sending)).await.text, "later");
}

/// The partial files the desktop keeps in its Downloads directory.
fn partials(rig: &Rig) -> usize {
    std::fs::read_dir(&rig.files.downloads)
        .map(|entries| {
            entries
                .filter(|entry| {
                    entry
                        .as_ref()
                        .is_ok_and(|e| e.file_name().to_string_lossy().starts_with(".link2linux-"))
                })
                .count()
        })
        .unwrap_or(0)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unpairing_a_phone_deletes_what_it_had_not_finished_sending() {
    let rig = start();
    let phone = Arc::new(TestPhone::bind(LOCALHOST, "files-5").await);
    let desktop = pair_and_confirm(&rig, &phone).await;
    let sources = tempfile::tempdir().unwrap();
    std::fs::write(
        sources.path().join("big.bin"),
        file_data(3 * 1024 * 1024, 5),
    )
    .unwrap();
    let session = within(files_session(&phone, desktop)).await;
    let on_phone = tempfile::tempdir().unwrap();
    let phone_files = PhoneFiles::start(
        session,
        FileService::new(on_phone.path().into()),
        Some(Stop::DropAfterSending(512 * 1024)),
    );
    phone_files.send(vec![sources.path().join("big.bin")]).await;
    within(phone_files.task).await.unwrap();
    rig.events
        .transfer_where("files-5", |t| t.status == TransferStatus::Interrupted)
        .await;
    assert_eq!(partials(&rig), 1);

    rig.runtime.unpair("files-5").unwrap();
    within(async {
        while partials(&rig) > 0 {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn closing_the_app_deletes_what_a_phone_had_not_finished_sending() {
    let rig = start();
    let phone = Arc::new(TestPhone::bind(LOCALHOST, "files-6").await);
    let desktop = pair_and_confirm(&rig, &phone).await;
    let sources = tempfile::tempdir().unwrap();
    std::fs::write(
        sources.path().join("big.bin"),
        file_data(3 * 1024 * 1024, 6),
    )
    .unwrap();
    let session = within(files_session(&phone, desktop)).await;
    let on_phone = tempfile::tempdir().unwrap();
    let phone_files = PhoneFiles::start(
        session,
        FileService::new(on_phone.path().into()),
        Some(Stop::DropAfterSending(512 * 1024)),
    );
    phone_files.send(vec![sources.path().join("big.bin")]).await;
    within(phone_files.task).await.unwrap();
    rig.events
        .transfer_where("files-6", |t| t.status == TransferStatus::Interrupted)
        .await;
    assert_eq!(partials(&rig), 1);

    rig.runtime.shutdown();
    within(async {
        while partials(&rig) > 0 {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_phone_shows_its_status_and_rings_when_asked() {
    let rig = start();
    let phone = Arc::new(TestPhone::bind(LOCALHOST, "status-1").await);
    let (desktop, device) = pair(&rig, &phone).await;
    let id = device.id.clone();
    let mut session = within(status_session(&phone, desktop)).await;

    // The desktop asks for the status, and shows the answer.
    let (asked, request) = within(next_request(&mut session)).await;
    assert!(
        matches!(request, Payload::PhoneStatusRequest(_)),
        "{request:?}"
    );
    let status = link2linux_protocol::v1::PhoneStatus {
        battery: Some(link2linux_protocol::v1::Battery {
            percent: 57,
            plugged_in: true,
            power_saver: false,
        }),
        wifi: Some(link2linux_protocol::v1::Wifi { level: 3 }),
        ..Default::default()
    };
    session
        .reply(
            asked,
            Payload::PhoneStatusReply(PhoneStatusReply {
                status: Some(status.clone()),
            }),
        )
        .await
        .unwrap();
    let shown = rig
        .events
        .status_where(&id, |status| matches!(status, PhoneStatus::Current { .. }))
        .await;
    let PhoneStatus::Current { battery, wifi, .. } = &shown else {
        unreachable!();
    };
    assert_eq!(battery.map(|battery| battery.percent), Some(Some(57)));
    assert_eq!(wifi.map(|wifi| wifi.level), Some(Some(3)));
    assert_eq!(rig.runtime.phone_status(&id).unwrap(), shown);

    // A change replaces it.
    let mut lower = status;
    lower.battery.as_mut().unwrap().percent = 56;
    session
        .send(Payload::PhoneStatusChanged(PhoneStatusChanged {
            status: Some(lower),
        }))
        .await
        .unwrap();
    rig.events
        .status_where(&id, |status| {
            matches!(status, PhoneStatus::Current { battery: Some(battery), .. }
                if battery.percent == Some(56))
        })
        .await;

    // The phone rings from its Ack until it says it stopped.
    rig.runtime.ring_phone(&id).unwrap();
    let (ring, request) = within(next_request(&mut session)).await;
    assert!(matches!(request, Payload::RingRequest(_)), "{request:?}");
    session.ack(ring).await.unwrap();
    rig.events
        .ring_where(&id, |ring| ring.phase == RingPhase::Ringing)
        .await;
    session
        .send(Payload::RingEnded(RingEnded {
            reason: RingEndReason::StoppedOnPhone.into(),
        }))
        .await
        .unwrap();
    let ended = rig
        .events
        .ring_where(&id, |ring| ring.phase == RingPhase::Idle)
        .await;
    assert_eq!(ended.ended, Some(RingEnd::StoppedOnPhone));
    assert_eq!(rig.runtime.ring_status(&id).unwrap(), ended);

    // Hidden here: the phone hears the status is off on this desktop.
    rig.runtime.set_phone_status_shown(false);
    rig.events
        .status_where(&id, |status| *status == PhoneStatus::Hidden)
        .await;
    while session
        .peer_availability()
        .and_then(|states| states.get(&Feature::Status))
        != Some(&Availability::Disabled)
    {
        next_event(&mut session).await;
    }

    // Once the session is over, nothing is current and nothing rings.
    session
        .goodbye(GoodbyeReason::Shutdown, "restarting")
        .await
        .unwrap();
    rig.events
        .status_where(&id, |status| *status == PhoneStatus::Disconnected)
        .await;
    let ring = rig
        .events
        .ring_where(&id, |ring| ring.blocked == Some(RingProblem::NotConnected))
        .await;
    assert_eq!(ring.phase, RingPhase::Idle);
    rig.runtime.ring_phone(&id).unwrap();
    rig.events
        .ring_where(&id, |ring| ring.problem == Some(RingProblem::NotConnected))
        .await;
    assert_eq!(
        rig.runtime.phone_status("nobody"),
        Err(link2linux_runtime::view::BackendError::NotFound)
    );
}

/// The phone's Bluetooth address in the calls tests.
const PHONE_BT: &str = "0C:1A:2B:3C:4D:5E";
const GATEWAY: &str = "/org/pipewire/Telephony/ag0";

/// What the desktop's Bluetooth and telephony hold, in memory.
#[derive(Default)]
struct FakeCallState {
    devices: Vec<BluetoothDevice>,
    gateways: Vec<Gateway>,
    open: bool,
    /// What the desktop asked of them, in order.
    log: Vec<String>,
}

/// The desktop's Bluetooth and telephony, in memory, which the tests
/// script: devices pair, gateways connect and calls ring.
struct FakeCalls {
    state: Mutex<FakeCallState>,
    bluetooth: watch::Sender<u64>,
    bluetooth_seen: tokio::sync::Mutex<watch::Receiver<u64>>,
    telephony: watch::Sender<u64>,
    telephony_seen: tokio::sync::Mutex<watch::Receiver<u64>>,
    prompts: mpsc::UnboundedSender<PairingEvent>,
    prompts_seen: tokio::sync::Mutex<mpsc::UnboundedReceiver<PairingEvent>>,
    changed: Notify,
}

impl FakeCalls {
    fn new() -> Arc<Self> {
        let (bluetooth, bluetooth_seen) = watch::channel(0);
        let (telephony, telephony_seen) = watch::channel(0);
        let (prompts, prompts_seen) = mpsc::unbounded_channel();
        Arc::new(Self {
            state: Mutex::default(),
            bluetooth,
            bluetooth_seen: tokio::sync::Mutex::new(bluetooth_seen),
            telephony,
            telephony_seen: tokio::sync::Mutex::new(telephony_seen),
            prompts,
            prompts_seen: tokio::sync::Mutex::new(prompts_seen),
            changed: Notify::new(),
        })
    }

    fn adapter() -> BluetoothAdapter {
        BluetoothAdapter {
            address: "A0:B1:C2:D3:E4:F5".parse().unwrap(),
            name: "Desk BT".into(),
            powered: true,
        }
    }

    fn record(&self, entry: String) {
        self.state.lock().unwrap().log.push(entry);
        self.changed.notify_waiters();
    }

    async fn wait_for(&self, entry: &str) {
        wait(&self.changed, || {
            self.state
                .lock()
                .unwrap()
                .log
                .iter()
                .any(|e| e == entry)
                .then_some(())
        })
        .await;
    }

    /// A device asks to pair and shows `code`.
    fn prompt(&self, id: u64, code: u32) {
        self.prompts
            .send(PairingEvent::Confirm(PairingPrompt {
                id,
                device: PHONE_BT.parse().unwrap(),
                device_name: "Pixel".into(),
                code,
            }))
            .unwrap();
    }

    /// The phone named `name` is now paired.
    fn pair_device(&self, name: &str) {
        self.state.lock().unwrap().devices.push(BluetoothDevice {
            address: PHONE_BT.parse().unwrap(),
            name: name.into(),
            paired: true,
            trusted: false,
            connected: true,
            phone: true,
            hands_free_gateway: true,
        });
        self.bluetooth.send_modify(|v| *v += 1);
    }

    /// A call to `number` rings on the phone's gateway.
    fn ring(&self, number: &str) {
        let mut state = self.state.lock().unwrap();
        state.gateways[0].calls.push(GatewayCall {
            id: format!("{GATEWAY}/call0"),
            number: number.into(),
            name: String::new(),
            state: CallState::Incoming,
            multiparty: false,
        });
        drop(state);
        self.telephony.send_modify(|v| *v += 1);
    }

    fn change_calls(&self, change: impl FnOnce(&mut Vec<GatewayCall>)) {
        if let Some(gateway) = self.state.lock().unwrap().gateways.first_mut() {
            change(&mut gateway.calls);
        }
        self.telephony.send_modify(|v| *v += 1);
    }
}

impl CallsPort for FakeCalls {
    fn adapter(&self) -> BoxFuture<'_, Result<Option<BluetoothAdapter>, PlatformError>> {
        Box::pin(async { Ok(Some(Self::adapter())) })
    }

    fn devices(&self) -> BoxFuture<'_, Result<Vec<BluetoothDevice>, PlatformError>> {
        Box::pin(async { Ok(self.state.lock().unwrap().devices.clone()) })
    }

    fn bluetooth_changed(&self) -> BoxFuture<'_, Result<(), PlatformError>> {
        Box::pin(async {
            let mut seen = self.bluetooth_seen.lock().await;
            seen.changed()
                .await
                .map_err(|_| PlatformError::Unsupported("Bluetooth"))
        })
    }

    fn open_pairing(
        &self,
        _window: Duration,
    ) -> BoxFuture<'_, Result<BluetoothAdapter, PlatformError>> {
        Box::pin(async {
            self.state.lock().unwrap().open = true;
            self.record("open".into());
            Ok(Self::adapter())
        })
    }

    fn close_pairing(&self) -> BoxFuture<'_, Result<(), PlatformError>> {
        Box::pin(async {
            let was_open = std::mem::take(&mut self.state.lock().unwrap().open);
            if was_open {
                self.record("close".into());
            }
            Ok(())
        })
    }

    fn next_pairing_event(&self) -> BoxFuture<'_, PairingEvent> {
        Box::pin(async {
            let mut prompts = self.prompts_seen.lock().await;
            match prompts.recv().await {
                Some(event) => event,
                None => std::future::pending().await,
            }
        })
    }

    fn answer_pairing(&self, id: u64, accept: bool) {
        self.record(format!("answer pairing {id} {accept}"));
    }

    fn connect_hands_free(
        &self,
        device: BluetoothAddress,
    ) -> BoxFuture<'_, Result<(), PlatformError>> {
        Box::pin(async move {
            self.record(format!("connect {device}"));
            self.state.lock().unwrap().gateways = vec![Gateway {
                id: GATEWAY.into(),
                address: Some(device),
                calls: Vec::new(),
                muted: None,
            }];
            self.telephony.send_modify(|v| *v += 1);
            Ok(())
        })
    }

    fn disconnect_hands_free(
        &self,
        device: BluetoothAddress,
    ) -> BoxFuture<'_, Result<(), PlatformError>> {
        Box::pin(async move {
            self.record(format!("disconnect {device}"));
            self.state.lock().unwrap().gateways.clear();
            self.telephony.send_modify(|v| *v += 1);
            Ok(())
        })
    }

    fn gateways(&self) -> BoxFuture<'_, Result<Vec<Gateway>, PlatformError>> {
        Box::pin(async { Ok(self.state.lock().unwrap().gateways.clone()) })
    }

    fn telephony_changed(&self) -> BoxFuture<'_, Result<(), PlatformError>> {
        Box::pin(async {
            let mut seen = self.telephony_seen.lock().await;
            seen.changed()
                .await
                .map_err(|_| PlatformError::Unsupported("telephony"))
        })
    }

    fn answer<'a>(&'a self, call: &'a str) -> BoxFuture<'a, Result<(), PlatformError>> {
        Box::pin(async move {
            self.record(format!("answer {call}"));
            self.change_calls(|calls| {
                for c in calls.iter_mut().filter(|c| c.id == call) {
                    c.state = CallState::Active;
                }
            });
            Ok(())
        })
    }

    fn hang_up<'a>(&'a self, call: &'a str) -> BoxFuture<'a, Result<(), PlatformError>> {
        Box::pin(async move {
            self.record(format!("hang up {call}"));
            self.change_calls(|calls| calls.retain(|c| c.id != call));
            Ok(())
        })
    }

    fn dial<'a>(
        &'a self,
        gateway: &'a str,
        number: &'a str,
    ) -> BoxFuture<'a, Result<(), PlatformError>> {
        Box::pin(async move {
            self.record(format!("dial {gateway} {number}"));
            Ok(())
        })
    }

    fn send_tones<'a>(
        &'a self,
        gateway: &'a str,
        tones: &'a str,
    ) -> BoxFuture<'a, Result<(), PlatformError>> {
        Box::pin(async move {
            self.record(format!("tones {gateway} {tones}"));
            Ok(())
        })
    }

    fn set_muted<'a>(
        &'a self,
        gateway: &'a str,
        muted: bool,
    ) -> BoxFuture<'a, Result<(), PlatformError>> {
        Box::pin(async move {
            self.record(format!("muted {gateway} {muted}"));
            Ok(())
        })
    }
}

/// The phone side of a session for a phone that offers calls, once calls
/// are ready on both devices.
async fn calls_session(
    phone: &TestPhone,
    desktop: CertificateFingerprint,
) -> Session<TlsStream<TcpStream>> {
    let connection = phone.accept(PhoneTls::pinned(desktop)).await.unwrap();
    let info = DeviceInfo::new(
        phone.id.clone(),
        phone.name.clone(),
        DeviceKind::Phone,
        [Feature::Calls],
    );
    let mut session =
        Session::handshake(connection.wire.into_inner(), info, SessionConfig::phone())
            .await
            .unwrap();
    assert!(session.peer().features.contains(&Feature::Calls));
    while !session.is_ready(Feature::Calls) {
        next_event(&mut session).await;
    }
    session
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_phone_pairs_over_bluetooth_and_its_call_is_answered_from_the_popup() {
    let calls = FakeCalls::new();
    let rig = start_rig(None, None, Some(calls.clone()));
    let phone = Arc::new(TestPhone::bind(LOCALHOST, "calls-1").await);
    let (desktop, device) = pair(&rig, &phone).await;
    let mut session = within(calls_session(&phone, desktop)).await;
    let status = rig
        .events
        .calls_where(|s| {
            s.phones
                .first()
                .is_some_and(|p| p.link == CallLink::NotPaired)
        })
        .await;
    assert_eq!(status.desktop, CallsDesktop::Ready);

    // The user pairs the phone over Bluetooth: the phone is asked to.
    rig.runtime.start_bluetooth_pairing(&device.id).unwrap();
    let (seq, request) = loop {
        if let SessionEvent::Request {
            seq,
            payload: Payload::BluetoothPairingRequest(request),
        } = next_event(&mut session).await
        {
            break (seq, request);
        }
    };
    assert_eq!(
        request,
        BluetoothPairingRequest {
            name: "Desk BT".into(),
            address: "A0:B1:C2:D3:E4:F5".into(),
        }
    );
    session.ack(seq).await.unwrap();
    rig.events
        .calls_where(|s| {
            s.pairing
                .as_ref()
                .is_some_and(|p| p.phone == PairingRequestState::Asked)
        })
        .await;
    calls.wait_for("open").await;

    // The phone pairs, and the user confirms the code here.
    calls.prompt(5, 123_456);
    let status = rig
        .events
        .calls_where(|s| s.pairing.as_ref().is_some_and(|p| p.prompt.is_some()))
        .await;
    assert_eq!(status.pairing.unwrap().prompt.unwrap().code, "123456");
    rig.runtime.confirm_bluetooth_code(5, true).unwrap();
    calls.wait_for("answer pairing 5 true").await;
    calls.pair_device(&phone.name);
    calls.wait_for("close").await;
    calls.wait_for(&format!("connect {PHONE_BT}")).await;
    rig.events
        .calls_where(|s| {
            s.pairing.is_none()
                && s.phones
                    .first()
                    .is_some_and(|p| matches!(p.link, CallLink::Connected { .. }))
        })
        .await;

    // A call comes in: a pop-up offers it, and answering it there answers
    // over Bluetooth.
    calls.ring("+15550100");
    let (id, popup) = rig.sink.wait_for_banners(1).await[0].clone();
    assert_eq!(popup.summary, "Call from +15550100");
    rig.respond
        .send(NotificationResponse::Action {
            id,
            key: "calls.answer".into(),
        })
        .unwrap();
    calls.wait_for(&format!("answer {GATEWAY}/call0")).await;
    let status = rig
        .events
        .calls_where(|s| {
            s.phones
                .first()
                .and_then(|p| p.calls.first())
                .is_some_and(|c| c.state == CallStateView::Active)
        })
        .await;
    assert!(status.phones[0].calls[0].active_since_unix_ms.is_some());
    wait(&rig.sink.changed, || {
        rig.sink.closed.lock().unwrap().contains(&id).then_some(())
    })
    .await;

    // The keypad and the dial pad go to the gateway.
    rig.runtime.send_tones(&device.id, "1#").unwrap();
    calls.wait_for(&format!("tones {GATEWAY} 1#")).await;
    rig.runtime.dial(&device.id, "+1 (555) 0123").unwrap();
    calls.wait_for(&format!("dial {GATEWAY} +15550123")).await;
    rig.runtime
        .hang_up_call(&device.id, &format!("bt:{GATEWAY}/call0"))
        .unwrap();
    calls.wait_for(&format!("hang up {GATEWAY}/call0")).await;

    // Calls turned off here: the phone hears of it, and the hands-free
    // link goes down so the phone keeps its calls.
    rig.runtime.set_calls_enabled(false);
    loop {
        next_event(&mut session).await;
        let disabled = session
            .peer_availability()
            .and_then(|states| states.get(&Feature::Calls))
            .is_some_and(|state| *state == Availability::Disabled);
        if disabled {
            break;
        }
    }
    calls.wait_for(&format!("disconnect {PHONE_BT}")).await;
}
