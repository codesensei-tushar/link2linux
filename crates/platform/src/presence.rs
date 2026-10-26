//! Bluetooth LE presence: the types and the trait. The Linux
//! implementation is in [`crate::linux::ble`].

use std::future::Future;

use crate::PlatformError;

/// The 128-bit service UUID of link2linux, advertised over Bluetooth LE so
/// a phone can recognize the desktop.
pub const PRESENCE_SERVICE_UUID: &str = "d0135d8c-397d-4afe-b65f-bb4c33308f6d";

/// The longest device name the advertisement carries, in bytes. It must
/// fit alone in a scan response (31 bytes less the 2-byte header).
pub const MAX_ADVERTISED_NAME_BYTES: usize = 29;

/// Cuts `name` to at most [`MAX_ADVERTISED_NAME_BYTES`] bytes at a UTF-8
/// character boundary, because BlueZ would otherwise cut it at a byte.
#[must_use]
pub fn truncate_device_name(name: &str) -> &str {
    if name.len() <= MAX_ADVERTISED_NAME_BYTES {
        return name;
    }
    let mut end = MAX_ADVERTISED_NAME_BYTES;
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    &name[..end]
}

/// A Bluetooth device address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BluetoothAddress(pub [u8; 6]);

impl BluetoothAddress {
    /// The address as 12 uppercase hexadecimal digits, most significant
    /// first: the form of the QR code's `b` parameter.
    #[must_use]
    pub fn to_hex(&self) -> String {
        self.0.iter().map(|b| format!("{b:02X}")).collect()
    }
}

impl std::fmt::Display for BluetoothAddress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let [a, b, c, d, e, g] = self.0;
        write!(f, "{a:02X}:{b:02X}:{c:02X}:{d:02X}:{e:02X}:{g:02X}")
    }
}

/// Why a string is not a Bluetooth address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("not a Bluetooth address")]
pub struct ParseAddressError;

impl std::str::FromStr for BluetoothAddress {
    type Err = ParseAddressError;

    /// Parses `AA:BB:CC:DD:EE:FF` (either case) or 12 hexadecimal digits.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let digits: String = if text.len() == 17 {
            let parts: Vec<&str> = text.split(':').collect();
            if parts.len() != 6 || parts.iter().any(|p| p.len() != 2) {
                return Err(ParseAddressError);
            }
            parts.concat()
        } else {
            text.to_owned()
        };
        if digits.len() != 12 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(ParseAddressError);
        }
        let mut out = [0u8; 6];
        for (i, byte) in out.iter_mut().enumerate() {
            *byte =
                u8::from_str_radix(&digits[2 * i..2 * i + 2], 16).map_err(|_| ParseAddressError)?;
        }
        Ok(Self(out))
    }
}

/// Whether an adapter's address is its fixed one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddressKind {
    /// A public address, stable across restarts.
    Public,
    /// A random address, which may rotate. The phone cannot follow it.
    Random,
    /// BlueZ did not say (it reports the kind only from 5.64 on).
    Unknown,
}

/// The adapter that advertises the presence of the desktop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresenceAdapter {
    /// The adapter's address, for the QR code's `b` parameter.
    pub address: BluetoothAddress,
    /// Whether that address is public.
    pub address_kind: AddressKind,
    /// The adapter's name in the OS, such as `hci0`.
    pub name: String,
}

/// Advertises the link2linux service over Bluetooth LE, with no data flowing
/// over Bluetooth. A phone creates a companion-device association with the
/// desktop and asks its OS to watch for this advertisement.
///
/// The advertisement is connectable and uses the adapter's own address, so
/// it only works when that address is public; with a random address
/// [`start`](Self::start) fails with [`PlatformError::Unavailable`].
pub trait PresenceAdvertiser: Send + Sync {
    /// Starts advertising the desktop under `device_name` (cut to
    /// [`MAX_ADVERTISED_NAME_BYTES`]) and tells which adapter does it.
    /// Replaces an advertisement started earlier.
    fn start(
        &self,
        device_name: &str,
    ) -> impl Future<Output = Result<PresenceAdapter, PlatformError>> + Send;

    /// Stops advertising. Does nothing when not advertising.
    fn stop(&self) -> impl Future<Output = Result<(), PlatformError>> + Send;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_cut_at_character_boundaries() {
        assert_eq!(truncate_device_name("Büro"), "Büro");
        let long = "ü".repeat(20); // 40 bytes
        let cut = truncate_device_name(&long);
        assert_eq!(cut.len(), 28);
        assert!(cut.chars().all(|c| c == 'ü'));
        let ascii = "a".repeat(40);
        assert_eq!(truncate_device_name(&ascii).len(), 29);
        assert_eq!(truncate_device_name(""), "");
    }

    #[test]
    fn addresses_parse_and_print() {
        let a: BluetoothAddress = "0c:1A:2b:3c:4d:5e".parse().unwrap();
        assert_eq!(a.0, [0x0c, 0x1a, 0x2b, 0x3c, 0x4d, 0x5e]);
        assert_eq!(a.to_string(), "0C:1A:2B:3C:4D:5E");
        assert_eq!(a.to_hex(), "0C1A2B3C4D5E");
        assert_eq!("0C1A2B3C4D5E".parse::<BluetoothAddress>(), Ok(a));
        for bad in [
            "",
            "0C:1A:2B:3C:4D",
            "0C1A2B3C4D5",
            "0C:1A:2B:3C:4D:5G",
            "0C-1A-2B-3C-4D-5E",
        ] {
            assert!(bad.parse::<BluetoothAddress>().is_err(), "{bad}");
        }
    }
}
