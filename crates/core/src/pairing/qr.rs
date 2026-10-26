//! The QR code the desktop shows to pair, and the offer behind it.
//!
//! The code holds a URI with the desktop's fingerprint, a one-time
//! [`PairingSecret`], its name, the addresses the phone sends beacons to,
//! and optionally its Bluetooth address:
//!
//! ```text
//! link2linux://pair?v=1&f=<fp_D>&s=<secret>&n=<name>&a=<addresses>&b=<address>
//! ```
//!
//! A [`QrOffer`] is valid for [`OFFER_LIFETIME`] or until one pairing
//! succeeds. A failed proof does not end it: the peer that failed is blocked
//! for the rest of the offer, by key and by address, so a forged
//! announcement cannot use the code up. A peer that never sends a valid
//! proof, because it stalls, drops the connection or breaks the protocol,
//! has failed one too. Only after [`MAX_FAILED_PROOFS`] failures in all, as
//! a backstop, does the desktop withdraw the offer and show a new code.
//!
//! The pairing tag and pairing beacons are public checks of a guess at the
//! secret, which is why it is 16 random bytes and never typed or derived
//! from a code.

use std::collections::BTreeSet;
use std::fmt;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};
use tokio::time::Instant;

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
/// How long an offer stays valid.
pub const OFFER_LIFETIME: Duration = Duration::from_secs(5 * 60);
/// Failed proofs after which the desktop withdraws an offer.
pub const MAX_FAILED_PROOFS: u32 = 32;
/// The name a code carries for a desktop without one: the phone refuses a
/// code with an empty name.
const UNNAMED: &str = "Linux desktop";

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
    /// The name is cut to [`MAX_QR_NAME_LEN`] bytes at a character boundary
    /// (an empty one is written as "Linux desktop"), IPv6 link-local
    /// addresses are skipped, and only the first [`MAX_QR_ADDRESSES`] of the
    /// rest are written.
    #[must_use]
    pub fn to_uri(&self) -> String {
        let mut uri = format!(
            "{QR_URI_PREFIX}v={QR_PAYLOAD_VERSION}&f={}&s={}",
            self.desktop.to_base64url(),
            URL_SAFE_NO_PAD.encode(self.secret.as_bytes()),
        );
        let name = match truncate(&self.name, MAX_QR_NAME_LEN) {
            "" => UNNAMED,
            name => name,
        };
        uri.push_str("&n=");
        uri.extend(utf8_percent_encode(name, NAME_ENCODE_SET));
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
    /// ignored; `a` and `b` are optional, and `n` must not be empty.
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
        let desktop = desktop.ok_or(QrError::Missing("f"))?;
        let secret = secret.ok_or(QrError::Missing("s"))?;
        if name.is_empty() {
            return Err(QrError::Missing("n"));
        }
        Ok(Self {
            desktop,
            secret,
            name,
            addresses,
            bluetooth_address,
        })
    }
}

/// Whether an offer can still be used to pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OfferStatus {
    /// The code can be scanned and used.
    Active,
    /// [`OFFER_LIFETIME`] has passed.
    Expired,
    /// A phone paired with it.
    Used,
    /// Too many proofs failed; the desktop shows a new code.
    Withdrawn,
}

/// A QR code on the desktop's screen, and what happened to it so far.
///
/// Share it between the connections that may use it behind a mutex; the
/// pairing functions lock it only briefly and never across a wait. Its
/// `Debug` output leaves out the URI, which holds the secret.
pub struct QrOffer {
    payload: QrPayload,
    uri: String,
    expires_at: Instant,
    failed_proofs: u32,
    max_failed_proofs: u32,
    blocked_keys: BTreeSet<CertificateFingerprint>,
    blocked_addresses: BTreeSet<IpAddr>,
    used: bool,
}

impl QrOffer {
    /// A new offer with a fresh secret, valid from now.
    #[must_use]
    pub fn new(
        desktop: CertificateFingerprint,
        name: impl Into<String>,
        addresses: Vec<SocketAddr>,
        bluetooth_address: Option<[u8; 6]>,
    ) -> Self {
        let payload = QrPayload {
            desktop,
            secret: PairingSecret::generate(),
            name: name.into(),
            addresses,
            bluetooth_address,
        };
        Self::from_payload(payload, Instant::now())
    }

    /// An offer for a given payload, created at `now`.
    #[must_use]
    pub fn from_payload(payload: QrPayload, now: Instant) -> Self {
        Self {
            uri: payload.to_uri(),
            payload,
            expires_at: now + OFFER_LIFETIME,
            failed_proofs: 0,
            max_failed_proofs: MAX_FAILED_PROOFS,
            blocked_keys: BTreeSet::new(),
            blocked_addresses: BTreeSet::new(),
            used: false,
        }
    }

    /// Changes how many failed proofs withdraw the offer.
    #[must_use]
    pub fn with_max_failed_proofs(mut self, max: u32) -> Self {
        self.max_failed_proofs = max;
        self
    }

    /// The URI to encode in the QR code (byte mode, error correction M).
    #[must_use]
    pub fn uri(&self) -> &str {
        &self.uri
    }

    /// What the code carries.
    #[must_use]
    pub fn payload(&self) -> &QrPayload {
        &self.payload
    }

    /// The one-time secret. Discovery keys pairing beacons with it.
    #[must_use]
    pub fn secret(&self) -> &PairingSecret {
        &self.payload.secret
    }

    /// The pairing tag `q` a phone announces after scanning this code.
    #[must_use]
    pub fn pairing_tag(&self) -> [u8; 8] {
        crate::discovery::presence::pairing_tag(self.payload.secret.as_bytes())
    }

    /// When the offer expires.
    #[must_use]
    pub fn expires_at(&self) -> Instant {
        self.expires_at
    }

    /// Proofs that failed so far.
    #[must_use]
    pub fn failed_proofs(&self) -> u32 {
        self.failed_proofs
    }

    /// Whether the offer can be used at `now`.
    #[must_use]
    pub fn status(&self, now: Instant) -> OfferStatus {
        if self.used {
            OfferStatus::Used
        } else if self.failed_proofs >= self.max_failed_proofs {
            OfferStatus::Withdrawn
        } else if now >= self.expires_at {
            OfferStatus::Expired
        } else {
            OfferStatus::Active
        }
    }

    /// Whether to dial `address` for this offer: false once a peer at that
    /// address failed a proof.
    #[must_use]
    pub fn may_dial(&self, address: IpAddr) -> bool {
        !self.blocked_addresses.contains(&address)
    }

    /// Whether a peer with this key, at this address, failed a proof
    /// before.
    #[must_use]
    pub fn is_blocked(&self, key: &CertificateFingerprint, address: IpAddr) -> bool {
        self.blocked_keys.contains(key) || self.blocked_addresses.contains(&address)
    }

    /// Counts a failed proof and blocks the peer for the rest of the offer.
    /// A peer whose connection ended before its proof verified, for any
    /// reason, failed one.
    pub fn record_failed_proof(&mut self, key: CertificateFingerprint, address: IpAddr) {
        self.failed_proofs = self.failed_proofs.saturating_add(1);
        self.blocked_keys.insert(key);
        self.blocked_addresses.insert(address);
    }

    /// Marks the offer as used by a successful pairing.
    pub(crate) fn mark_used(&mut self) {
        self.used = true;
    }
}

impl fmt::Debug for QrOffer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("QrOffer")
            .field("payload", &self.payload)
            .field("expires_at", &self.expires_at)
            .field("failed_proofs", &self.failed_proofs)
            .field("used", &self.used)
            .finish_non_exhaustive()
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

    #[test]
    fn a_code_always_names_the_desktop() {
        let mut payload = payload();
        payload.name = String::new();
        let uri = payload.to_uri();
        assert!(uri.contains("&n=Linux%20desktop&"), "{uri}");
        assert_eq!(QrPayload::parse(&uri).unwrap().name, "Linux desktop");

        let unnamed = uri.replace("&n=Linux%20desktop", "&n=");
        assert_eq!(QrPayload::parse(&unnamed), Err(QrError::Missing("n")));
    }

    #[test]
    fn offers_expire_get_used_and_block_failing_peers() {
        let start = Instant::now();
        let mut offer = QrOffer::from_payload(payload(), start);
        assert_eq!(offer.status(start), OfferStatus::Active);
        assert_eq!(offer.status(start + OFFER_LIFETIME), OfferStatus::Expired);

        let key = CertificateFingerprint([9; 32]);
        let address: IpAddr = Ipv4Addr::new(10, 0, 0, 9).into();
        offer.record_failed_proof(key, address);
        assert!(offer.is_blocked(&key, Ipv4Addr::LOCALHOST.into()));
        assert!(offer.is_blocked(&CertificateFingerprint([0; 32]), address));
        assert!(!offer.may_dial(address));
        assert_eq!(offer.status(start), OfferStatus::Active);

        for _ in 1..MAX_FAILED_PROOFS {
            offer.record_failed_proof(key, address);
        }
        assert_eq!(offer.status(start), OfferStatus::Withdrawn);

        let mut used = QrOffer::from_payload(payload(), start);
        used.mark_used();
        assert_eq!(used.status(start), OfferStatus::Used);
    }

    #[test]
    fn secrets_stay_out_of_debug_output() {
        let offer = QrOffer::new(CertificateFingerprint([0; 32]), "Desk", Vec::new(), None);
        let printed = format!("{offer:?}");
        assert!(printed.contains("PairingSecret(..)"));
        let secret = URL_SAFE_NO_PAD.encode(offer.secret().as_bytes());
        assert!(!printed.contains(&secret), "{printed}");
        assert_eq!(offer.uri().len(), offer.payload().to_uri().len());
        assert!(offer.uri().ends_with("&n=Desk"));
    }
}
