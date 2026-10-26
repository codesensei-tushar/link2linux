//! A [`TrustStore`] kept in a JSON file that only the user can read.

use std::collections::BTreeMap;
use std::fmt;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use aws_lc_rs::aead::{AES_256_GCM, Aad, LessSafeKey, NONCE_LEN, Nonce, UnboundKey};
use serde::{Deserialize, Serialize};
use tracing::warn;
use zeroize::Zeroizing;

use super::crypto::random_bytes;
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
/// The presence keys in it are sealed with a [`StoreKey`] from the keyring
/// when there is one, so the file, which home backups include, does not
/// hold them; without a keyring they are in the file in clear. Once the
/// file was sealed it never goes back to clear: without the key at hand,
/// the sealed keys are written back as they are, and new ones stay in
/// memory until [`use_key`](Self::use_key) brings the key.
#[derive(Debug)]
pub struct FileTrustStore {
    path: PathBuf,
    records: MemoryTrustStore,
    key: Option<StoreKey>,
    /// The file holds its presence keys sealed.
    sealed: bool,
    /// Sealed presence keys from the file that did not open, by record.
    /// They are written back unchanged until the record's presence key is
    /// sealed again.
    unopened: BTreeMap<CertificateFingerprint, String>,
}

impl FileTrustStore {
    /// Opens the store at `path` without a store key: presence keys are in
    /// clear, unless the file was sealed. A missing file is an empty store;
    /// the file is created on the first change.
    ///
    /// # Errors
    ///
    /// Fails if the file cannot be read, does not parse, or was written by a
    /// newer version of the app.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, PairingError> {
        Self::open_sealed(path, None)
    }

    /// Opens the store at `path` like [`open`](Self::open), sealing the
    /// presence keys in the file with `key` when there is one. Keys the file
    /// holds in clear are sealed at once. A key sealed under another store
    /// key, or with none at hand, is not known in memory, where each phone
    /// gives it again at the start of every session, but stays in the file.
    ///
    /// # Errors
    ///
    /// As for [`open`](Self::open).
    pub fn open_sealed(
        path: impl Into<PathBuf>,
        key: Option<StoreKey>,
    ) -> Result<Self, PairingError> {
        let path = path.into();
        let mut records = MemoryTrustStore::default();
        let mut in_clear = false;
        let mut sealed = false;
        let mut unopened = BTreeMap::new();
        if let Some(bytes) = secure_file::read(&path).map_err(store_error)? {
            let file: StoreFile = serde_json::from_slice(&bytes).map_err(store_error)?;
            if file.version > FORMAT_VERSION {
                return Err(PairingError::Store(format!(
                    "{} has format version {}, newer than this app",
                    path.display(),
                    file.version
                )));
            }
            sealed = file.sealed;
            for stored in file.devices {
                in_clear |= stored.presence_key.is_some();
                sealed |= stored.sealed_presence_key.is_some();
                let (device, closed) = stored.into_device(key.as_ref())?;
                if let Some(closed) = closed {
                    unopened.insert(device.fingerprint, closed);
                }
                records.insert(device)?;
            }
        }
        if sealed && key.is_none() {
            warn!(
                "the pairing file is sealed but its key is not at hand: new presence keys stay out of it"
            );
        }
        let mut store = Self {
            path,
            records,
            key,
            sealed,
            unopened,
        };
        if in_clear
            && (store.key.is_some() || store.sealed)
            && let Err(error) = store.write_with(|_| Ok(()))
        {
            warn!(%error, "could not take the presence keys out of the file");
        }
        Ok(store)
    }

    /// Starts sealing with `key`, the store key that was not at hand when
    /// the store opened. The presence keys sealed under it open, unless a
    /// phone gave a newer one since, and the file is written again with all
    /// of them sealed. Does nothing when the store has a key.
    ///
    /// # Errors
    ///
    /// Fails when the file cannot be written; the store seals with `key`
    /// from its next write on.
    pub fn use_key(&mut self, key: StoreKey) -> Result<(), PairingError> {
        if self.key.is_some() {
            return Ok(());
        }
        let opened: Vec<_> = self
            .unopened
            .iter()
            .filter_map(|(fingerprint, sealed)| {
                Some((*fingerprint, key.open(fingerprint, sealed)?))
            })
            .collect();
        self.key = Some(key);
        self.write_with(|records| {
            for (fingerprint, presence) in opened {
                if let Some(mut device) = records.get(&fingerprint)
                    && device.presence_key.is_none()
                {
                    device.presence_key = Some(presence);
                    records.replace(&fingerprint, device)?;
                }
            }
            Ok(())
        })
    }

    /// Where the store lives.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Whether the file is sealed but its key is not at hand, so presence
    /// keys the phones give stay in memory only.
    #[must_use]
    pub fn needs_key(&self) -> bool {
        self.sealed && self.key.is_none()
    }

    /// Applies `change` to a copy of the records, writes the copy, and keeps
    /// it once the write succeeded.
    fn write_with<T>(
        &mut self,
        change: impl FnOnce(&mut MemoryTrustStore) -> Result<T, PairingError>,
    ) -> Result<T, PairingError> {
        let mut records = self.records.clone();
        let result = change(&mut records)?;
        let sealed = self.sealed || self.key.is_some();
        let file = StoreFile {
            version: FORMAT_VERSION,
            sealed,
            devices: records
                .list()
                .iter()
                .map(|device| {
                    let unopened = self.unopened.get(&device.fingerprint);
                    StoredDevice::from_device(device, self.key.as_ref(), sealed, unopened)
                })
                .collect(),
        };
        let json = serde_json::to_vec_pretty(&file).map_err(store_error)?;
        secure_file::write(&self.path, &json).map_err(store_error)?;
        self.unopened
            .retain(|fingerprint, _| records.get(fingerprint).is_some());
        self.records = records;
        self.sealed = sealed;
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

/// The key that seals the presence keys in a [`FileTrustStore`]'s file,
/// kept in the keyring
/// ([`load_store_key`](crate::key_storage::load_store_key)). Its `Debug`
/// output leaves the key out, and its memory is cleared on drop.
pub struct StoreKey(Zeroizing<[u8; 32]>);

impl StoreKey {
    /// A new random key.
    #[must_use]
    pub fn generate() -> Self {
        Self(Zeroizing::new(random_bytes()))
    }

    /// The key from the bytes the keyring holds, if there are 32.
    #[must_use]
    pub fn from_slice(bytes: &[u8]) -> Option<Self> {
        bytes
            .try_into()
            .ok()
            .map(|bytes| Self(Zeroizing::new(bytes)))
    }

    /// The bytes to keep in the keyring.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_slice()
    }

    fn aead(&self) -> LessSafeKey {
        let key = UnboundKey::new(&AES_256_GCM, self.as_bytes()).expect("a 32-byte key");
        LessSafeKey::new(key)
    }

    /// Seals the presence key of the record under `fingerprint`, which is
    /// bound in, so the sealed key cannot be moved to another record: hex
    /// of a random nonce, the ciphertext and the tag.
    fn seal(&self, fingerprint: &CertificateFingerprint, key: &PresenceKey) -> String {
        let nonce: [u8; NONCE_LEN] = random_bytes();
        let mut sealed = key.0.to_vec();
        self.aead()
            .seal_in_place_append_tag(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(fingerprint.0),
                &mut sealed,
            )
            .expect("sealing 32 bytes does not fail");
        hex(&[nonce.as_slice(), &sealed].concat())
    }

    /// The presence key [`seal`](Self::seal) sealed for `fingerprint`, if
    /// it opens.
    fn open(&self, fingerprint: &CertificateFingerprint, sealed: &str) -> Option<PresenceKey> {
        let bytes = decode_hex(sealed)?;
        let (nonce, sealed) = bytes.split_at_checked(NONCE_LEN)?;
        let nonce = Nonce::try_assume_unique_for_key(nonce).ok()?;
        let mut in_out = Zeroizing::new(sealed.to_vec());
        let key = self
            .aead()
            .open_in_place(nonce, Aad::from(fingerprint.0), &mut in_out)
            .ok()?;
        PresenceKey::from_slice(key)
    }
}

impl fmt::Debug for StoreKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("StoreKey(..)")
    }
}

/// The file's top level.
#[derive(Serialize, Deserialize)]
struct StoreFile {
    version: u32,
    /// Presence keys are sealed, and never written in clear again.
    #[serde(default)]
    sealed: bool,
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
    /// Lowercase hex, in a file that was never sealed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    presence_key: Option<String>,
    /// From [`StoreKey::seal`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sealed_presence_key: Option<String>,
    #[serde(default)]
    last_addresses: Vec<SocketAddr>,
}

impl StoredDevice {
    /// `device` for a file that is `sealed` with `key`. Without the key, a
    /// sealed file keeps the sealed key it held, `unopened`.
    fn from_device(
        device: &PairedDevice,
        key: Option<&StoreKey>,
        sealed: bool,
        unopened: Option<&String>,
    ) -> Self {
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
                .filter(|_| !sealed)
                .map(|presence| hex(&presence.0)),
            sealed_presence_key: key
                .zip(device.presence_key.as_ref())
                .map(|(key, presence)| key.seal(&device.fingerprint, presence))
                .or_else(|| unopened.cloned()),
            last_addresses: device.last_addresses.clone(),
        }
    }

    /// The record, and its sealed presence key if that did not open.
    fn into_device(
        self,
        key: Option<&StoreKey>,
    ) -> Result<(PairedDevice, Option<String>), PairingError> {
        let fingerprint = |text: &str| {
            text.parse::<CertificateFingerprint>()
                .map_err(|_| PairingError::Store(format!("malformed fingerprint {text:?}")))
        };
        let current = fingerprint(&self.fingerprint)?;
        let mut unopened = None;
        let presence_key = match (self.sealed_presence_key, self.presence_key) {
            (Some(sealed), _) => {
                let opened = key.and_then(|key| key.open(&current, &sealed));
                if opened.is_none() {
                    unopened = Some(sealed);
                }
                opened
            }
            (None, clear) => clear
                .map(|text| {
                    decode_hex(&text)
                        .as_deref()
                        .and_then(PresenceKey::from_slice)
                        .ok_or_else(|| PairingError::Store("malformed presence key".into()))
                })
                .transpose()?,
        };
        let device = PairedDevice {
            kind: DeviceKind::from_str_name(&self.kind).unwrap_or(DeviceKind::Unspecified),
            fingerprint: current,
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
        };
        Ok((device, unopened))
    }
}

fn store_error(error: impl std::fmt::Display) -> PairingError {
    PairingError::Store(error.to_string())
}

/// Lowercase hex.
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
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
            key: None,
            sealed: false,
            unopened: BTreeMap::new(),
        };
        assert!(store.insert(phone("a", 1)).is_err());
        assert!(store.list().is_empty());
    }

    #[test]
    fn a_store_key_keeps_presence_keys_out_of_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("paired.json");
        let mut device = phone("phone-1", 1);
        device.presence_key = Some(PresenceKey([0xc0; 32]));
        let in_clear = "c0".repeat(32);
        let file = || fs::read_to_string(&path).unwrap();

        // A file from before has the key in clear; opening it with a store
        // key seals it at once.
        FileTrustStore::open(&path)
            .unwrap()
            .insert(device.clone())
            .unwrap();
        assert!(file().contains(&in_clear));
        let key = StoreKey::generate();
        let copy = || StoreKey::from_slice(key.as_bytes());
        let mut sealed = FileTrustStore::open_sealed(&path, copy()).unwrap();
        assert!(!file().contains(&in_clear), "{}", file());
        assert_eq!(sealed.list(), [device.clone()]);
        sealed.insert(phone("phone-2", 3)).unwrap();
        assert!(!file().contains(&in_clear));
        assert_eq!(
            FileTrustStore::open_sealed(&path, copy()).unwrap().list(),
            sealed.list()
        );

        // Under another store key, or none, only the presence key is lost:
        // the phone gives it again in its next session.
        let mut lost = device.clone();
        lost.presence_key = None;
        let other = FileTrustStore::open_sealed(&path, Some(StoreKey::generate())).unwrap();
        assert_eq!(other.get(&device.fingerprint), Some(lost.clone()));
        let without = FileTrustStore::open(&path).unwrap();
        assert_eq!(without.get(&device.fingerprint), Some(lost));
    }

    /// The sealed presence key of the first record in the file at `path`.
    fn sealed_in(path: &Path) -> Option<String> {
        let file: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        file["devices"][0]["sealed_presence_key"]
            .as_str()
            .map(str::to_owned)
    }

    #[test]
    fn a_start_without_the_store_key_keeps_the_sealed_keys_and_writes_none_in_clear() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("paired.json");
        let key = StoreKey::generate();
        let copy = || StoreKey::from_slice(key.as_bytes());
        let mut device = phone("phone-1", 1);
        device.presence_key = Some(PresenceKey([0xc0; 32]));
        let fingerprint = device.fingerprint;
        let file = || fs::read_to_string(&path).unwrap();
        let in_clear = |byte: &str| file().contains(&byte.repeat(32));
        FileTrustStore::open_sealed(&path, copy())
            .unwrap()
            .insert(device)
            .unwrap();
        let sealed = sealed_in(&path).unwrap();

        // One start where the keyring did not answer, so the store opens
        // without its key.
        let mut degraded = FileTrustStore::open_sealed(&path, None).unwrap();
        assert!(degraded.needs_key());
        assert_eq!(degraded.get(&fingerprint).unwrap().presence_key, None);
        // A write, such as the address noted on every connect, keeps the
        // sealed key as it is.
        records::note_connected(
            &mut degraded,
            &fingerprint,
            "192.168.1.9:27380".parse().unwrap(),
        )
        .unwrap();
        assert_eq!(sealed_in(&path).as_ref(), Some(&sealed));
        // The phone gives its presence key again: it stays out of the file.
        records::set_presence_key(&mut degraded, &fingerprint, &[0xd1; 32]).unwrap();
        let newer = Some(PresenceKey([0xd1; 32]));
        assert_eq!(degraded.get(&fingerprint).unwrap().presence_key, newer);
        assert!(!in_clear("d1") && !in_clear("c0"), "{}", file());
        assert_eq!(sealed_in(&path).as_ref(), Some(&sealed));
        assert!(FileTrustStore::open(&path).unwrap().needs_key());
        let next_start = FileTrustStore::open_sealed(&path, copy()).unwrap();
        assert_eq!(
            next_start.get(&fingerprint).unwrap().presence_key,
            Some(PresenceKey([0xc0; 32]))
        );

        // Once the key comes, the newer presence key is sealed with it.
        degraded.use_key(copy().unwrap()).unwrap();
        assert!(!degraded.needs_key());
        assert_ne!(sealed_in(&path).as_ref(), Some(&sealed));
        assert!(!in_clear("d1"));
        let reopened = FileTrustStore::open_sealed(&path, copy()).unwrap();
        assert_eq!(reopened.get(&fingerprint).unwrap().presence_key, newer);
    }

    #[test]
    fn a_sealed_file_stays_sealed_and_its_keys_open_once_the_key_comes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("paired.json");
        let key = StoreKey::generate();
        let copy = || StoreKey::from_slice(key.as_bytes());
        let mut with_key = phone("phone-1", 1);
        with_key.presence_key = Some(PresenceKey([0xc0; 32]));
        // A phone that has not given its presence key yet.
        let without = phone("phone-2", 3);
        let mut store = FileTrustStore::open_sealed(&path, copy()).unwrap();
        store.insert(with_key.clone()).unwrap();
        store.insert(without.clone()).unwrap();

        let mut degraded = FileTrustStore::open_sealed(&path, None).unwrap();
        records::set_presence_key(&mut degraded, &without.fingerprint, &[0xd1; 32]).unwrap();
        let file = fs::read_to_string(&path).unwrap();
        assert!(!file.contains(&"d1".repeat(32)), "{file}");

        // The key comes later: the sealed presence key opens, without the
        // phone giving it again.
        degraded.use_key(copy().unwrap()).unwrap();
        assert_eq!(degraded.get(&with_key.fingerprint), Some(with_key));
        let reopened = FileTrustStore::open_sealed(&path, copy()).unwrap();
        assert_eq!(reopened.list(), degraded.list());

        // A store key the keyring made anew opens nothing, and the old
        // sealed keys stay in the file.
        let sealed = sealed_in(&path);
        let mut other = FileTrustStore::open_sealed(&path, Some(StoreKey::generate())).unwrap();
        records::note_connected(
            &mut other,
            &without.fingerprint,
            "192.168.1.9:27380".parse().unwrap(),
        )
        .unwrap();
        assert_eq!(sealed_in(&path), sealed);
    }

    #[test]
    fn a_sealed_presence_key_opens_only_for_its_record() {
        let key = StoreKey::generate();
        let presence = PresenceKey([7; 32]);
        let mine = CertificateFingerprint([1; 32]);
        let sealed = key.seal(&mine, &presence);
        assert_eq!(key.open(&mine, &sealed), Some(presence.clone()));
        assert_eq!(key.open(&CertificateFingerprint([2; 32]), &sealed), None);
        assert_eq!(StoreKey::generate().open(&mine, &sealed), None);
        assert_ne!(
            key.seal(&mine, &presence),
            sealed,
            "a fresh nonce each time"
        );
        assert_eq!(format!("{key:?}"), "StoreKey(..)");
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
