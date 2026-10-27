//! The window in which the desktop accepts Bluetooth pairing
//! (`docs/protocol.md`, "Pairing over Bluetooth").
//!
//! The user opens it for one paired phone. For [`PAIRING_WINDOW`] the
//! adapter is discoverable, and a device that asks to pair waits until the
//! user confirmed its code on the desktop: a pairing is accepted only
//! inside the window and only after that confirmation (implementation
//! requirement 12). Everything here is a pure function of the time it is
//! given, so the rules are tested without a clock or a Bluetooth stack.

use std::time::Duration;

use link2linux_platform::{BluetoothAdapter, BluetoothAddress, BluetoothDevice, PairingPrompt};
use link2linux_protocol::v1::{BluetoothPairingRequest, ErrorCode};
use tokio::time::Instant;

/// How long the desktop accepts pairing once the user opened it.
pub const PAIRING_WINDOW: Duration = Duration::from_secs(120);
/// The longest adapter name `BluetoothPairingRequest` carries, in bytes.
pub const MAX_NAME_BYTES: usize = 248;

/// What the phone made of the desktop's `BluetoothPairingRequest`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhoneAnswer {
    /// Not sent yet: the phone is not connected, or calls are not ready
    /// on both devices.
    NotAsked,
    /// Sent; no answer yet.
    Waiting,
    /// The phone asked its user to pair.
    Asked,
    /// The phone refused, for example because it has no Bluetooth.
    Refused(ErrorCode),
    /// No answer came: the request timed out, or the session ended first.
    NoAnswer,
}

/// The pairing window the user opened for one phone.
#[derive(Debug, Clone)]
pub struct PairingWindow {
    device_id: String,
    adapter: BluetoothAdapter,
    deadline: Instant,
    /// Prompts that wait for the user, oldest first.
    prompts: Vec<PairingPrompt>,
    /// The device whose code the user confirmed.
    confirmed: Option<BluetoothAddress>,
    phone: PhoneAnswer,
}

impl PairingWindow {
    /// The window for phone `device_id`, open from `now` on, in which
    /// devices pair with `adapter`.
    #[must_use]
    pub fn open(device_id: impl Into<String>, adapter: BluetoothAdapter, now: Instant) -> Self {
        Self {
            device_id: device_id.into(),
            adapter,
            deadline: now + PAIRING_WINDOW,
            prompts: Vec::new(),
            confirmed: None,
            phone: PhoneAnswer::NotAsked,
        }
    }

    /// The phone the user opened the window for.
    #[must_use]
    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    /// The adapter devices pair with.
    #[must_use]
    pub fn adapter(&self) -> &BluetoothAdapter {
        &self.adapter
    }

    /// When the window closes by itself.
    #[must_use]
    pub fn deadline(&self) -> Instant {
        self.deadline
    }

    /// Whether pairing is accepted at `now`.
    #[must_use]
    pub fn is_open(&self, now: Instant) -> bool {
        now < self.deadline
    }

    /// The prompt to show the user: the oldest one that waits.
    #[must_use]
    pub fn prompt(&self) -> Option<&PairingPrompt> {
        self.prompts.first()
    }

    /// A device asks to pair. Returns the prompt's id when it is to be
    /// refused at once, because the window is closed.
    pub fn prompted(&mut self, prompt: PairingPrompt, now: Instant) -> Option<u64> {
        if !self.is_open(now) {
            return Some(prompt.id);
        }
        self.prompts.retain(|p| p.id != prompt.id);
        self.prompts.push(prompt);
        None
    }

    /// Prompt `id` is over without the user's answer.
    pub fn cancelled(&mut self, id: u64) {
        self.prompts.retain(|p| p.id != id);
    }

    /// The user answered prompt `id`: whether the code matches. Returns
    /// the answer for the Bluetooth stack, which is a yes only for a
    /// prompt that waits, while the window is open, and when the user said
    /// yes; `None` for a prompt that is over.
    pub fn answer(&mut self, id: u64, matches: bool, now: Instant) -> Option<bool> {
        let index = self.prompts.iter().position(|p| p.id == id)?;
        let prompt = self.prompts.remove(index);
        let accept = matches && self.is_open(now);
        if accept {
            self.confirmed = Some(prompt.device);
        }
        Some(accept)
    }

    /// The device the user confirmed, if one is now paired: pairing is
    /// done.
    #[must_use]
    pub fn paired(&self, devices: &[BluetoothDevice]) -> Option<BluetoothAddress> {
        let confirmed = self.confirmed?;
        devices
            .iter()
            .any(|d| d.address == confirmed && d.paired)
            .then_some(confirmed)
    }

    /// The prompts that still wait, which closing the window refuses.
    #[must_use]
    pub fn waiting(&self) -> Vec<u64> {
        self.prompts.iter().map(|p| p.id).collect()
    }

    /// What the phone made of the pairing request.
    #[must_use]
    pub fn phone_answer(&self) -> PhoneAnswer {
        self.phone
    }

    /// Records what the phone made of the pairing request.
    pub fn set_phone_answer(&mut self, answer: PhoneAnswer) {
        self.phone = answer;
    }

    /// The request that asks the phone to pair: the adapter's name, cut to
    /// [`MAX_NAME_BYTES`], and address.
    #[must_use]
    pub fn request(&self) -> BluetoothPairingRequest {
        let address = self.adapter.address.to_string();
        let name = truncate(self.adapter.name.trim(), MAX_NAME_BYTES);
        BluetoothPairingRequest {
            name: if name.is_empty() {
                address.clone()
            } else {
                name.to_owned()
            },
            address,
        }
    }
}

/// `text` cut to at most `max` bytes at a character boundary.
fn truncate(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn adapter(name: &str) -> BluetoothAdapter {
        BluetoothAdapter {
            address: "A0:B1:C2:D3:E4:F5".parse().unwrap(),
            name: name.into(),
            powered: true,
        }
    }

    fn prompt(id: u64, device: &str) -> PairingPrompt {
        PairingPrompt {
            id,
            device: device.parse().unwrap(),
            device_name: "Pixel 8".into(),
            code: 123_456,
        }
    }

    fn device(address: &str, paired: bool) -> BluetoothDevice {
        BluetoothDevice {
            address: address.parse().unwrap(),
            name: "Pixel 8".into(),
            paired,
            trusted: false,
            connected: false,
            phone: true,
            hands_free_gateway: true,
        }
    }

    const PHONE: &str = "0C:1A:2B:3C:4D:5E";
    const OTHER: &str = "00:11:22:33:44:55";

    #[test]
    fn a_confirmed_code_is_accepted_and_the_pairing_seen_through() {
        let now = Instant::now();
        let mut window = PairingWindow::open("phone-1", adapter("workstation"), now);
        assert_eq!(window.device_id(), "phone-1");
        assert!(window.is_open(now));
        assert_eq!(window.prompted(prompt(1, PHONE), now), None);
        assert_eq!(window.prompt().unwrap().code, 123_456);
        assert_eq!(window.answer(1, true, now), Some(true));
        assert_eq!(window.prompt(), None);
        // Done once the confirmed device is paired, not before.
        assert_eq!(window.paired(&[device(PHONE, false)]), None);
        assert_eq!(
            window.paired(&[device(OTHER, true), device(PHONE, true)]),
            Some(PHONE.parse().unwrap())
        );
    }

    #[test]
    fn a_code_that_does_not_match_is_refused() {
        let now = Instant::now();
        let mut window = PairingWindow::open("phone-1", adapter("x"), now);
        window.prompted(prompt(1, PHONE), now);
        assert_eq!(window.answer(1, false, now), Some(false));
        assert_eq!(window.paired(&[device(PHONE, true)]), None);
        // A prompt answered already is over.
        assert_eq!(window.answer(1, true, now), None);
    }

    #[test]
    fn nothing_is_accepted_outside_the_window() {
        let now = Instant::now();
        let mut window = PairingWindow::open("phone-1", adapter("x"), now);
        window.prompted(prompt(1, PHONE), now);
        let late = now + PAIRING_WINDOW;
        assert!(!window.is_open(late));
        assert_eq!(window.deadline(), late);
        // The user says yes after the window closed: refused.
        assert_eq!(window.answer(1, true, late), Some(false));
        assert_eq!(window.paired(&[device(PHONE, true)]), None);
        // A device that asks after the window closed is refused at once.
        assert_eq!(window.prompted(prompt(2, OTHER), late), Some(2));
        assert_eq!(window.prompt(), None);
        assert_eq!(window.answer(2, true, late), None);
    }

    #[test]
    fn prompts_wait_in_order_until_answered_or_cancelled() {
        let now = Instant::now();
        let mut window = PairingWindow::open("phone-1", adapter("x"), now);
        window.prompted(prompt(1, PHONE), now);
        window.prompted(prompt(2, OTHER), now);
        assert_eq!(window.waiting(), [1, 2]);
        window.cancelled(1);
        assert_eq!(window.prompt().unwrap().id, 2);
        assert_eq!(window.waiting(), [2]);
        assert_eq!(window.answer(1, true, now), None);
    }

    #[test]
    fn the_request_carries_the_adapter_name_and_address() {
        let now = Instant::now();
        let mut window = PairingWindow::open("phone-1", adapter(" workstation "), now);
        let request = window.request();
        assert_eq!(request.name, "workstation");
        assert_eq!(request.address, "A0:B1:C2:D3:E4:F5");
        assert_eq!(window.phone_answer(), PhoneAnswer::NotAsked);
        window.set_phone_answer(PhoneAnswer::Asked);
        assert_eq!(window.phone_answer(), PhoneAnswer::Asked);

        let long = PairingWindow::open("phone-1", adapter(&"é".repeat(200)), now);
        let name = long.request().name;
        assert_eq!(name.len(), 248);
        assert_eq!(name, "é".repeat(124));
        let unnamed = PairingWindow::open("phone-1", adapter(""), now);
        assert_eq!(unnamed.request().name, "A0:B1:C2:D3:E4:F5");
    }
}
