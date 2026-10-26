//! The one-time trust step between a desktop and a phone.
//!
//! The desktop shows a QR code with its key's fingerprint and a one-time
//! secret. The phone scans it, and inside the TLS connection each side
//! proves that it knows the secret. A phone without a camera pairs with a
//! six-digit [`PairingCode`] instead, derived with commit-then-reveal and
//! confirmed by the user on both screens. Each side then stores the other's
//! [`CertificateFingerprint`] in its [`TrustStore`]. From then on the
//! transport only accepts peers whose key matches a stored fingerprint.
//! `docs/protocol.md` ("Pairing") gives the exact rules.
//!
//! Stub: the types, the [`TrustStore`] trait and an in-memory store are in
//! place, and the messages are in `pairing.proto`; the handshake comes next.

use std::collections::BTreeMap;
use std::fmt;
use std::time::SystemTime;

use crate::device::DeviceKind;
pub use crate::identity::CertificateFingerprint;

/// The six-digit code both screens show while pairing without a QR code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingCode(String);

impl PairingCode {
    /// Wraps an already derived code.
    pub fn new(code: impl Into<String>) -> Self {
        Self(code.into())
    }

    /// The code as shown to the user.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PairingCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A device this one trusts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairedDevice {
    /// The device's stable identifier.
    pub id: String,
    /// Its name at pairing time, until a `Hello` brings a newer one.
    pub name: String,
    /// Desktop or phone.
    pub kind: DeviceKind,
    /// The key it must present on every connection.
    pub fingerprint: CertificateFingerprint,
    /// When the user confirmed the pairing.
    pub paired_at: SystemTime,
}

/// Why pairing, or storing its result, failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PairingError {
    /// The user declined on one of the two devices.
    #[error("pairing was declined")]
    Declined,
    /// The user did not confirm in time.
    #[error("pairing timed out")]
    TimedOut,
    /// The trust store could not be read or written.
    #[error("trust store error: {0}")]
    Store(String),
}

/// Persistent storage for paired devices, keyed by device id.
pub trait TrustStore: Send + Sync {
    /// The paired device with this id, if any.
    fn get(&self, id: &str) -> Option<PairedDevice>;

    /// Every paired device, ordered by id.
    fn list(&self) -> Vec<PairedDevice>;

    /// Stores `device`, replacing an earlier record with the same id.
    ///
    /// # Errors
    ///
    /// Fails when the store cannot be written.
    fn insert(&mut self, device: PairedDevice) -> Result<(), PairingError>;

    /// Forgets a device. Returns whether it was paired.
    ///
    /// # Errors
    ///
    /// Fails when the store cannot be written.
    fn remove(&mut self, id: &str) -> Result<bool, PairingError>;
}

/// A [`TrustStore`] that lives in memory only, for tests and for running
/// before persistent storage is wired up.
#[derive(Debug, Clone, Default)]
pub struct MemoryTrustStore {
    devices: BTreeMap<String, PairedDevice>,
}

impl TrustStore for MemoryTrustStore {
    fn get(&self, id: &str) -> Option<PairedDevice> {
        self.devices.get(id).map(ToOwned::to_owned)
    }

    fn list(&self) -> Vec<PairedDevice> {
        self.devices.values().map(ToOwned::to_owned).collect()
    }

    fn insert(&mut self, device: PairedDevice) -> Result<(), PairingError> {
        self.devices.insert(device.id.clone(), device);
        Ok(())
    }

    fn remove(&mut self, id: &str) -> Result<bool, PairingError> {
        Ok(self.devices.remove(id).is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn phone(id: &str, fingerprint_byte: u8) -> PairedDevice {
        PairedDevice {
            id: id.into(),
            name: "Pixel".into(),
            kind: DeviceKind::Phone,
            fingerprint: CertificateFingerprint([fingerprint_byte; 32]),
            paired_at: SystemTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn memory_store_keeps_one_record_per_device() {
        let mut store = MemoryTrustStore::default();
        store.insert(phone("b", 1)).unwrap();
        store.insert(phone("a", 2)).unwrap();
        store.insert(phone("b", 3)).unwrap();

        let ids: Vec<_> = store.list().into_iter().map(|device| device.id).collect();
        assert_eq!(ids, ["a", "b"]);
        assert_eq!(store.get("b").unwrap().fingerprint.0, [3; 32]);
        assert!(store.remove("a").unwrap());
        assert!(!store.remove("a").unwrap());
        assert_eq!(store.get("a"), None);
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
