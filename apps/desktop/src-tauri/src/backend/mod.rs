//! The seam between the desktop shell and whatever does the pairing.
//!
//! The UI talks to the [`Backend`] trait only. The app runs [`live`], which
//! drives the real runtime, unless `L2L_FAKE_SCRIPT` names one of the
//! deterministic scripts of the [`fake`] backend for development and tests.
//! Commands, events and the generated TypeScript bindings all use the types
//! defined in `link2linux-runtime`, re-exported here.
//!
//! Commands never return state. They start or answer something, and the
//! resulting state arrives through [`EventSink`], so a slow reply can never
//! overwrite a newer event.

pub mod fake;
pub mod live;

pub use link2linux_runtime::view::{
    BackendError, Device, DeviceStatus, DiscoveredPhone, EventSink, PairingFailure, PairingMethod,
    PairingState, PhoneTarget,
};

/// What the desktop shell needs from the pairing and device layer.
pub trait Backend: Send + Sync {
    /// Paired phones, in pairing order.
    fn devices(&self) -> Vec<Device>;
    /// The current pairing step.
    fn pairing(&self) -> PairingState;
    /// Begins a pairing activation. Refused unless pairing is idle or
    /// finished.
    fn start_pairing(&self, method: PairingMethod) -> Result<(), BackendError>;
    /// Dials a phone: from the search list, or a typed address (also allowed
    /// while a QR code is showing).
    fn pair_with(&self, target: PhoneTarget) -> Result<(), BackendError>;
    /// The user's answer to "do the codes match?".
    fn answer_code(&self, matches: bool) -> Result<(), BackendError>;
    /// Stops pairing, or dismisses a finished one. Always ends in `Idle`.
    fn cancel_pairing(&self);
    /// Forgets a phone.
    fn unpair(&self, device_id: &str) -> Result<(), BackendError>;
    /// The window gained or lost focus, which sets how often sessions ping.
    fn set_active(&self, _active: bool) {}
}
