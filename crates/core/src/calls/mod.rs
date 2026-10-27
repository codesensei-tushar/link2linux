//! Phone calls on the desktop (`docs/protocol.md`, "Calls").
//!
//! Calls do not travel over the session. The phone is the audio gateway of
//! the Bluetooth Hands-Free Profile and the desktop its hands-free unit, so
//! the phone's OS and the desktop's Bluetooth and audio stacks carry the
//! audio and the call control (the `Bluetooth` and `Telephony` traits of
//! `link2linux-platform`). The session only helps the user pair the two
//! devices over Bluetooth. What is here:
//!
//! - [`phone`]: which of the desktop's paired Bluetooth devices is a
//!   paired phone;
//! - [`pairing`]: the 2-minute window in which the desktop accepts
//!   Bluetooth pairing, and only after the user confirmed the code;
//! - [`merge`]: one list of calls from the hands-free link and the phone's
//!   call notifications, and what answering or ending each one takes;
//! - [`CallsService`]: the feature in one phone's session, which reports
//!   the desktop's availability and asks the phone to pair.

pub mod merge;
pub mod pairing;
pub mod phone;
mod service;

pub use merge::{CallAction, CallNotification, Control, Operation, ShownCall, merge, operation};
pub use pairing::{PAIRING_WINDOW, PairingWindow, PhoneAnswer};
pub use phone::{PhoneDevice, find_phone};
pub use service::{CallsService, Handled};

/// The longest number the hands-free link dials.
pub const MAX_NUMBER_LEN: usize = 80;
/// The most touch tones sent at once.
pub const MAX_TONES_LEN: usize = 32;

/// `input` as a number the phone can dial over the hands-free link: the
/// characters `0-9 + * # ,` and `A-D`, at most [`MAX_NUMBER_LEN`] of them.
/// The spaces, dashes, dots and brackets people write numbers with are
/// dropped. `None` when nothing dialable is left, or anything else is in
/// it.
#[must_use]
pub fn dialable(input: &str) -> Option<String> {
    let mut number = String::with_capacity(input.len());
    for c in input.chars() {
        match c {
            ' ' | '-' | '.' | '(' | ')' | '/' => {}
            '0'..='9' | '+' | '*' | '#' | ',' | 'A'..='D' => number.push(c),
            'a'..='d' => number.push(c.to_ascii_uppercase()),
            _ => return None,
        }
    }
    (!number.is_empty() && number.len() <= MAX_NUMBER_LEN).then_some(number)
}

/// Whether `tones` are touch tones to send in a call: `0-9 * #` and
/// `A-D`, at most [`MAX_TONES_LEN`] of them.
#[must_use]
pub fn are_tones(tones: &str) -> bool {
    !tones.is_empty()
        && tones.len() <= MAX_TONES_LEN
        && tones
            .bytes()
            .all(|b| b.is_ascii_digit() || matches!(b, b'*' | b'#' | b'A'..=b'D'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_are_cleaned_up_for_dialing() {
        assert_eq!(
            dialable("+1 (555) 010-0.123").as_deref(),
            Some("+15550100123")
        );
        assert_eq!(dialable("*31#0800,1").as_deref(), Some("*31#0800,1"));
        assert_eq!(dialable("abcd").as_deref(), Some("ABCD"));
        assert_eq!(dialable(""), None);
        assert_eq!(dialable(" - "), None);
        assert_eq!(dialable("555 0100 e"), None);
        assert_eq!(dialable("0800\n1"), None);
        assert_eq!(dialable(&"1".repeat(80)).map(|n| n.len()), Some(80));
        assert_eq!(dialable(&"1".repeat(81)), None);
    }

    #[test]
    fn tones_are_keypad_keys() {
        assert!(are_tones("0123456789*#ABCD"));
        assert!(!are_tones(""));
        assert!(!are_tones("1,2"));
        assert!(!are_tones("+"));
        assert!(!are_tones("a"));
        assert!(!are_tones(&"1".repeat(33)));
    }
}
