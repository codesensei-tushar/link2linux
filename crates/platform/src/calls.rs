//! Phone calls over Bluetooth: the phone is the audio gateway of the
//! Hands-Free Profile (HFP) and the desktop the hands-free unit, as a car
//! kit is.
//!
//! Two OS services take part, each behind a trait here:
//!
//! - [`Bluetooth`]: the adapter and the devices paired with it, the
//!   window in which a phone may pair, and connecting the hands-free
//!   profile;
//! - [`Telephony`]: the calls of each connected audio gateway, and the
//!   commands that answer, end and place them.
//!
//! The Linux implementations are in [`crate::linux::calls`].

use std::time::Duration;

use crate::{BluetoothAddress, PlatformError};

/// The service class of the Hands-Free Profile's audio gateway, which a
/// phone offers and the desktop connects to.
pub const HANDS_FREE_GATEWAY_UUID: &str = "0000111f-0000-1000-8000-00805f9b34fb";

/// The desktop's Bluetooth adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BluetoothAdapter {
    /// Its address, which phones pair with.
    pub address: BluetoothAddress,
    /// The name nearby devices see.
    pub name: String,
    /// Whether it is switched on.
    pub powered: bool,
}

/// A Bluetooth device the desktop knows, paired or only seen nearby.
#[derive(Clone, PartialEq, Eq)]
pub struct BluetoothDevice {
    /// Its address.
    pub address: BluetoothAddress,
    /// Its name, as the user renamed it or as it calls itself.
    pub name: String,
    /// Paired (bonded) with the desktop.
    pub paired: bool,
    /// Allowed to connect without asking the user.
    pub trusted: bool,
    /// Connected now, through any profile.
    pub connected: bool,
    /// It says it is a phone, by its device class or icon.
    pub phone: bool,
    /// It offers the hands-free audio gateway, as phones do.
    pub hands_free_gateway: bool,
}

impl std::fmt::Debug for BluetoothDevice {
    // Device names are often the owner's ("Anna's phone"): not logged.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BluetoothDevice")
            .field("address", &self.address)
            .field("name_bytes", &self.name.len())
            .field("paired", &self.paired)
            .field("trusted", &self.trusted)
            .field("connected", &self.connected)
            .field("phone", &self.phone)
            .field("hands_free_gateway", &self.hands_free_gateway)
            .finish()
    }
}

/// A device asks to pair, and shows `code`: the user confirms that both
/// devices show the same one (Bluetooth numeric comparison).
#[derive(Clone, PartialEq, Eq)]
pub struct PairingPrompt {
    /// Identifies the prompt in [`Bluetooth::answer_pairing`].
    pub id: u64,
    /// The device that asks.
    pub device: BluetoothAddress,
    /// Its name, when the desktop knows it.
    pub device_name: String,
    /// The 6-digit code, 0 to 999 999.
    pub code: u32,
}

impl std::fmt::Debug for PairingPrompt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PairingPrompt")
            .field("id", &self.id)
            .field("device", &self.device)
            .field("name_bytes", &self.device_name.len())
            .finish_non_exhaustive()
    }
}

/// What happens while pairing is open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairingEvent {
    /// A device asks the user to confirm a code.
    Confirm(PairingPrompt),
    /// The prompt is over without an answer: the other device gave up, or
    /// no answer came in time.
    Cancelled {
        /// The prompt's [`PairingPrompt::id`].
        id: u64,
    },
}

/// The desktop's Bluetooth stack, for calls.
///
/// Pairing is open only between [`open_pairing`](Self::open_pairing) and
/// [`close_pairing`](Self::close_pairing). Outside it the desktop answers
/// no pairing request itself, and while it is open it accepts one only
/// when [`answer_pairing`](Self::answer_pairing) says so: the user
/// confirmed the code.
pub trait Bluetooth: Send + Sync {
    /// The adapter calls use, or `None` without one.
    fn adapter(
        &self,
    ) -> impl Future<Output = Result<Option<BluetoothAdapter>, PlatformError>> + Send;

    /// The devices the adapter knows.
    fn devices(&self) -> impl Future<Output = Result<Vec<BluetoothDevice>, PlatformError>> + Send;

    /// Resolves the next time the adapter or one of its devices changes:
    /// callers read [`adapter`](Self::adapter) and
    /// [`devices`](Self::devices) again.
    fn changed(&self) -> impl Future<Output = Result<(), PlatformError>> + Send;

    /// Makes the adapter discoverable and pairable for `window`, and takes
    /// the pairing requests that come meanwhile. Returns the adapter,
    /// whose name and address the phone looks for.
    fn open_pairing(
        &self,
        window: Duration,
    ) -> impl Future<Output = Result<BluetoothAdapter, PlatformError>> + Send;

    /// Ends what [`open_pairing`](Self::open_pairing) started: the adapter
    /// is discoverable and pairable as before, and prompts still open are
    /// refused. Does nothing when pairing is not open.
    fn close_pairing(&self) -> impl Future<Output = Result<(), PlatformError>> + Send;

    /// Waits for the next [`PairingEvent`]. Never resolves while pairing
    /// is closed.
    fn next_pairing_event(&self) -> impl Future<Output = PairingEvent> + Send;

    /// Accepts or refuses prompt `id`. Prompts that are over are ignored.
    fn answer_pairing(&self, id: u64, accept: bool);

    /// Trusts the paired `device`, so it connects without asking, and
    /// connects its hands-free audio gateway, and only that: the phone's
    /// media audio stays on the phone.
    fn connect_hands_free(
        &self,
        device: BluetoothAddress,
    ) -> impl Future<Output = Result<(), PlatformError>> + Send;

    /// Disconnects `device`'s hands-free audio gateway, so the phone keeps
    /// its calls to itself.
    fn disconnect_hands_free(
        &self,
        device: BluetoothAddress,
    ) -> impl Future<Output = Result<(), PlatformError>> + Send;
}

/// Where a call is, as the phone reports it over Bluetooth.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CallState {
    /// Someone calls, and the phone rings.
    Incoming,
    /// Someone calls while another call is on.
    Waiting,
    /// An outgoing call is being set up.
    Dialing,
    /// An outgoing call rings at the other end.
    Alerting,
    /// Both sides talk.
    Active,
    /// On hold.
    Held,
    /// Over; it goes away shortly.
    Disconnected,
}

impl CallState {
    /// The state named `name` in the telephony service's terms.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "incoming" => Self::Incoming,
            "waiting" => Self::Waiting,
            "dialing" => Self::Dialing,
            "alerting" => Self::Alerting,
            "active" => Self::Active,
            "held" => Self::Held,
            "disconnected" => Self::Disconnected,
            _ => return None,
        })
    }

    /// Someone calls and nobody answered yet.
    #[must_use]
    pub fn is_ringing(self) -> bool {
        matches!(self, Self::Incoming | Self::Waiting)
    }
}

/// A call of an audio gateway.
#[derive(Clone, PartialEq, Eq)]
pub struct GatewayCall {
    /// Identifies the call while it lasts.
    pub id: String,
    /// The other party's number, or empty when the phone withholds it.
    pub number: String,
    /// The other party's name, when the phone sends one.
    pub name: String,
    /// Where the call is.
    pub state: CallState,
    /// Part of a conference.
    pub multiparty: bool,
}

impl std::fmt::Debug for GatewayCall {
    // Numbers and names are the user's contacts: never logged.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GatewayCall")
            .field("id", &self.id)
            .field("number_bytes", &self.number.len())
            .field("name_bytes", &self.name.len())
            .field("state", &self.state)
            .field("multiparty", &self.multiparty)
            .finish()
    }
}

/// A phone connected as an audio gateway, with its calls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gateway {
    /// Identifies the gateway while it stays connected.
    pub id: String,
    /// The phone's Bluetooth address, when the service reports it.
    pub address: Option<BluetoothAddress>,
    /// Its calls, oldest first.
    pub calls: Vec<GatewayCall>,
    /// Whether the desktop's microphone is muted for its calls, or `None`
    /// when the service cannot mute.
    pub muted: Option<bool>,
}

/// The desktop's hands-free call control.
pub trait Telephony: Send + Sync {
    /// The connected audio gateways. Fails with
    /// [`PlatformError::Unavailable`] when the desktop offers no call
    /// control.
    fn gateways(&self) -> impl Future<Output = Result<Vec<Gateway>, PlatformError>> + Send;

    /// Resolves the next time a gateway or call changes, or the service
    /// comes or goes: callers read [`gateways`](Self::gateways) again.
    fn changed(&self) -> impl Future<Output = Result<(), PlatformError>> + Send;

    /// Answers the ringing call `call`.
    fn answer(&self, call: &str) -> impl Future<Output = Result<(), PlatformError>> + Send;

    /// Ends call `call`, or declines it while it rings.
    fn hang_up(&self, call: &str) -> impl Future<Output = Result<(), PlatformError>> + Send;

    /// Calls `number` from `gateway`.
    fn dial(
        &self,
        gateway: &str,
        number: &str,
    ) -> impl Future<Output = Result<(), PlatformError>> + Send;

    /// Sends the touch tones `tones` in `gateway`'s active call.
    fn send_tones(
        &self,
        gateway: &str,
        tones: &str,
    ) -> impl Future<Output = Result<(), PlatformError>> + Send;

    /// Mutes or unmutes the desktop's microphone in `gateway`'s calls,
    /// where [`Gateway::muted`] says the service can.
    fn set_muted(
        &self,
        gateway: &str,
        muted: bool,
    ) -> impl Future<Output = Result<(), PlatformError>> + Send;
}

/// How long a pairing prompt waits for the user before it is refused.
pub const PROMPT_TIMEOUT: Duration = Duration::from_secs(60);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn call_states_parse_the_service_names() {
        for (name, state) in [
            ("incoming", CallState::Incoming),
            ("waiting", CallState::Waiting),
            ("dialing", CallState::Dialing),
            ("alerting", CallState::Alerting),
            ("active", CallState::Active),
            ("held", CallState::Held),
            ("disconnected", CallState::Disconnected),
        ] {
            assert_eq!(CallState::from_name(name), Some(state));
        }
        assert_eq!(CallState::from_name("Active"), None);
        assert!(CallState::Waiting.is_ringing());
        assert!(!CallState::Held.is_ringing());
    }

    #[test]
    fn debug_output_leaves_numbers_and_names_out() {
        let call = GatewayCall {
            id: "/ag0/call0".into(),
            number: "+15550100".into(),
            name: "Anna".into(),
            state: CallState::Incoming,
            multiparty: false,
        };
        let device = BluetoothDevice {
            address: "00:11:22:33:44:55".parse().unwrap(),
            name: "Anna's phone".into(),
            paired: true,
            trusted: true,
            connected: false,
            phone: true,
            hands_free_gateway: true,
        };
        let prompt = PairingPrompt {
            id: 1,
            device: device.address,
            device_name: "Anna's phone".into(),
            code: 123_456,
        };
        let logged = format!("{call:?} {device:?} {prompt:?}");
        for secret in ["5550100", "Anna", "123456"] {
            assert!(!logged.contains(secret), "{logged}");
        }
    }
}
