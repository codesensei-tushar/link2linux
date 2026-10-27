//! The BlueZ pairing agent (`org.bluez.Agent1`) the desktop registers
//! while pairing is open.
//!
//! It accepts nothing by itself. A numeric comparison becomes a
//! [`PairingEvent::Confirm`] and waits for [`AgentShared::answer`]; every
//! other way to pair (a PIN, a passkey typed or shown, "just works") is
//! refused, since none of them lets the user confirm a code on the
//! desktop. A device the user confirmed may then connect its services
//! until pairing closes.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use tokio::sync::{mpsc, oneshot};
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue};
use zbus::{Connection, Proxy};

use super::bluez::{BLUEZ, DEVICE};
use super::{CALL_TIMEOUT, bounded};
use crate::{BluetoothAddress, PROMPT_TIMEOUT, PairingEvent, PairingPrompt};

/// Where the agent is exported.
pub(super) const AGENT_PATH: &str = "/org/link2linux/agent";
/// What the desktop can do to confirm a pairing: show a code, and take a
/// yes or no. With a phone, which can too, this makes BlueZ use numeric
/// comparison.
pub(super) const CAPABILITY: &str = "DisplayYesNo";

/// Errors BlueZ understands from an agent.
#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.bluez.Error")]
pub(super) enum AgentError {
    #[zbus(error)]
    ZBus(zbus::Error),
    /// The request is refused.
    Rejected(String),
}

fn rejected(why: &str) -> AgentError {
    AgentError::Rejected(why.to_owned())
}

/// What the agent and the [`BluezBluetooth`](super::BluezBluetooth) that
/// registered it share.
pub(super) struct AgentShared {
    /// Whether pairing is open. Requests that come while it is not are
    /// refused, even if BlueZ still has the agent.
    open: AtomicBool,
    next_id: AtomicU64,
    /// The prompts that wait for the user, by id.
    pending: Mutex<HashMap<u64, oneshot::Sender<bool>>>,
    /// Devices whose pairing the user confirmed since pairing opened.
    confirmed: Mutex<HashSet<BluetoothAddress>>,
    events: mpsc::UnboundedSender<PairingEvent>,
    receiver: tokio::sync::Mutex<mpsc::UnboundedReceiver<PairingEvent>>,
}

impl AgentShared {
    pub(super) fn new() -> Self {
        let (events, receiver) = mpsc::unbounded_channel();
        Self {
            open: AtomicBool::new(false),
            next_id: AtomicU64::new(1),
            pending: Mutex::default(),
            confirmed: Mutex::default(),
            events,
            receiver: tokio::sync::Mutex::new(receiver),
        }
    }

    /// Pairing opens: nobody is confirmed yet.
    pub(super) fn open(&self) {
        lock(&self.confirmed).clear();
        self.open.store(true, Ordering::SeqCst);
    }

    /// Pairing closes: prompts still open are refused.
    pub(super) fn close(&self) {
        self.open.store(false, Ordering::SeqCst);
        self.refuse_all();
        lock(&self.confirmed).clear();
    }

    pub(super) fn is_open(&self) -> bool {
        self.open.load(Ordering::SeqCst)
    }

    pub(super) fn answer(&self, id: u64, accept: bool) {
        if let Some(waiting) = lock(&self.pending).remove(&id) {
            // A request BlueZ gave up on no longer listens; nothing to do.
            let _ = waiting.send(accept && self.is_open());
        }
    }

    pub(super) async fn next_event(&self) -> PairingEvent {
        let mut receiver = self.receiver.lock().await;
        match receiver.recv().await {
            Some(event) => event,
            // The sender lives as long as `self`.
            None => std::future::pending().await,
        }
    }

    /// Ends every prompt without the user's answer, which refuses it.
    fn refuse_all(&self) {
        let ended: Vec<u64> = lock(&self.pending).drain().map(|(id, _)| id).collect();
        for id in ended {
            let _ = self.events.send(PairingEvent::Cancelled { id });
        }
    }

    /// Asks the user about `prompt` and waits for the answer.
    async fn ask(&self, mut prompt: PairingPrompt) -> bool {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        prompt.id = id;
        let device = prompt.device;
        let (answer, waiting) = oneshot::channel();
        lock(&self.pending).insert(id, answer);
        let _ = self.events.send(PairingEvent::Confirm(prompt));
        let accepted = match tokio::time::timeout(PROMPT_TIMEOUT, waiting).await {
            Ok(answer) => answer.unwrap_or(false),
            Err(_) => {
                if lock(&self.pending).remove(&id).is_some() {
                    let _ = self.events.send(PairingEvent::Cancelled { id });
                }
                false
            }
        };
        // Pairing may have closed while the prompt waited.
        let accepted = accepted && self.is_open();
        if accepted {
            lock(&self.confirmed).insert(device);
        }
        accepted
    }

    pub(super) fn is_confirmed(&self, device: BluetoothAddress) -> bool {
        self.is_open() && lock(&self.confirmed).contains(&device)
    }

    /// The devices whose pairing the user confirmed since pairing opened.
    pub(super) fn confirmed(&self) -> HashSet<BluetoothAddress> {
        lock(&self.confirmed).clone()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The exported agent object.
pub(super) struct Agent {
    pub(super) shared: std::sync::Arc<AgentShared>,
}

/// The address in a BlueZ device path, `.../dev_00_11_22_33_44_55`.
pub(super) fn address_of(device: &ObjectPath<'_>) -> Option<BluetoothAddress> {
    let last = device.as_str().rsplit('/').next()?;
    last.strip_prefix("dev_")?.replace('_', ":").parse().ok()
}

/// The name BlueZ shows for `device`, or an empty one.
async fn name_of(connection: &Connection, device: &ObjectPath<'_>) -> String {
    let read = async {
        let properties = Proxy::new(
            connection,
            BLUEZ,
            device.to_owned(),
            "org.freedesktop.DBus.Properties",
        )
        .await?;
        let alias: OwnedValue = properties.call("Get", &(DEVICE, "Alias")).await?;
        Ok::<_, zbus::Error>(String::try_from(alias).unwrap_or_default())
    };
    bounded(CALL_TIMEOUT, read).await.unwrap_or_default()
}

#[zbus::interface(name = "org.bluez.Agent1")]
impl Agent {
    /// BlueZ dropped the agent: whatever waits is refused.
    fn release(&self) {
        tracing::debug!("BlueZ released the pairing agent");
        self.shared.refuse_all();
    }

    /// Legacy pairing with a PIN: no code to confirm.
    fn request_pin_code(&self, _device: OwnedObjectPath) -> Result<String, AgentError> {
        Err(rejected("pairing needs a code confirmed on this computer"))
    }

    fn display_pin_code(
        &self,
        _device: OwnedObjectPath,
        _pincode: String,
    ) -> Result<(), AgentError> {
        Err(rejected("pairing needs a code confirmed on this computer"))
    }

    fn request_passkey(&self, _device: OwnedObjectPath) -> Result<u32, AgentError> {
        Err(rejected("pairing needs a code confirmed on this computer"))
    }

    /// The other device would type the code shown here. It is not shown,
    /// so that pairing cannot succeed.
    fn display_passkey(
        &self,
        _device: OwnedObjectPath,
        _passkey: u32,
        _entered: u16,
    ) -> Result<(), AgentError> {
        Err(rejected("pairing needs a code confirmed on this computer"))
    }

    /// Numeric comparison: the user confirms that both devices show
    /// `passkey`.
    async fn request_confirmation(
        &self,
        device: OwnedObjectPath,
        passkey: u32,
        #[zbus(connection)] connection: &Connection,
    ) -> Result<(), AgentError> {
        if !self.shared.is_open() {
            return Err(rejected("pairing is not open"));
        }
        let Some(address) = address_of(&device) else {
            return Err(rejected("unknown device"));
        };
        let prompt = PairingPrompt {
            id: 0,
            device: address,
            device_name: name_of(connection, &device).await,
            code: passkey,
        };
        if self.shared.ask(prompt).await {
            tracing::info!(%address, "the user confirmed a Bluetooth pairing");
            Ok(())
        } else {
            tracing::info!(%address, "a Bluetooth pairing was refused");
            Err(rejected("the user did not confirm the code"))
        }
    }

    /// Pairing without a code ("just works"): refused, since the user
    /// confirms nothing.
    fn request_authorization(&self, _device: OwnedObjectPath) -> Result<(), AgentError> {
        Err(rejected("pairing needs a code confirmed on this computer"))
    }

    /// A device that is not trusted yet connects a service: allowed for
    /// the device the user just confirmed, which the desktop trusts next.
    fn authorize_service(&self, device: OwnedObjectPath, _uuid: String) -> Result<(), AgentError> {
        match address_of(&device) {
            Some(address) if self.shared.is_confirmed(address) => Ok(()),
            _ => Err(rejected("not a device paired just now")),
        }
    }

    /// The other side gave up: prompts that wait are over.
    fn cancel(&self) {
        self.shared.refuse_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_come_from_device_paths() {
        let path = ObjectPath::try_from("/org/bluez/hci0/dev_0C_1A_2B_3C_4D_5E").unwrap();
        assert_eq!(address_of(&path).unwrap().to_string(), "0C:1A:2B:3C:4D:5E");
        let adapter = ObjectPath::try_from("/org/bluez/hci0").unwrap();
        assert_eq!(address_of(&adapter), None);
    }

    #[tokio::test]
    async fn answers_reach_the_prompt_only_while_open() {
        let shared = std::sync::Arc::new(AgentShared::new());
        shared.open();
        let prompt = PairingPrompt {
            id: 0,
            device: "00:11:22:33:44:55".parse().unwrap(),
            device_name: String::new(),
            code: 1,
        };
        let asking = tokio::spawn({
            let shared = shared.clone();
            let prompt = prompt.clone();
            async move { shared.ask(prompt).await }
        });
        let PairingEvent::Confirm(shown) = shared.next_event().await else {
            panic!("expected a prompt");
        };
        shared.answer(shown.id, true);
        assert!(asking.await.unwrap());
        assert!(shared.is_confirmed(prompt.device));

        // Closing refuses the next prompt even though the user says yes
        // afterwards, and forgets who was confirmed.
        let asking = tokio::spawn({
            let shared = shared.clone();
            async move { shared.ask(prompt).await }
        });
        let PairingEvent::Confirm(shown) = shared.next_event().await else {
            panic!("expected a prompt");
        };
        shared.close();
        assert_eq!(
            shared.next_event().await,
            PairingEvent::Cancelled { id: shown.id }
        );
        shared.answer(shown.id, true);
        assert!(!asking.await.unwrap());
        assert!(!shared.is_confirmed(shown.device));
    }
}
