//! The runtime the shell holds.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use link2linux_core::pairing::{TrustStore, records};
use tokio::runtime::Handle;

use crate::discovery::{pump_responses, run_discovery, watch_network};
use crate::link::{Command, lock};
use crate::pairing;
use crate::shared::{Core, RuntimeConfig, RuntimeError, Shell};
use crate::view::{BackendError, Device, PairingMethod, PairingState, PhoneTarget};

/// Discovery, pairing and a session with each paired phone.
///
/// Every method returns at once. Changes are reported through the event
/// sink the runtime was started with, which also receives the state a
/// command leads to. Dropping the runtime stops it.
pub struct Runtime {
    core: Arc<Core>,
}

impl Runtime {
    /// Loads the identity and the paired phones and starts working on
    /// `handle`: discovery runs, and every paired phone is dialed.
    ///
    /// # Errors
    ///
    /// Fails when the data directory, the identity or the list of paired
    /// phones cannot be used.
    pub fn start(
        handle: Handle,
        config: &RuntimeConfig,
        shell: Shell,
    ) -> Result<Self, RuntimeError> {
        let core = Core::open(handle.clone(), config, shell)?;
        if let Some(lan) = config.discovery {
            handle.spawn(run_discovery(core.clone(), lan));
        }
        handle.spawn(watch_network(core.clone()));
        handle.spawn(pump_responses(core.clone()));
        Ok(Self { core })
    }

    /// The paired phones, in pairing order.
    #[must_use]
    pub fn devices(&self) -> Vec<Device> {
        self.core.devices()
    }

    /// The current pairing step.
    #[must_use]
    pub fn pairing(&self) -> PairingState {
        pairing::state(&self.core)
    }

    /// Begins a pairing activation.
    ///
    /// # Errors
    ///
    /// Refused unless pairing is idle or finished.
    pub fn start_pairing(&self, method: PairingMethod) -> Result<(), BackendError> {
        pairing::start(&self.core, method)
    }

    /// Dials a phone from the search list, or a typed address.
    ///
    /// # Errors
    ///
    /// Refused when the target does not fit the current step.
    pub fn pair_with(&self, target: PhoneTarget) -> Result<(), BackendError> {
        pairing::pair_with(&self.core, target)
    }

    /// The user's answer to "do the codes match?".
    ///
    /// # Errors
    ///
    /// Refused when no comparison is waiting.
    pub fn answer_code(&self, matches: bool) -> Result<(), BackendError> {
        pairing::answer_code(&self.core, matches)
    }

    /// Stops pairing, or dismisses a finished one.
    pub fn cancel_pairing(&self) {
        pairing::cancel(&self.core);
    }

    /// Forgets a phone: tells it, if a session is up, and deletes the
    /// record.
    ///
    /// # Errors
    ///
    /// Refused for a phone that is not paired.
    pub fn unpair(&self, device_id: &str) -> Result<(), BackendError> {
        let core = &self.core;
        let record = lock(&core.store)
            .list()
            .into_iter()
            .find(|record| record.id == device_id)
            .ok_or(BackendError::NotFound)?;
        let removed = records::unpair(&mut *lock(&core.store), &record.fingerprint);
        match removed {
            Ok(Some(_)) => {}
            Ok(None) => return Err(BackendError::NotFound),
            Err(error) => {
                tracing::error!(%error, "could not remove the pairing record");
                return Err(BackendError::WrongState);
            }
        }
        // The task says goodbye if a session is up, and then ends.
        if let Some(slot) = lock(&core.slots).get(device_id) {
            let _ = slot.commands.send(Command::Unpair);
        }
        core.forget_device(device_id, false);
        Ok(())
    }

    /// Asks a phone to connect again right away, for example after the
    /// user disconnected on the phone.
    pub fn reconnect(&self, device_id: &str) {
        if let Some(slot) = lock(&self.core.slots).get(device_id) {
            let _ = slot.commands.send(Command::Reconnect);
        }
    }

    /// Tells the sessions whether the user can see the window, which sets
    /// how often they ping.
    pub fn set_active(&self, active: bool) {
        self.core.active.store(active, Ordering::Relaxed);
        for slot in lock(&self.core.slots).values() {
            let _ = slot.commands.send(Command::SetActive(active));
        }
    }

    /// The Bluetooth LE address this computer advertises from, which QR
    /// codes and new sessions carry.
    pub fn set_ble_address(&self, address: Option<[u8; 6]>) {
        *lock(&self.core.ble_address) = address;
    }

    /// The UDP port the beacon listener is bound to.
    #[must_use]
    pub fn beacon_port(&self) -> u16 {
        self.core.beacon_port.load(Ordering::Relaxed)
    }

    /// Ends every session with a `Goodbye` and stops all tasks.
    pub fn shutdown(&self) {
        self.core.cancel.cancel();
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        self.shutdown();
    }
}
