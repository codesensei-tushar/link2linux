//! What the calls page receives: a snapshot of the desktop's part in calls
//! and each paired phone's calls.
//!
//! Numbers and names are shown to the user, so they are in the snapshot,
//! but the types' `Debug` leaves them out, as logs must.

use serde::{Deserialize, Serialize};
use specta::Type;

/// Calls on this desktop, as a snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CallsStatus {
    /// Whether this desktop can take calls, and why not.
    pub desktop: CallsDesktop,
    /// The user's "Use this computer for calls" switch.
    pub enabled: bool,
    /// Bluetooth pairing, while the user pairs a phone.
    pub pairing: Option<BluetoothPairing>,
    /// Every paired phone, in pairing order.
    pub phones: Vec<PhoneCalls>,
}

/// Whether this desktop can take calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum CallsDesktop {
    /// It can.
    Ready,
    /// The user turned calls off on this desktop.
    Disabled,
    /// This build has no calls for this OS.
    Unsupported,
    /// The desktop has no Bluetooth adapter.
    NoAdapter,
    /// Bluetooth is turned off.
    BluetoothOff,
    /// The desktop's audio system offers no hands-free call control: it
    /// needs PipeWire 1.4 or later with its native Bluetooth backend.
    NoCallControl,
    /// Still looking.
    Checking,
}

/// The pairing window the user opened.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct BluetoothPairing {
    /// The phone it is for.
    pub device_id: String,
    /// The name the phone looks for in its Bluetooth list.
    pub adapter_name: String,
    /// The desktop's Bluetooth address.
    pub adapter_address: String,
    /// When the window closes (Unix time, milliseconds).
    #[specta(type = specta_typescript::Number)]
    pub ends_unix_ms: i64,
    /// What the phone made of the request to pair.
    pub phone: PairingRequestState,
    /// A device that asks to pair and shows a code, for the user to
    /// confirm.
    pub prompt: Option<PairingCode>,
}

impl std::fmt::Debug for BluetoothPairing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BluetoothPairing")
            .field("device_id", &self.device_id)
            .field("phone", &self.phone)
            .field("prompt", &self.prompt.as_ref().map(|p| p.id))
            .finish_non_exhaustive()
    }
}

/// Whether the phone was asked to pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum PairingRequestState {
    /// Not yet: the phone is not connected, or calls are not ready on it.
    NotAsked,
    /// Asked; no answer yet.
    Waiting,
    /// The phone shows its user a prompt to pair.
    Asked,
    /// The phone has no Bluetooth, or turned calls off.
    Unavailable,
    /// The phone could not ask its user.
    Failed,
}

/// A device that asks to pair, with the code both devices show.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PairingCode {
    /// Identifies the prompt when the user answers it.
    pub id: u32,
    /// The device's Bluetooth name.
    pub device_name: String,
    /// The six digits.
    pub code: String,
}

/// A paired phone and its calls.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PhoneCalls {
    pub device_id: String,
    pub phone_name: String,
    /// The phone's hands-free link with this desktop.
    pub link: CallLink,
    /// Its calls, the hands-free link's first.
    pub calls: Vec<CallView>,
    /// What went wrong last, until it works.
    pub problem: Option<CallProblem>,
}

impl std::fmt::Debug for PhoneCalls {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PhoneCalls")
            .field("device_id", &self.device_id)
            .field("link", &self.link)
            .field("calls", &self.calls)
            .field("problem", &self.problem)
            .finish_non_exhaustive()
    }
}

/// A Bluetooth device the phone may be.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct BluetoothChoice {
    /// Its address, `AA:BB:CC:DD:EE:FF`.
    pub address: String,
    /// Its Bluetooth name.
    pub name: String,
}

impl std::fmt::Debug for BluetoothChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BluetoothChoice")
            .field("address", &self.address)
            .finish_non_exhaustive()
    }
}

/// A phone's hands-free link with this desktop.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum CallLink {
    /// Calls cannot be used on this desktop now; see
    /// [`CallsStatus::desktop`].
    Off,
    /// The phone is not paired with this desktop over Bluetooth.
    NotPaired,
    /// Several paired devices may be the phone: the user picks one.
    Choose { candidates: Vec<BluetoothChoice> },
    /// Paired, but the hands-free link is down.
    Disconnected { device: BluetoothChoice },
    /// The desktop is connecting the hands-free link.
    Connecting { device: BluetoothChoice },
    /// The hands-free link is up: calls come to this desktop.
    #[serde(rename_all = "camelCase")]
    Connected {
        device: BluetoothChoice,
        /// The desktop can mute its microphone in calls.
        can_mute: bool,
        muted: bool,
    },
}

/// A call as the calls page shows it.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CallView {
    /// Identifies the call in the commands that act on it.
    pub id: String,
    /// The other party's number, when known.
    pub number: String,
    /// The other party's name, when known.
    pub name: String,
    pub state: CallStateView,
    /// The phone app whose call it is, for calls of apps other than the
    /// phone's dialer, which come only as a notification.
    pub app_name: Option<String>,
    /// The call's audio is on this desktop when answered here; otherwise
    /// it stays on the phone.
    pub audio_here: bool,
    pub can_answer: bool,
    pub can_decline: bool,
    pub can_hang_up: bool,
    /// Touch tones can be sent in it.
    pub can_send_tones: bool,
    /// When the call became active (Unix time, milliseconds), for its
    /// timer.
    #[specta(type = Option<specta_typescript::Number>)]
    pub active_since_unix_ms: Option<i64>,
}

impl std::fmt::Debug for CallView {
    // Numbers and names are the user's contacts: never logged.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CallView")
            .field("id", &self.id)
            .field("state", &self.state)
            .field("audio_here", &self.audio_here)
            .finish_non_exhaustive()
    }
}

/// Where a call is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum CallStateView {
    /// It rings.
    Incoming,
    /// It rings while another call is on.
    Waiting,
    /// An outgoing call is being set up.
    Dialing,
    /// An outgoing call rings at the other end.
    Alerting,
    Active,
    Held,
}

/// What went wrong with a phone's calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum CallProblem {
    /// The hands-free link could not be connected.
    ConnectFailed,
    /// Pairing could not start, as when Bluetooth turned off.
    PairingFailed,
    /// The pairing window closed before the phone paired.
    PairingTimedOut,
    /// A call command failed, such as dialing.
    CommandFailed,
}
