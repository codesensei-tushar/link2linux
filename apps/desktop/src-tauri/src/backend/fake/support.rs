//! Made-up phones, offers and address checks for the fake backend.

use std::net::{IpAddr, SocketAddr};

use super::script::Now;
use crate::backend::{BackendError, Device, DeviceStatus, DiscoveredPhone};

pub const PHONE_NAME: &str = "Pixel 9";

pub fn pretend_phones() -> Vec<DiscoveredPhone> {
    vec![
        DiscoveredPhone {
            id: "l2l-3f9a12bc".to_owned(),
            name: "Pixel 9".to_owned(),
            address: "192.168.1.42:27380".to_owned(),
        },
        DiscoveredPhone {
            id: "l2l-77c0de01".to_owned(),
            name: "Galaxy S24".to_owned(),
            address: "192.168.1.57:27380".to_owned(),
        },
    ]
}

pub fn demo_device(
    id: &str,
    name: &str,
    model: &str,
    status: DeviceStatus,
    now: Now,
    age_ms: i64,
) -> Device {
    Device {
        id: id.to_owned(),
        name: name.to_owned(),
        model: Some(model.to_owned()),
        status,
        paired_at_unix_ms: now.unix_ms - age_ms,
        last_connected_unix_ms: Some(match status {
            DeviceStatus::Connected => now.unix_ms,
            _ => now.back(3_600_000).unix_ms,
        }),
    }
}

/// Checks a typed address and names the phone behind it.
pub fn typed_phone(address: &str) -> Result<String, BackendError> {
    let address = address.trim();
    let ip = match address.parse::<SocketAddr>() {
        Ok(socket) if socket.port() != 0 => socket.ip(),
        Ok(_) => return Err(BackendError::InvalidAddress),
        Err(_) => address
            .parse::<IpAddr>()
            .map_err(|_| BackendError::InvalidAddress)?,
    };
    if ip.is_unspecified() || ip.is_multicast() {
        return Err(BackendError::InvalidAddress);
    }
    Ok(PHONE_NAME.to_owned())
}

/// A QR payload with the right shape and made-up key and secret. Offers with
/// the same `seed` are identical, which keeps tests stable.
pub fn offer_uri(seed: u64, desktop_name: &str) -> String {
    let fingerprint = base64url_chars(seed, 43);
    let secret = base64url_chars(seed ^ 0x9e37_79b9_7f4a_7c15, 22);
    format!(
        "link2linux://pair?v=1&f={fingerprint}&s={secret}&n={}&a=192.168.1.20",
        percent_encode(desktop_name),
    )
}

fn base64url_chars(seed: u64, len: usize) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut state = seed;
    (0..len)
        .map(|_| {
            // splitmix64
            state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut z = state;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            z ^= z >> 31;
            char::from(ALPHABET[usize::try_from(z % 64).unwrap_or(0)])
        })
        .collect()
}

fn percent_encode(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// The computer's name, shown on the phone before it connects.
pub fn desktop_name() -> String {
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
        .or_else(|| {
            std::env::var("HOSTNAME")
                .ok()
                .filter(|name| !name.is_empty())
        })
        .unwrap_or_else(|| "This computer".to_owned())
}
