//! The seam between the desktop shell and whatever does the pairing.
//!
//! The UI talks to the [`Backend`] trait only. Today the app runs the
//! deterministic fake backend; once `crates/core` exposes pairing and a
//! device registry, a real implementation replaces it in
//! [`crate::AppState::default`] and nothing else changes: commands, events and
//! the generated TypeScript bindings all use the types defined here.
//!
//! Commands never return state. They start or answer something, and the
//! resulting state arrives through [`EventSink`], so a slow reply can never
//! overwrite a newer event.
//!
//! # Assumed mapping to `crates/core`
//!
//! The real backend is built elsewhere, so these are assumptions to check
//! when swapping it in:
//!
//! - [`PairingMethod::QrCode`] is the QR pairing mode and
//!   [`PairingMethod::Sas`] is the short-authentication-string mode.
//! - [`PairingState::QrOffer`] is one active offer. `uri` is the
//!   `link2linux://pair?...` payload and `svg` is `qr::svg(uri)`; the backend
//!   (not the UI) renders it. Offers last 5 minutes and are replaced by a new
//!   one shortly before they expire.
//! - [`PairingState::Searching`] lists phones whose mDNS record advertises
//!   `pm=1`. [`PhoneTarget::Address`] is the typed `IP:port` fallback.
//! - [`PairingState::SasCode`] carries the six digits, grouped for display by
//!   the UI. The backend allows 3 SAS attempts per `start_pairing(Sas)`; one
//!   counts when the phone reveals its nonce.
//! - [`PairingFailure`] values map one to one onto the protocol's pairing
//!   outcomes and local errors (`rejected` and `notExpected` come from
//!   `PairingResult`, `expired` and `tooManyAttempts` are local).
//! - [`Device`] comes from the pairing record plus the session state.
//!   `reconnecting` covers the first two minutes after a session is lost,
//!   `revoked` is a record the phone has told us it removed.

use serde::{Deserialize, Serialize};
use specta::Type;

/// How the user pairs a new phone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum PairingMethod {
    /// The phone scans a QR code shown on this computer.
    QrCode,
    /// Both screens show a six-digit code and the user compares them.
    Sas,
}

/// Which phone to dial.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum PhoneTarget {
    /// A phone from [`PairingState::Searching`], by its id.
    #[serde(rename_all = "camelCase")]
    Discovered { id: String },
    /// An address typed by the user: `192.168.1.20`, `192.168.1.20:27380` or
    /// `[fd00::2]:27380`. The default port is 27380.
    #[serde(rename_all = "camelCase")]
    Address { address: String },
}

/// A phone that announced it is ready to pair.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredPhone {
    pub id: String,
    pub name: String,
    pub address: String,
}

/// Where a pairing attempt currently stands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "step", rename_all = "camelCase")]
pub enum PairingState {
    /// Nothing is happening.
    Idle,
    /// A QR code is on screen, waiting for a phone to scan it.
    #[serde(rename_all = "camelCase")]
    QrOffer {
        /// The `link2linux://pair?...` payload held by the code.
        uri: String,
        /// The code as an SVG document, black on white so it scans in any theme.
        svg: String,
        /// The name the phone will see for this computer.
        desktop_name: String,
        /// Time left until this offer is replaced.
        expires_in_ms: u32,
        /// Length of the offer, for drawing progress.
        lifetime_ms: u32,
    },
    /// Looking for phones in pairing mode (code comparison).
    #[serde(rename_all = "camelCase")]
    Searching {
        phones: Vec<DiscoveredPhone>,
        /// Attempts left before the user has to start again.
        attempts_left: u8,
        /// Why the previous attempt ended, if one did.
        last_failure: Option<PairingFailure>,
    },
    /// Opening a connection.
    #[serde(rename_all = "camelCase")]
    Connecting {
        method: PairingMethod,
        phone_name: Option<String>,
    },
    /// Checking the phone's proof.
    #[serde(rename_all = "camelCase")]
    Verifying {
        method: PairingMethod,
        phone_name: Option<String>,
    },
    /// Both screens show this code and the user compares them.
    #[serde(rename_all = "camelCase")]
    SasCode {
        /// Six digits, without spaces.
        code: String,
        phone_name: String,
        expires_in_ms: u32,
        lifetime_ms: u32,
        /// The user has already confirmed here and we wait for the phone.
        confirmed_here: bool,
    },
    /// The phone is now paired.
    Paired { device: Device },
    /// Pairing ended without a phone being paired.
    #[serde(rename_all = "camelCase")]
    Failed {
        method: PairingMethod,
        reason: PairingFailure,
    },
}

/// Why pairing did not finish.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum PairingFailure {
    /// The phone refused the connection.
    Rejected,
    /// The person using the phone chose not to pair.
    DeclinedByPhone,
    /// The codes did not match, or the phone's proof was wrong.
    VerificationFailed,
    /// A step took too long.
    TimedOut,
    /// The phone was not waiting for this computer.
    NotExpected,
    /// The phone runs a version that cannot pair with this one.
    Unsupported,
    /// The QR code expired and could not be renewed.
    Expired,
    /// No phone answered at that address.
    PhoneNotFound,
    /// The connection dropped part way.
    ConnectionLost,
    /// Three code comparisons did not succeed.
    TooManyAttempts,
}

/// A phone paired with this computer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    /// Stable identifier assigned at pairing time.
    pub id: String,
    /// Name the user gave the phone.
    pub name: String,
    /// Hardware model, when the phone reports one.
    pub model: Option<String>,
    pub status: DeviceStatus,
    /// When the phone was paired (Unix time, milliseconds).
    #[specta(type = specta_typescript::Number)]
    pub paired_at_unix_ms: i64,
    /// When a session with the phone last ended or started.
    #[specta(type = Option<specta_typescript::Number>)]
    pub last_connected_unix_ms: Option<i64>,
}

/// Whether a paired phone currently has a live session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum DeviceStatus {
    Connected,
    /// The session was lost less than two minutes ago and is being restored.
    Reconnecting,
    Disconnected,
    /// The phone removed this computer. Unpair to clear the entry.
    Revoked,
}

/// Why a request to the backend was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum BackendError {
    /// The request does not fit the current pairing step.
    #[error("That isn't possible right now.")]
    WrongState,
    /// The address is not an IP address with an optional port.
    #[error("Enter an address like 192.168.1.20 or 192.168.1.20:27380.")]
    InvalidAddress,
    /// No such phone.
    #[error("That phone is no longer available.")]
    NotFound,
}

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
}

/// Where a backend reports changes. Called with the backend's lock held, so
/// events arrive in order; implementations must return quickly.
pub trait EventSink: Send + Sync {
    fn pairing_changed(&self, state: &PairingState);
    fn devices_changed(&self, devices: &[Device]);
}
