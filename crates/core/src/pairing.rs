//! The one-time trust step between a desktop and a phone, and the records
//! it leaves behind.
//!
//! The desktop shows a QR code ([`QrOffer`]) with its key's fingerprint and
//! a one-time secret. The phone scans it, and inside the TLS connection each
//! side proves that it knows the secret ([`pair_by_qr`]). A phone without a
//! camera pairs with a six-digit [`PairingCode`] instead, derived with
//! commit-then-reveal and confirmed by the user on both screens
//! ([`pair_by_sas`]). Each side then stores a [`PairedDevice`] record in its
//! [`TrustStore`], keyed by the peer's [`CertificateFingerprint`]. From then
//! on the transport only accepts peers whose key matches a record, and the
//! functions in [`records`] keep the records current: confirmation, key
//! rotation and unpairing. `docs/protocol.md` ("Pairing") gives the exact
//! rules, and [`crypto`] the exact bytes.
//!
//! Records are found by fingerprint, never by the device id a peer claims
//! in its `Hello`; the id is only checked against the record
//! ([`records::check_peer`]).

pub mod crypto;
mod desktop;
mod qr;
pub mod records;
mod store;

use std::collections::BTreeMap;
use std::fmt;
use std::net::SocketAddr;
use std::time::SystemTime;

pub use self::crypto::PairingSecret;
pub use self::desktop::{
    PairingBinding, PairingChannel, PairingPrompt, PairingSuccess, PairingTimeouts, SasDecision,
    pair_by_qr, pair_by_sas, refuse,
};
pub use self::qr::{
    DEFAULT_BEACON_PORT, MAX_FAILED_PROOFS, MAX_QR_ADDRESSES, MAX_QR_NAME_LEN, OFFER_LIFETIME,
    OfferStatus, QR_PAYLOAD_VERSION, QR_URI_PREFIX, QrError, QrOffer, QrPayload,
};
pub use self::store::FileTrustStore;
use crate::device::DeviceKind;
pub use crate::identity::CertificateFingerprint;
use crate::transport::PeerPin;
use link2linux_protocol::v1::GoodbyeReason;
pub use link2linux_protocol::v1::{PairingMethod, PairingOutcome};
use zeroize::Zeroize;

/// The six-digit code both screens show while pairing without a QR code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingCode(String);

impl PairingCode {
    /// Wraps an already derived code.
    pub fn new(code: impl Into<String>) -> Self {
        Self(code.into())
    }

    /// The code for a number below 1 000 000: six digits, with leading
    /// zeros.
    #[must_use]
    pub fn from_number(value: u32) -> Self {
        Self(format!("{:06}", value % 1_000_000))
    }

    /// The code as six digits, such as `"042917"`.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The code as shown to the user: two groups of three digits, such as
    /// `"042 917"`.
    #[must_use]
    pub fn grouped(&self) -> String {
        match (self.0.get(..3), self.0.get(3..)) {
            (Some(head), Some(tail)) if self.0.len() == 6 => format!("{head} {tail}"),
            _ => self.0.clone(),
        }
    }
}

impl fmt::Display for PairingCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The 32-byte key behind a phone's rotating presence tags, which it gives
/// to the desktop in `PresenceUpdate`. `Debug` never prints it, and its
/// memory is cleared on drop.
#[derive(Clone, PartialEq, Eq)]
pub struct PresenceKey(pub [u8; 32]);

impl Drop for PresenceKey {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl PresenceKey {
    /// The key from the bytes of a `PresenceUpdate`, if there are 32.
    #[must_use]
    pub fn from_slice(bytes: &[u8]) -> Option<Self> {
        bytes.try_into().ok().map(Self)
    }
}

impl fmt::Debug for PresenceKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PresenceKey(..)")
    }
}

/// A device this one trusts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairedDevice {
    /// The device id from its `Hello` at pairing time. Checked against
    /// every later `Hello`, never used to find the record.
    pub id: String,
    /// Its name at pairing time, until a `Hello` brings a newer one.
    pub name: String,
    /// Desktop or phone.
    pub kind: DeviceKind,
    /// The key it presents; the record is stored under it.
    pub fingerprint: CertificateFingerprint,
    /// The key it announced with `KeyRotation`, accepted as well until a
    /// connection with it succeeds.
    pub next_fingerprint: Option<CertificateFingerprint>,
    /// When the pairing was stored.
    pub paired_at: SystemTime,
    /// Whether the peer confirmed that it stored the pairing too: set by its
    /// first `FeatureStatus`.
    pub confirmed: bool,
    /// The phone's presence key, once its `PresenceUpdate` arrived.
    pub presence_key: Option<PresenceKey>,
    /// Where the peer was reached last, most recent first.
    pub last_addresses: Vec<SocketAddr>,
}

impl PairedDevice {
    /// A new, unconfirmed record with no next key, presence key or
    /// addresses.
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        kind: DeviceKind,
        fingerprint: CertificateFingerprint,
        paired_at: SystemTime,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            kind,
            fingerprint,
            next_fingerprint: None,
            paired_at,
            confirmed: false,
            presence_key: None,
            last_addresses: Vec::new(),
        }
    }

    /// Whether the device may present `fingerprint`: its current or its
    /// next key.
    #[must_use]
    pub fn accepts(&self, fingerprint: &CertificateFingerprint) -> bool {
        self.fingerprint == *fingerprint || self.next_fingerprint.as_ref() == Some(fingerprint)
    }

    /// The keys to accept when dialing this device.
    #[must_use]
    pub fn pin(&self) -> PeerPin {
        PeerPin::Paired {
            current: self.fingerprint,
            next: self.next_fingerprint,
        }
    }
}

/// Why pairing, or storing its result, failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PairingError {
    /// The user declined on this device.
    #[error("pairing was declined")]
    Declined,
    /// The pairing would replace records of the same device id under
    /// another key, and the user kept them.
    #[error("pairing was declined to keep the device's other pairing")]
    ReplaceDeclined,
    /// The user, or the peer, did not go on in time.
    #[error("pairing timed out")]
    TimedOut,
    /// The peer ended the pairing with this outcome.
    #[error("the phone ended pairing: {}", .0.as_str_name())]
    Rejected(PairingOutcome),
    /// The peer's `PairingProof` or `SasCommit` did not verify.
    #[error("the phone's proof did not verify")]
    VerificationFailed,
    /// The QR offer cannot be used any more.
    #[error("the QR code is no longer valid ({0:?})")]
    OfferUnavailable(OfferStatus),
    /// This peer failed a proof for the current QR offer before.
    #[error("this peer already failed to pair with the current QR code")]
    Blocked,
    /// The peer sent this pairing message out of order, or twice.
    #[error("the phone sent {0} out of order")]
    OutOfOrder(&'static str),
    /// The peer broke the protocol of the connection.
    #[error("protocol error: {0}")]
    Protocol(String),
    /// The peer closed the connection.
    #[error("the connection closed during pairing")]
    Closed,
    /// Sending or receiving failed.
    #[error("connection error during pairing: {0}")]
    Connection(String),
    /// The TLS keying material exporter failed; pairing without it is not
    /// allowed.
    #[error("cannot bind pairing to the connection: {0}")]
    Exporter(String),
    /// No record exists for the peer's key.
    #[error("no pairing record for {0}")]
    NotPaired(CertificateFingerprint),
    /// The trust store could not be read or written.
    #[error("trust store error: {0}")]
    Store(String),
}

impl PairingError {
    /// The reason of the `Goodbye` that ends the connection after this
    /// error, when the pairing exchange has not told the peer already:
    /// [`GoodbyeReason::ProtocolError`] when the peer broke the protocol,
    /// and [`GoodbyeReason::Unspecified`] when the trust store failed here.
    /// `None` when a `PairingResult` went out or the connection is gone;
    /// the caller then just closes it.
    #[must_use]
    pub fn goodbye_reason(&self) -> Option<GoodbyeReason> {
        match self {
            Self::Protocol(_) => Some(GoodbyeReason::ProtocolError),
            Self::Store(_) => Some(GoodbyeReason::Unspecified),
            _ => None,
        }
    }
}

/// Persistent storage for paired devices, keyed by fingerprint.
///
/// Each record is stored under its current fingerprint. Lookups also match
/// a record's next fingerprint, so a peer that rotated its key is found
/// either way. The functions in [`records`] build the pairing life cycle on
/// these primitives.
pub trait TrustStore: Send + Sync {
    /// Every paired device, ordered by fingerprint.
    fn list(&self) -> Vec<PairedDevice>;

    /// The device that may present `fingerprint`: the record stored under
    /// it, or else a record whose next fingerprint it is.
    fn get(&self, fingerprint: &CertificateFingerprint) -> Option<PairedDevice> {
        let devices = self.list();
        let current = devices
            .iter()
            .position(|device| device.fingerprint == *fingerprint);
        let index = current.or_else(|| {
            devices
                .iter()
                .position(|device| device.next_fingerprint.as_ref() == Some(fingerprint))
        })?;
        devices.into_iter().nth(index)
    }

    /// In one step, removes the record found by `old` (if any) and stores
    /// `device` under its current fingerprint, replacing a record stored
    /// there.
    ///
    /// # Errors
    ///
    /// Fails when the store cannot be written; nothing changes then.
    fn replace(
        &mut self,
        old: &CertificateFingerprint,
        device: PairedDevice,
    ) -> Result<(), PairingError>;

    /// Stores `device` under its current fingerprint, replacing a record
    /// stored there.
    ///
    /// # Errors
    ///
    /// Fails when the store cannot be written.
    fn insert(&mut self, device: PairedDevice) -> Result<(), PairingError> {
        let key = device.fingerprint;
        self.replace(&key, device)
    }

    /// Forgets the device found by `fingerprint`, and returns its record.
    ///
    /// # Errors
    ///
    /// Fails when the store cannot be written.
    fn remove(
        &mut self,
        fingerprint: &CertificateFingerprint,
    ) -> Result<Option<PairedDevice>, PairingError>;
}

/// A [`TrustStore`] that lives in memory only, for tests and for running
/// before persistent storage is wired up.
#[derive(Debug, Clone, Default)]
pub struct MemoryTrustStore {
    devices: BTreeMap<CertificateFingerprint, PairedDevice>,
}

impl MemoryTrustStore {
    /// The key a lookup by `fingerprint` finds, as for [`TrustStore::get`].
    fn key_of(&self, fingerprint: &CertificateFingerprint) -> Option<CertificateFingerprint> {
        if self.devices.contains_key(fingerprint) {
            return Some(*fingerprint);
        }
        self.devices
            .values()
            .find(|device| device.next_fingerprint.as_ref() == Some(fingerprint))
            .map(|device| device.fingerprint)
    }
}

impl TrustStore for MemoryTrustStore {
    fn list(&self) -> Vec<PairedDevice> {
        self.devices.values().cloned().collect()
    }

    fn get(&self, fingerprint: &CertificateFingerprint) -> Option<PairedDevice> {
        self.key_of(fingerprint)
            .and_then(|key| self.devices.get(&key).cloned())
    }

    fn replace(
        &mut self,
        old: &CertificateFingerprint,
        device: PairedDevice,
    ) -> Result<(), PairingError> {
        if let Some(key) = self.key_of(old) {
            self.devices.remove(&key);
        }
        self.devices.insert(device.fingerprint, device);
        Ok(())
    }

    fn remove(
        &mut self,
        fingerprint: &CertificateFingerprint,
    ) -> Result<Option<PairedDevice>, PairingError> {
        Ok(self
            .key_of(fingerprint)
            .and_then(|key| self.devices.remove(&key)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn phone(id: &str, fingerprint_byte: u8) -> PairedDevice {
        PairedDevice::new(
            id,
            "Pixel",
            DeviceKind::Phone,
            CertificateFingerprint([fingerprint_byte; 32]),
            SystemTime::UNIX_EPOCH,
        )
    }

    #[test]
    fn memory_store_keys_records_by_fingerprint() {
        let mut store = MemoryTrustStore::default();
        store.insert(phone("b", 1)).unwrap();
        store.insert(phone("a", 2)).unwrap();
        // Same key again: replaces the record.
        let mut renamed = phone("b", 1);
        renamed.name = "Renamed".into();
        store.insert(renamed).unwrap();
        // Same id, other key: a separate record.
        store.insert(phone("b", 3)).unwrap();

        let keys: Vec<_> = store.list().iter().map(|d| d.fingerprint.0[0]).collect();
        assert_eq!(keys, [1, 2, 3]);
        let first = store.get(&CertificateFingerprint([1; 32])).unwrap();
        assert_eq!(first.name, "Renamed");

        let removed = store.remove(&CertificateFingerprint([2; 32])).unwrap();
        assert_eq!(removed.unwrap().id, "a");
        assert_eq!(
            store.remove(&CertificateFingerprint([2; 32])).unwrap(),
            None
        );
        assert_eq!(store.get(&CertificateFingerprint([2; 32])), None);
    }

    #[test]
    fn lookups_match_the_next_key_and_prefer_the_current_one() {
        let mut store = MemoryTrustStore::default();
        let mut rotating = phone("a", 1);
        rotating.next_fingerprint = Some(CertificateFingerprint([2; 32]));
        store.insert(rotating).unwrap();
        assert_eq!(store.get(&CertificateFingerprint([2; 32])).unwrap().id, "a");

        store.insert(phone("b", 2)).unwrap();
        assert_eq!(store.get(&CertificateFingerprint([2; 32])).unwrap().id, "b");

        // The provided default lookup agrees with the store's own.
        struct ListOnly(MemoryTrustStore);
        impl TrustStore for ListOnly {
            fn list(&self) -> Vec<PairedDevice> {
                self.0.list()
            }
            fn replace(
                &mut self,
                _: &CertificateFingerprint,
                _: PairedDevice,
            ) -> Result<(), PairingError> {
                unreachable!()
            }
            fn remove(
                &mut self,
                _: &CertificateFingerprint,
            ) -> Result<Option<PairedDevice>, PairingError> {
                unreachable!()
            }
        }
        let list_only = ListOnly(store.clone());
        for byte in 0..4 {
            let fingerprint = CertificateFingerprint([byte; 32]);
            assert_eq!(list_only.get(&fingerprint), store.get(&fingerprint));
        }
    }

    #[test]
    fn codes_are_six_digits_in_two_groups() {
        assert_eq!(PairingCode::from_number(42_917).as_str(), "042917");
        assert_eq!(PairingCode::from_number(979_999).grouped(), "979 999");
        assert_eq!(PairingCode::new("12").grouped(), "12");
    }

    #[test]
    fn fingerprints_print_as_hex() {
        let mut bytes = [0u8; 32];
        bytes[0] = 0xab;
        bytes[31] = 0x01;
        let text = CertificateFingerprint(bytes).to_string();
        assert_eq!(text.len(), 64);
        assert!(text.starts_with("ab00"));
        assert!(text.ends_with("0001"));
    }
}
