//! The QR code the desktop shows to pair.
//!
//! The code holds a URI with the desktop's fingerprint, a one-time
//! [`PairingSecret`], its name, the addresses the phone sends beacons to,
//! and optionally its Bluetooth address:
//!
//! ```text
//! link2linux://pair?v=1&f=<fp_D>&s=<secret>&n=<name>&a=<addresses>&b=<address>
//! ```
//!
//! The pairing tag and pairing beacons are public checks of a guess at the
//! secret, which is why it is 16 random bytes and never typed or derived
//! from a code.

use std::net::{IpAddr, SocketAddr};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};

use super::crypto::PairingSecret;
use crate::identity::{CertificateFingerprint, decode_hex};

/// Everything before the parameters of a QR code URI.
pub const QR_URI_PREFIX: &str = "link2linux://pair?";
/// The payload format version this module writes and reads (`v`).
pub const QR_PAYLOAD_VERSION: &str = "1";
/// UDP port the desktop listens on for beacons, assumed for addresses in
/// the code that carry no port.
pub const DEFAULT_BEACON_PORT: u16 = 27381;
/// Most addresses a code carries.
pub const MAX_QR_ADDRESSES: usize = 4;
/// Longest device name a code carries, in bytes before encoding.
pub const MAX_QR_NAME_LEN: usize = 64;
/// Characters left as they are in the name: RFC 3986 unreserved.
const NAME_ENCODE_SET: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

/// The content of a QR code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QrPayload {
    /// The desktop's fingerprint (`f`).
    pub desktop: CertificateFingerprint,
    /// The one-time secret (`s`).
    pub secret: PairingSecret,
    /// The desktop's device name (`n`).
    pub name: String,
    /// Where the phone sends beacons (`a`). Port [`DEFAULT_BEACON_PORT`] is
    /// left out of the URI, and IPv6 link-local addresses are never written:
    /// the phone could not know their interface.
    pub addresses: Vec<SocketAddr>,
    /// The desktop's Bluetooth LE address, most significant byte first
    /// (`b`), when it advertises.
    pub bluetooth_address: Option<[u8; 6]>,
}

/// Why a QR code URI could not be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum QrError {
    /// Not a `link2linux://pair?` URI.
    #[error("not a link2linux pairing code")]
    NotAPairingCode,
    /// A payload version this build does not know; the app needs an update.
    #[error("unsupported pairing code version {0:?}")]
    UnsupportedVersion(String),
    /// A required parameter is missing.
    #[error("the pairing code has no {0:?} parameter")]
    Missing(&'static str),
    /// A parameter does not decode.
    #[error("malformed {0:?} parameter in the pairing code")]
    Malformed(&'static str),
}

impl QrPayload {
    /// The URI to put in the QR code.
    ///
    /// The name is cut to [`MAX_QR_NAME_LEN`] bytes at a character boundary,
    /// IPv6 link-local addresses are skipped, and only the first
    /// [`MAX_QR_ADDRESSES`] of the rest are written.
    #[must_use]
    pub fn to_uri(&self) -> String {
        let mut uri = format!(
            "{QR_URI_PREFIX}v={QR_PAYLOAD_VERSION}&f={}&s={}",
            self.desktop.to_base64url(),
            URL_SAFE_NO_PAD.encode(self.secret.as_bytes()),
        );
        uri.push_str("&n=");
        uri.extend(utf8_percent_encode(
            truncate(&self.name, MAX_QR_NAME_LEN),
            NAME_ENCODE_SET,
        ));
        if self
            .addresses
            .iter()
            .any(|address| !is_link_local_v6(address))
        {
            let addresses: Vec<String> = self
                .addresses
                .iter()
                .filter(|address| !is_link_local_v6(address))
                .take(MAX_QR_ADDRESSES)
                .map(|address| format_address(*address))
                .collect();
            uri.push_str("&a=");
            uri.push_str(&addresses.join(","));
        }
        if let Some(address) = self.bluetooth_address {
            uri.push_str("&b=");
            address
                .iter()
                .for_each(|byte| uri.push_str(&format!("{byte:02X}")));
        }
        uri
    }

    /// Reads a QR code URI, as the phone does. Unknown parameters are
    /// ignored; `n`, `a` and `b` are optional.
    ///
    /// # Errors
    ///
    /// Fails if the URI is not a pairing code of a known version, or if a
    /// parameter is missing or malformed.
    pub fn parse(uri: &str) -> Result<Self, QrError> {
        let query = uri
            .strip_prefix(QR_URI_PREFIX)
            .ok_or(QrError::NotAPairingCode)?;
        let mut version = None;
        let mut desktop = None;
        let mut secret = None;
        let mut name = String::new();
        let mut addresses = Vec::new();
        let mut bluetooth_address = None;
        for pair in query.split('&') {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            let value = percent_decode_str(value)
                .decode_utf8()
                .map_err(|_| QrError::Malformed("query"))?;
            match key {
                "v" => version = Some(value.into_owned()),
                "f" => {
                    desktop = Some(
                        CertificateFingerprint::from_base64url(&value)
                            .map_err(|_| QrError::Malformed("f"))?,
                    );
                }
                "s" => {
                    let bytes = URL_SAFE_NO_PAD
                        .decode(value.as_bytes())
                        .map_err(|_| QrError::Malformed("s"))?;
                    let bytes: [u8; 16] = bytes.try_into().map_err(|_| QrError::Malformed("s"))?;
                    secret = Some(PairingSecret::from_bytes(bytes));
                }
                "n" => name = value.into_owned(),
                "a" => {
                    addresses = value
                        .split(',')
                        .filter(|address| !address.is_empty())
                        .map(parse_address)
                        .collect::<Option<_>>()
                        .ok_or(QrError::Malformed("a"))?;
                }
                "b" => {
                    let bytes = decode_hex(&value).ok_or(QrError::Malformed("b"))?;
                    bluetooth_address =
                        Some(bytes.try_into().map_err(|_| QrError::Malformed("b"))?);
                }
                _ => {}
            }
        }
        match version {
            Some(version) if version == QR_PAYLOAD_VERSION => {}
            Some(version) => return Err(QrError::UnsupportedVersion(version)),
            None => return Err(QrError::Missing("v")),
        }
        Ok(Self {
            desktop: desktop.ok_or(QrError::Missing("f"))?,
            secret: secret.ok_or(QrError::Missing("s"))?,
            name,
            addresses,
            bluetooth_address,
        })
    }
}

/// The longest prefix of `text` of at most `max` bytes that ends on a
/// character boundary.
fn truncate(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let end = (0..=max)
        .rev()
        .find(|&at| text.is_char_boundary(at))
        .unwrap_or(0);
    &text[..end]
}

/// Whether `address` is an IPv6 link-local address.
fn is_link_local_v6(address: &SocketAddr) -> bool {
    matches!(address.ip(), IpAddr::V6(ip) if ip.is_unicast_link_local())
}

/// An address as written in `a`: the port only when it is not the default.
fn format_address(address: SocketAddr) -> String {
    match (address.ip(), address.port()) {
        (ip, DEFAULT_BEACON_PORT) => ip.to_string(),
        (IpAddr::V4(ip), port) => format!("{ip}:{port}"),
        (IpAddr::V6(ip), port) => format!("[{ip}]:{port}"),
    }
}

/// Reads one address of `a`.
fn parse_address(text: &str) -> Option<SocketAddr> {
    if let Ok(ip) = text.parse::<IpAddr>() {
        return Some(SocketAddr::new(ip, DEFAULT_BEACON_PORT));
    }
    let address: SocketAddr = text.parse().ok()?;
    // Scope ids are not part of the format.
    match address {
        SocketAddr::V6(v6) if v6.scope_id() != 0 => None,
        _ => Some(address),
    }
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, Ipv6Addr, SocketAddrV6};

    use super::*;

    fn payload() -> QrPayload {
        QrPayload {
            desktop: CertificateFingerprint([7; 32]),
            secret: PairingSecret::from_bytes([1; 16]),
            name: "Desk / Ü+&".into(),
            addresses: vec![
                SocketAddr::new(Ipv4Addr::new(10, 0, 0, 2).into(), DEFAULT_BEACON_PORT),
                SocketAddr::new(Ipv4Addr::new(10, 0, 0, 3).into(), 4000),
                SocketAddr::new(Ipv6Addr::LOCALHOST.into(), 4001),
            ],
            bluetooth_address: Some([0xab, 0, 1, 2, 3, 0xff]),
        }
    }

    #[test]
    fn uri_round_trips() {
        let payload = payload();
        let uri = payload.to_uri();
        assert!(uri.contains("&n=Desk%20%2F%20%C3%9C%2B%26&"));
        assert!(uri.contains("&a=10.0.0.2,10.0.0.3:4000,[::1]:4001&"));
        assert!(uri.ends_with("&b=AB00010203FF"));
        assert_eq!(QrPayload::parse(&uri).unwrap(), payload);
    }

    #[test]
    fn parsing_ignores_unknown_parameters_and_checks_the_rest() {
        let uri = payload().to_uri();
        let extended = format!("{uri}&zz=1&x");
        assert_eq!(QrPayload::parse(&extended).unwrap(), payload());

        let v2 = uri.replace("v=1", "v=2");
        assert_eq!(
            QrPayload::parse(&v2),
            Err(QrError::UnsupportedVersion("2".into()))
        );
        assert_eq!(
            QrPayload::parse("https://example.com/"),
            Err(QrError::NotAPairingCode)
        );
        let short_secret = uri.replace("&s=AQEBAQEBAQEBAQEBAQEBAQ", "&s=AQEB");
        assert_eq!(
            QrPayload::parse(&short_secret),
            Err(QrError::Malformed("s"))
        );
        let no_f = format!("{QR_URI_PREFIX}v=1&s=AQEBAQEBAQEBAQEBAQEBAQ");
        assert_eq!(QrPayload::parse(&no_f), Err(QrError::Missing("f")));
    }

    #[test]
    fn long_names_and_address_lists_are_cut() {
        let mut payload = payload();
        payload.name = "ü".repeat(40);
        payload.addresses = (1..=6)
            .map(|n| SocketAddr::new(Ipv4Addr::new(10, 0, 0, n).into(), DEFAULT_BEACON_PORT))
            .collect();
        payload.addresses[0] = SocketAddr::V6(SocketAddrV6::new(
            "fe80::1".parse().unwrap(),
            DEFAULT_BEACON_PORT,
            0,
            3,
        ));
        let parsed = QrPayload::parse(&payload.to_uri()).unwrap();
        assert_eq!(parsed.name, "ü".repeat(32));
        // The link-local address is skipped; four of the other five remain.
        assert_eq!(parsed.addresses, payload.addresses[1..5]);

        payload.addresses.truncate(1);
        assert!(!payload.to_uri().contains("&a="));
    }
}
