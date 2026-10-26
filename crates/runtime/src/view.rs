//! The types the desktop shell and the runtime share: pairing steps, paired
//! phones and the sink for changes. They are the IPC types, so they derive
//! the serde and specta traits the generated bindings need.
//!
//! # Meaning of the values
//!
//! - [`PairingMethod::QrCode`] is the QR pairing mode and
//!   [`PairingMethod::Sas`] is the short-authentication-string mode.
//! - [`PairingState::QrOffer`] is one active offer. `svg` is the code drawn
//!   from its `link2linux://pair?...` payload, which holds the pairing
//!   secret and is never sent by itself; offers last 5 minutes and are
//!   replaced by a new one shortly before they expire.
//! - [`PairingState::Searching`] lists phones whose mDNS record advertises
//!   `pm=1`. [`PhoneTarget::Address`] is the typed `IP:port` fallback.
//! - [`PairingState::SasCode`] carries the six digits, grouped for display by
//!   the UI. Each activation allows 3 SAS attempts; one counts when the phone
//!   reveals its nonce.
//! - [`Device`] comes from the pairing record plus the session state.
//!   `reconnecting` covers the first two minutes after a session is lost,
//!   `revoked` is a record the phone has told us it removed.

use std::fmt;

use serde::{Deserialize, Serialize};
use specta::Type;

pub use crate::clipboard::{
    ClipboardActivity, ClipboardDirection, ClipboardMode, ClipboardPhone, ClipboardPhoneState,
    ClipboardProblem, ClipboardProblemKind, ClipboardStatus, HeldClipboard,
};
pub use crate::notify::dto::{
    ConversationMessageInfo, ImageRef, MirroredNotification, NotificationActionInfo,
    NotificationActionKindView, NotificationCategoryView, NotificationContentView,
    NotificationEvent, NotificationImportanceView, NotificationProgressInfo,
    NotificationVisibilityView,
};

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
        /// The code as an SVG document, black on white so it scans in any theme.
        #[specta(type = String)]
        svg: QrImage,
        /// Which code of this activation it is, from 1. A new one replaces
        /// it shortly before it expires.
        offer_number: u8,
        /// The name the phone will see for this computer.
        desktop_name: String,
        /// Time left until this offer is replaced.
        expires_in_ms: u32,
        /// Length of the offer, for drawing progress.
        lifetime_ms: u32,
        /// Why the last connection the user asked for, to a typed address,
        /// ended. A failed connection does not end the offer.
        last_failure: Option<PairingFailure>,
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

/// A QR code drawn as an SVG document. It holds the pairing secret, so its
/// `Debug` output leaves it out.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct QrImage(pub String);

impl fmt::Debug for QrImage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("QrImage(..)")
    }
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
    /// A phone with the same id is paired under another key. Pairing never
    /// replaces it without asking, so the user removes it first.
    AlreadyPaired,
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
    /// A setting could not be stored. It applies until the app quits.
    #[error("The setting couldn't be saved, so it lasts only until the app quits.")]
    NotSaved,
    /// The runtime did not start, so nothing it does is available.
    #[error("link2linux didn't start, so this isn't available.")]
    NotStarted,
}

/// Why the app could not start, for the page shown instead of the app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct StartupError {
    /// What failed, in plain words.
    pub message: String,
    /// What the user can do about it.
    pub hint: String,
    /// The error itself, for a bug report.
    pub detail: String,
}

/// Where a backend reports changes. Called with the backend's lock held, so
/// events arrive in order; implementations must return quickly.
pub trait EventSink: Send + Sync {
    fn pairing_changed(&self, state: &PairingState);
    fn devices_changed(&self, devices: &[Device]);
    /// A phone's mirrored notifications changed.
    fn notification_changed(&self, _device_id: &str, _event: &NotificationEvent) {}
    /// The shared clipboard's state changed.
    fn clipboard_changed(&self, _status: &ClipboardStatus) {}
}
