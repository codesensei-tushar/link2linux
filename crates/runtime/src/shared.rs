//! What every part of the runtime shares: the identity, the paired phones,
//! the per-phone slots and the way changes reach the shell.

use std::collections::{BTreeMap, HashMap};
use std::io;
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use link2linux_core::clipboard::{ClipboardHub, ClipboardService, ClipboardSettings};
use link2linux_core::device::{DeviceInfo, DeviceKind, Feature};
use link2linux_core::discovery::onlink::{LocalAddress, local_addresses};
use link2linux_core::discovery::{DiscoveryKeys, LanConfig, PairedKey, SecretBytes};
use link2linux_core::identity::{Identity, IdentityError};
use link2linux_core::pairing::{
    DEFAULT_BEACON_PORT, FileTrustStore, PairedDevice, PairingError, StoreKey, TrustStore,
};
use link2linux_core::session::{Availability, SessionStart};
use link2linux_core::transport::{DialTimeouts, TlsDialer, TransportError};
use link2linux_platform::{Clipboard, ClipboardAccess};
use link2linux_protocol::v1::PresenceUpdate;
use tokio::runtime::Handle;
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};
use tokio::sync::{Notify, watch};
use tokio_util::sync::CancellationToken;

use crate::candidates::Evidence;
use crate::clipboard::{
    self, ClipboardMode, ClipboardPhoneState, ClipboardPort, ClipboardStatus, HeldClipboard,
    NoClipboard, Outgoing, SharedClipboard,
};
use crate::device::{self, Established, Inbox};
use crate::features::Features;
use crate::link::{Command, lock};
use crate::notify::dto::{self, MirroredNotification, NotificationEvent};
use crate::notify::{DriverFactory, Presenter};
use crate::pairing::PairingCtl;
use crate::paths::{DataDir, desktop_name};
use crate::view::{Device, DeviceStatus, EventSink, StartupError};

/// How the runtime starts.
#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    /// Where the identity and the paired phones are kept.
    pub data_dir: DataDir,
    /// The name phones see for this computer; the host name when `None`.
    pub desktop_name: Option<String>,
    /// How discovery listens. `None` runs no discovery at all.
    pub discovery: Option<LanConfig>,
    /// Addresses to treat as on-link next to the machine's own, which lets
    /// tests dial loopback.
    pub extra_locals: Vec<LocalAddress>,
    /// Dial timeouts, for tests; the protocol's when `None`.
    pub dial_timeouts: Option<DialTimeouts>,
    /// The user's clipboard settings at the start.
    pub clipboard: ClipboardSettings,
}

impl RuntimeConfig {
    /// The usual configuration for the data directory `data_dir`.
    #[must_use]
    pub fn new(data_dir: DataDir) -> Self {
        Self {
            data_dir,
            desktop_name: None,
            discovery: Some(LanConfig::default()),
            extra_locals: Vec::new(),
            dial_timeouts: None,
            clipboard: ClipboardSettings::default(),
        }
    }
}

/// What the shell provides to the runtime.
pub struct Shell {
    /// This desktop's identity, which the shell loads with
    /// [`DataDir::load_identity`](crate::paths::DataDir::load_identity)
    /// before starting, since the keyring may prompt the user.
    pub identity: Identity,
    /// Seals the phones' presence keys in the pairing file, from
    /// [`DataDir::load_store_key`](crate::paths::DataDir::load_store_key).
    /// `None` keeps them there in clear.
    pub store_key: Option<StoreKey>,
    /// Shows banners on the desktop.
    pub presenter: Arc<dyn Presenter>,
    /// Receives every change as a snapshot.
    pub events: Arc<dyn EventSink>,
    /// Makes the notifications driver of each phone.
    pub drivers: Arc<dyn DriverFactory>,
    /// The desktop clipboard. Without one, the desktop reports the
    /// clipboard unavailable to the phones.
    pub clipboard: Option<Arc<dyn ClipboardPort>>,
}

/// Why the runtime could not start.
#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    /// The data directory could not be used.
    #[error("data directory: {0}")]
    Data(#[from] io::Error),
    /// The identity could not be loaded or made.
    #[error("identity: {0}")]
    Identity(#[from] IdentityError),
    /// The paired phones could not be read.
    #[error("paired phones: {0}")]
    Store(#[from] PairingError),
    /// The TLS client could not be set up.
    #[error("transport: {0}")]
    Transport(#[from] TransportError),
}

/// Told when the cause gives the user nothing better to do.
const TRY_AGAIN: &str = "Try again. If it keeps happening, report it with the details below.";

impl From<&RuntimeError> for StartupError {
    fn from(error: &RuntimeError) -> Self {
        let (message, hint) = match error {
            RuntimeError::Identity(IdentityError::KeyringUnavailable(_)) => (
                "link2linux couldn't open your keyring, which holds the key your phones know \
                 this computer by.",
                "Your keyring is locked or not running. Unlock it, then try again.",
            ),
            RuntimeError::Data(_) => (
                "link2linux couldn't use its data folder.",
                "Check that your home folder has free space and can be written to, then try \
                 again.",
            ),
            RuntimeError::Identity(_) => (
                "link2linux couldn't load the key your phones know this computer by.",
                TRY_AGAIN,
            ),
            RuntimeError::Store(_) => (
                "link2linux couldn't read the list of paired phones.",
                TRY_AGAIN,
            ),
            RuntimeError::Transport(_) => (
                "link2linux couldn't set up its secure connections.",
                TRY_AGAIN,
            ),
        };
        Self {
            message: message.to_owned(),
            hint: hint.to_owned(),
            detail: error.to_string(),
        }
    }
}

/// One paired phone's connection state and the way to reach its task.
pub(crate) struct Slot {
    /// The phone's notifications as the page shows them, by key.
    pub board: Arc<Mutex<BTreeMap<String, MirroredNotification>>>,
    pub status: DeviceStatus,
    pub last_connected_unix_ms: Option<i64>,
    pub commands: UnboundedSender<Command>,
    /// The newest copy to push to the phone's clipboard.
    pub copies: watch::Sender<Option<Outgoing>>,
    pub wake: Arc<Notify>,
    pub cancel: CancellationToken,
}

/// The state every task of the runtime shares.
pub(crate) struct Core {
    pub handle: Handle,
    pub local: DeviceInfo,
    pub desktop_name: String,
    /// The UDP port this desktop's beacon listener binds, named in QR codes.
    pub beacon_port: AtomicU16,
    pub dialer: TlsDialer,
    pub identity: Identity,
    pub store: Mutex<FileTrustStore>,
    pub events: Arc<dyn EventSink>,
    pub presenter: Arc<dyn Presenter>,
    pub drivers: Arc<dyn DriverFactory>,
    pub extra_locals: Vec<LocalAddress>,
    pub keys: watch::Sender<DiscoveryKeys>,
    pub evidence: Mutex<Evidence>,
    pub offer_secret: Mutex<Option<SecretBytes>>,
    pub ble_address: Mutex<Option<[u8; 6]>>,
    pub active: AtomicBool,
    pub cancel: CancellationToken,
    /// The paired phones' slots. Take this lock before the store's.
    pub slots: Mutex<HashMap<String, Slot>>,
    pub pairing: Mutex<PairingCtl>,
    /// Phones that announce pairing mode, whether or not pairing runs.
    pub pairing_phones: Mutex<crate::pairing::PhoneList>,
    /// The desktop clipboard, if the shell has one.
    pub clipboard: Option<SharedClipboard>,
    pub clipboard_hub: ClipboardHub,
    /// What the clipboard page shows. Take this lock before the store's.
    pub clipboard_page: Mutex<clipboard::Page>,
    /// Wakes the task that keeps the clipboard page.
    pub clipboard_nudge: Notify,
}

impl Core {
    pub fn open(
        handle: Handle,
        config: &RuntimeConfig,
        shell: Shell,
    ) -> Result<Arc<Self>, RuntimeError> {
        config.data_dir.create()?;
        let identity = shell.identity;
        let store = FileTrustStore::open_sealed(config.data_dir.trust_store(), shell.store_key)?;
        let id = config.data_dir.load_or_create_device_id()?;
        let name = config.desktop_name.clone().unwrap_or_else(desktop_name);
        let mut dialer = TlsDialer::new(&identity)?;
        if let Some(timeouts) = config.dial_timeouts {
            dialer = dialer.with_timeouts(timeouts);
        }
        let local = DeviceInfo::new(
            id,
            name.clone(),
            DeviceKind::Desktop,
            [Feature::Notifications, Feature::Clipboard],
        );
        let (keys, _) = watch::channel(DiscoveryKeys::default());
        let core = Arc::new(Self {
            handle,
            local,
            desktop_name: name,
            beacon_port: AtomicU16::new(
                config
                    .discovery
                    .map_or(DEFAULT_BEACON_PORT, |lan| lan.beacon_port),
            ),
            dialer,
            identity,
            store: Mutex::new(store),
            events: shell.events,
            presenter: shell.presenter,
            drivers: shell.drivers,
            extra_locals: config.extra_locals.clone(),
            keys,
            evidence: Mutex::default(),
            offer_secret: Mutex::new(None),
            ble_address: Mutex::new(None),
            active: AtomicBool::new(false),
            cancel: CancellationToken::new(),
            slots: Mutex::default(),
            pairing: Mutex::new(PairingCtl::default()),
            pairing_phones: Mutex::default(),
            clipboard: shell.clipboard.map(SharedClipboard),
            clipboard_hub: ClipboardHub::new(config.clipboard),
            clipboard_page: Mutex::default(),
            clipboard_nudge: Notify::new(),
        });
        // Following the hub before any phone's service can report.
        let clipboard_events = core.clipboard_hub.subscribe();
        core.handle
            .spawn(clipboard::follow(core.clone(), clipboard_events));
        core.publish_keys();
        let records = lock(&core.store).list();
        for record in &records {
            core.spawn_device(record, None);
        }
        Ok(core)
    }

    /// This machine's addresses plus the configured extras.
    pub fn locals(&self) -> Vec<LocalAddress> {
        let mut locals = local_addresses().unwrap_or_default();
        locals.extend(self.extra_locals.iter().copied());
        locals
    }

    /// The first message of a session: this desktop, whether it shares
    /// its clipboard, and its Bluetooth LE address if it advertises.
    pub fn session_start(&self) -> SessionStart {
        let mut start = SessionStart::new(self.local.clone());
        start
            .availability
            .insert(Feature::Clipboard, self.clipboard_availability());
        if let Some(address) = *lock(&self.ble_address) {
            start.presence = PresenceUpdate {
                ble_address: address.to_vec(),
                ..PresenceUpdate::default()
            };
        }
        start
    }

    /// Starts keeping a session with `record`, using `initial` as the
    /// first one when a connection to it is already up.
    pub fn spawn_device(self: &Arc<Self>, record: &PairedDevice, initial: Option<Established>) {
        let (commands, receiver) = unbounded_channel();
        let (copies, copies_receiver) = watch::channel(None);
        let wake = Arc::new(Notify::new());
        let cancel = self.cancel.child_token();
        let connected = initial.is_some();
        let board = Arc::new(Mutex::new(BTreeMap::new()));
        {
            let mut slots = lock(&self.slots);
            if let Some(old) = slots.remove(&record.id) {
                old.cancel.cancel();
            }
            slots.insert(
                record.id.clone(),
                Slot {
                    board: board.clone(),
                    status: if connected {
                        DeviceStatus::Connected
                    } else {
                        DeviceStatus::Reconnecting
                    },
                    last_connected_unix_ms: connected.then(now_unix_ms),
                    commands,
                    copies,
                    wake: wake.clone(),
                    cancel: cancel.clone(),
                },
            );
        }
        let info = DeviceInfo::new(
            record.id.clone(),
            record.name.clone(),
            record.kind,
            [Feature::Notifications],
        );
        let features = Features {
            notifications: self.drivers.create(&info),
            clipboard: ClipboardService::new(
                record.id.clone(),
                self.clipboard_hub.clone(),
                self.clipboard_port(),
            ),
        };
        self.handle.spawn(forward_changes(
            self.clone(),
            record.id.clone(),
            board,
            features.notifications.subscribe(),
        ));
        let inbox = Inbox {
            commands: receiver,
            copies: copies_receiver,
            wake,
            cancel,
        };
        self.handle.spawn(device::supervise(
            self.clone(),
            record.id.clone(),
            features,
            inbox,
            initial,
        ));
    }

    /// The paired phones as the UI shows them, in pairing order.
    pub fn devices(&self) -> Vec<Device> {
        let slots = lock(&self.slots);
        self.devices_with(&slots)
    }

    fn devices_with(&self, slots: &HashMap<String, Slot>) -> Vec<Device> {
        let mut records = lock(&self.store).list();
        records.sort_by_key(|record| (record.paired_at, record.id.clone()));
        records
            .iter()
            .map(|record| device_view(record, slots.get(&record.id)))
            .collect()
    }

    /// Reports the devices to the shell. The slot lock stays held while the
    /// sink runs, so snapshots arrive in order.
    pub fn emit_devices(&self) {
        let slots = lock(&self.slots);
        let devices = self.devices_with(&slots);
        self.events.devices_changed(&devices);
    }

    /// Records a phone's connection state, and reports it if it changed.
    pub fn set_status(&self, id: &str, status: DeviceStatus, connected_now: bool) {
        let changed = {
            let mut slots = lock(&self.slots);
            let Some(slot) = slots.get_mut(id) else {
                return;
            };
            let before = (slot.status, slot.last_connected_unix_ms);
            slot.status = status;
            if connected_now {
                slot.last_connected_unix_ms = Some(now_unix_ms());
            }
            before != (slot.status, slot.last_connected_unix_ms)
        };
        if changed {
            self.emit_devices();
        }
    }

    /// A phone's record is gone: forget its slot, its pictures and what
    /// discovery showed, and report. `stop` ends the phone's task at once; without
    /// it the task has been told to leave and says goodbye first.
    pub fn forget_device(&self, id: &str, stop: bool) {
        let slot = lock(&self.slots).remove(id);
        if let Some(slot) = slot
            && stop
        {
            slot.cancel.cancel();
        }
        self.drivers.forget(id);
        lock(&self.evidence).forget(id);
        lock(&self.clipboard_page).board.forget(id);
        self.clipboard_hub.forget_phone(id);
        self.publish_keys();
        self.emit_devices();
        self.clipboard_changed();
    }

    /// Gives discovery the keys of the paired phones and of the QR offer
    /// that is showing.
    pub fn publish_keys(&self) {
        let paired = lock(&self.store)
            .list()
            .into_iter()
            .filter_map(|record| {
                record.presence_key.map(|key| PairedKey {
                    phone_id: record.id,
                    presence_key: SecretBytes::new(key.0.to_vec()),
                })
            })
            .collect();
        let offer_secret = lock(&self.offer_secret).clone();
        self.keys.send_replace(DiscoveryKeys {
            paired,
            offer_secret,
        });
    }

    /// Whether the window has focus.
    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::Relaxed)
    }

    /// The desktop clipboard each phone's service writes to.
    pub fn clipboard_port(&self) -> SharedClipboard {
        self.clipboard
            .clone()
            .unwrap_or_else(|| SharedClipboard(Arc::new(NoClipboard)))
    }

    /// What this desktop reports about its clipboard in `FeatureStatus`.
    pub fn clipboard_availability(&self) -> Availability {
        if self.clipboard.is_none() {
            Availability::Unavailable
        } else if self.clipboard_hub.settings().sharing {
            Availability::Ready
        } else {
            Availability::Disabled
        }
    }

    /// The name of paired phone `id`, or the id of one no longer paired.
    pub fn phone_name(&self, id: &str) -> String {
        lock(&self.store)
            .list()
            .into_iter()
            .find(|record| record.id == id)
            .map_or_else(|| id.to_owned(), |record| record.name)
    }

    /// What the clipboard page shows.
    pub fn clipboard_status(&self) -> ClipboardStatus {
        let page = lock(&self.clipboard_page);
        self.clipboard_with(&page)
    }

    fn clipboard_with(&self, page: &clipboard::Page) -> ClipboardStatus {
        let mode = match self.clipboard.as_ref().map(Clipboard::access) {
            Some(ClipboardAccess::Background) => ClipboardMode::Automatic,
            Some(ClipboardAccess::FocusOnly) => ClipboardMode::WhenFocused,
            None => ClipboardMode::Unavailable,
        };
        let records = self.paired_in_order();
        let held = self.clipboard_hub.held().map(|held| HeldClipboard {
            phone_name: records
                .iter()
                .find(|record| record.id == held.device_id)
                .map_or_else(|| held.device_id.clone(), |record| record.name.clone()),
            device_id: held.device_id,
            bytes: clipboard::bytes(held.clip.len()),
            sensitive: held.clip.is_sensitive(),
        });
        let paired = records
            .into_iter()
            .map(|record| (record.id, record.name))
            .collect();
        let phones = clipboard::phones(paired, &self.clipboard_hub.phones());
        page.board.snapshot(mode, phones, held)
    }

    fn paired_in_order(&self) -> Vec<PairedDevice> {
        let mut records = lock(&self.store).list();
        records.sort_by_key(|record| (record.paired_at, record.id.clone()));
        records
    }

    /// Whether `id` is a paired phone.
    pub fn is_paired(&self, id: &str) -> bool {
        lock(&self.store)
            .list()
            .iter()
            .any(|record| record.id == id)
    }

    /// Whether any paired phone takes the clipboard now.
    pub fn clipboard_any_ready(&self) -> bool {
        let paired = self
            .paired_in_order()
            .into_iter()
            .map(|record| (record.id, record.name))
            .collect();
        clipboard::phones(paired, &self.clipboard_hub.phones())
            .iter()
            .any(|phone| phone.state == ClipboardPhoneState::Ready)
    }

    /// Reports the clipboard page to the shell. The page's lock stays held
    /// while the sink runs, so snapshots arrive in order.
    pub fn emit_clipboard(&self) {
        let page = lock(&self.clipboard_page);
        let status = self.clipboard_with(&page);
        self.events.clipboard_changed(&status);
    }

    /// Something the clipboard page shows changed outside the hub's
    /// events: the page is reported again, and the pop-up checked.
    pub fn clipboard_changed(&self) {
        self.clipboard_nudge.notify_one();
    }
}

/// Keeps a phone's board in step with its notifications service and tells
/// the shell about each change. Ends when the service is dropped.
async fn forward_changes(
    core: Arc<Core>,
    device_id: String,
    board: Arc<Mutex<BTreeMap<String, MirroredNotification>>>,
    mut changes: tokio::sync::broadcast::Receiver<link2linux_core::notifications::Change>,
) {
    loop {
        match changes.recv().await {
            Ok(change) => {
                let event = dto::event(&change);
                {
                    let mut board = lock(&board);
                    match &event {
                        NotificationEvent::Posted { notification, .. } => {
                            board.insert(notification.key.clone(), notification.clone());
                        }
                        NotificationEvent::Removed { key } => {
                            board.remove(key);
                        }
                    }
                }
                core.events.notification_changed(&device_id, &event);
            }
            Err(tokio::sync::broadcast::error::RecvError::Lagged(missed)) => {
                tracing::warn!(missed, "the notifications page missed some changes");
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
        }
    }
}

fn device_view(record: &PairedDevice, slot: Option<&Slot>) -> Device {
    Device {
        id: record.id.clone(),
        name: record.name.clone(),
        model: None,
        status: slot.map_or(DeviceStatus::Disconnected, |slot| slot.status),
        paired_at_unix_ms: unix_ms(record.paired_at),
        last_connected_unix_ms: slot.and_then(|slot| slot.last_connected_unix_ms),
    }
}

/// Milliseconds since the Unix epoch, for timestamps the UI shows.
pub(crate) fn unix_ms(time: SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH).map_or(0, |since| {
        i64::try_from(since.as_millis()).unwrap_or(i64::MAX)
    })
}

pub(crate) fn now_unix_ms() -> i64 {
    unix_ms(SystemTime::now())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_start_says_what_failed_and_what_to_do() {
        let locked = StartupError::from(&RuntimeError::Identity(
            IdentityError::KeyringUnavailable("the prompt was dismissed".into()),
        ));
        assert!(locked.message.contains("keyring"));
        assert_eq!(
            locked.hint,
            "Your keyring is locked or not running. Unlock it, then try again."
        );
        assert!(locked.detail.contains("the prompt was dismissed"));

        let data = StartupError::from(&RuntimeError::Data(io::Error::other("disk full")));
        assert_eq!(data.message, "link2linux couldn't use its data folder.");
        assert!(data.detail.contains("disk full"));
    }
}
