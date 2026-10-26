//! A [`TrustStore`] kept in a JSON file that only the user can read.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::{
    CertificateFingerprint, MemoryTrustStore, PairedDevice, PairingError, PresenceKey, TrustStore,
};
use crate::device::DeviceKind;
use crate::identity::decode_hex;
use crate::secure_file;

/// Version of the file format this build writes.
const FORMAT_VERSION: u32 = 1;

/// A [`TrustStore`] that keeps its records in a file.
///
/// The path is chosen by the caller, normally under the user's data
/// directory. Every change rewrites the whole file atomically, with mode
/// 0600, before it takes effect in memory: a failed write changes nothing.
/// The file holds presence keys, so it must stay out of backups and logs.
#[derive(Debug)]
pub struct FileTrustStore {
    path: PathBuf,
    records: MemoryTrustStore,
}

impl FileTrustStore {
    /// Opens the store at `path`. A missing file is an empty store; the file
    /// is created on the first change.
    ///
    /// # Errors
    ///
    /// Fails if the file cannot be read, does not parse, or was written by a
    /// newer version of the app.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, PairingError> {
        let path = path.into();
        let mut records = MemoryTrustStore::default();
        if let Some(bytes) = secure_file::read(&path).map_err(store_error)? {
            let file: StoreFile = serde_json::from_slice(&bytes).map_err(store_error)?;
            if file.version > FORMAT_VERSION {
                return Err(PairingError::Store(format!(
                    "{} has format version {}, newer than this app",
                    path.display(),
                    file.version
                )));
            }
            for stored in file.devices {
                records.insert(stored.into_device()?)?;
            }
        }
        Ok(Self { path, records })
    }

    /// Where the store lives.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Applies `change` to a copy of the records, writes the copy, and keeps
    /// it once the write succeeded.
    fn write_with<T>(
        &mut self,
        change: impl FnOnce(&mut MemoryTrustStore) -> Result<T, PairingError>,
    ) -> Result<T, PairingError> {
        let mut records = self.records.clone();
        let result = change(&mut records)?;
        let file = StoreFile {
            version: FORMAT_VERSION,
            devices: records
                .list()
                .iter()
                .map(StoredDevice::from_device)
                .collect(),
        };
        let json = serde_json::to_vec_pretty(&file).map_err(store_error)?;
        secure_file::write(&self.path, &json).map_err(store_error)?;
        self.records = records;
        Ok(result)
    }
}

impl TrustStore for FileTrustStore {
    fn list(&self) -> Vec<PairedDevice> {
        self.records.list()
    }

    fn get(&self, fingerprint: &CertificateFingerprint) -> Option<PairedDevice> {
        self.records.get(fingerprint)
    }

    fn replace(
        &mut self,
        old: &CertificateFingerprint,
        device: PairedDevice,
    ) -> Result<(), PairingError> {
        self.write_with(|records| records.replace(old, device))
    }

    fn remove(
        &mut self,
        fingerprint: &CertificateFingerprint,
    ) -> Result<Option<PairedDevice>, PairingError> {
        if self.records.get(fingerprint).is_none() {
            return Ok(None);
        }
        self.write_with(|records| records.remove(fingerprint))
    }
}

/// The file's top level.
#[derive(Serialize, Deserialize)]
struct StoreFile {
    version: u32,
    devices: Vec<StoredDevice>,
}

/// One record on disk. Unknown fields from newer versions are ignored.
#[derive(Serialize, Deserialize)]
struct StoredDevice {
    id: String,
    name: String,
    /// The `DeviceKind` name, such as `DEVICE_KIND_PHONE`.
    kind: String,
    /// Lowercase hex.
    fingerprint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    next_fingerprint: Option<String>,
    paired_at_unix_ms: u64,
    #[serde(default)]
    confirmed: bool,
    /// Lowercase hex.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    presence_key: Option<String>,
    #[serde(default)]
    last_addresses: Vec<SocketAddr>,
}

impl StoredDevice {
    fn from_device(device: &PairedDevice) -> Self {
        let paired_at = device
            .paired_at
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        Self {
            id: device.id.clone(),
            name: device.name.clone(),
            kind: device.kind.as_str_name().to_owned(),
            fingerprint: device.fingerprint.to_string(),
            next_fingerprint: device.next_fingerprint.map(|next| next.to_string()),
            paired_at_unix_ms: u64::try_from(paired_at.as_millis()).unwrap_or(u64::MAX),
            confirmed: device.confirmed,
            presence_key: device
                .presence_key
                .as_ref()
                .map(|key| key.0.iter().map(|byte| format!("{byte:02x}")).collect()),
            last_addresses: device.last_addresses.clone(),
        }
    }

    fn into_device(self) -> Result<PairedDevice, PairingError> {
        let fingerprint = |text: &str| {
            text.parse::<CertificateFingerprint>()
                .map_err(|_| PairingError::Store(format!("malformed fingerprint {text:?}")))
        };
        let presence_key = self
            .presence_key
            .map(|text| {
                decode_hex(&text)
                    .as_deref()
                    .and_then(PresenceKey::from_slice)
                    .ok_or_else(|| PairingError::Store("malformed presence key".into()))
            })
            .transpose()?;
        Ok(PairedDevice {
            kind: DeviceKind::from_str_name(&self.kind).unwrap_or(DeviceKind::Unspecified),
            fingerprint: fingerprint(&self.fingerprint)?,
            next_fingerprint: self
                .next_fingerprint
                .as_deref()
                .map(fingerprint)
                .transpose()?,
            paired_at: UNIX_EPOCH + Duration::from_millis(self.paired_at_unix_ms),
            confirmed: self.confirmed,
            presence_key,
            last_addresses: self.last_addresses,
            id: self.id,
            name: self.name,
        })
    }
}

fn store_error(error: impl std::fmt::Display) -> PairingError {
    PairingError::Store(error.to_string())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::time::SystemTime;

    use super::*;
    use crate::pairing::records;
    use crate::pairing::tests::phone;

    fn at_ms(ms: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_millis(ms)
    }

    #[test]
    fn records_survive_reopening() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("link2linux/paired.json");

        let mut store = FileTrustStore::open(&path).unwrap();
        assert!(store.list().is_empty());
        assert!(!path.exists());

        let mut device = phone("phone-1", 1);
        device.paired_at = at_ms(1_793_318_400_123);
        device.next_fingerprint = Some(CertificateFingerprint([2; 32]));
        device.presence_key = Some(PresenceKey([0xc0; 32]));
        device.last_addresses = vec![
            "192.168.1.7:27380".parse().unwrap(),
            "[fe80::1]:27380".parse().unwrap(),
        ];
        device.confirmed = true;
        store.insert(device.clone()).unwrap();
        store.insert(phone("phone-2", 3)).unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );

        let reopened = FileTrustStore::open(&path).unwrap();
        assert_eq!(reopened.list(), store.list());
        assert_eq!(reopened.get(&CertificateFingerprint([2; 32])), Some(device));

        let mut reopened = reopened;
        records::note_connected(
            &mut reopened,
            &CertificateFingerprint([2; 32]),
            "192.168.1.8:27380".parse().unwrap(),
        )
        .unwrap();
        assert!(
            reopened
                .remove(&CertificateFingerprint([3; 32]))
                .unwrap()
                .is_some()
        );
        let again = FileTrustStore::open(&path).unwrap();
        let ids: Vec<_> = again
            .list()
            .into_iter()
            .map(|d| (d.id, d.fingerprint.0[0]))
            .collect();
        assert_eq!(ids, [("phone-1".to_owned(), 2)]);
    }

    #[test]
    fn failed_writes_change_nothing() {
        let dir = tempfile::tempdir().unwrap();
        // A directory where the file should be makes every write fail.
        let path = dir.path().join("paired.json");
        fs::create_dir(&path).unwrap();
        let mut store = FileTrustStore {
            path,
            records: MemoryTrustStore::default(),
        };
        assert!(store.insert(phone("a", 1)).is_err());
        assert!(store.list().is_empty());
    }

    #[test]
    fn newer_formats_and_garbage_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("paired.json");
        fs::write(&path, br#"{"version": 2, "devices": []}"#).unwrap();
        assert!(matches!(
            FileTrustStore::open(&path),
            Err(PairingError::Store(_))
        ));
        fs::write(&path, b"not json").unwrap();
        assert!(matches!(
            FileTrustStore::open(&path),
            Err(PairingError::Store(_))
        ));

        let unknown_fields = br#"{"version": 1, "future": true, "devices": [{"id": "a",
            "name": "n", "kind": "DEVICE_KIND_PHONE", "paired_at_unix_ms": 5, "extra": 1,
            "fingerprint": "0101010101010101010101010101010101010101010101010101010101010101"}]}"#;
        fs::write(&path, unknown_fields).unwrap();
        let store = FileTrustStore::open(&path).unwrap();
        assert_eq!(store.list()[0].paired_at, at_ms(5));
        assert_eq!(store.list()[0].kind, DeviceKind::Phone);
    }
}
