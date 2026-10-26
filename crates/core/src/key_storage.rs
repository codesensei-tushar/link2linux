//! Where the desktop keeps its private key: in the Secret Service keyring
//! when there is one, and otherwise in a file only the user can read (mode
//! 0600).
//!
//! [`KeyStorage::load_or_create`] runs at every start:
//!
//! - A key in the file is used, and moved into the keyring if the keyring
//!   takes it: it is written there, read back and compared, and only then
//!   is the file overwritten and deleted. The file goes first because it
//!   exists only while a key has not moved yet: one from before the
//!   keyring was used, or one made while the keyring was out of reach.
//! - Otherwise the key in the keyring is used.
//! - With neither, a new key is made and kept in the keyring, or in the
//!   file when the keyring cannot take it.
//!
//! A keyring that cannot be reached (none on a minimal desktop, a locked
//! one whose prompt was dismissed, or one that does not answer in time) is
//! no error by itself: the key stays in, or goes to, the file. Once a key
//! has moved to the keyring, though, a marker file records it, and an
//! unreachable keyring then stops the start. Making a new key instead
//! would quietly break every pairing.
//!
//! The keyring also holds the [`StoreKey`] that seals the phones' presence
//! keys in the pairing file ([`load_store_key`]).

use std::io;
use std::path::PathBuf;

use link2linux_platform::{PlatformError, SecretStore};
use tracing::{info, warn};

use crate::identity::{Identity, IdentityError};
use crate::pairing::StoreKey;
use crate::secure_file;

/// What the marker file says, for anyone who opens it.
const MARKER_TEXT: &[u8] = b"The private key of this desktop is in the Secret Service keyring.\n";

/// The places a desktop's private key can be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyStorage {
    /// The key's name in the keyring.
    pub name: String,
    /// What the keyring's own app shows for the key.
    pub label: String,
    /// The file that holds the key when no keyring does.
    pub file: PathBuf,
    /// The file that records that the key is in the keyring.
    pub marker: PathBuf,
}

/// Why a key could not be moved into the keyring. The key then stays in
/// its file.
#[derive(Debug, thiserror::Error)]
enum MoveError {
    #[error(transparent)]
    Keyring(#[from] PlatformError),
    #[error("the keyring returned a different key than the one written")]
    Mismatch,
    #[error("could not record that the key is in the keyring: {0}")]
    Marker(#[from] io::Error),
}

impl KeyStorage {
    /// Loads the desktop's identity from wherever it is kept, or creates
    /// one, as the module documentation describes. Unlocking the keyring
    /// may prompt the user.
    ///
    /// # Errors
    ///
    /// Fails when the file cannot be read or written, when a stored key is
    /// invalid, or with [`IdentityError::KeyringUnavailable`] when the key
    /// is in a keyring that cannot be reached.
    pub async fn load_or_create<S: SecretStore>(
        &self,
        keyring: &S,
    ) -> Result<Identity, IdentityError> {
        if let Some(identity) = Identity::load(&self.file)? {
            match self.move_to_keyring(keyring, &identity).await {
                Ok(()) => info!("moved the private key from its file into the keyring"),
                Err(error) => warn!(%error, "the private key stays in its file"),
            }
            return Ok(identity);
        }
        match keyring.read(&self.name).await {
            Ok(Some(pem)) => {
                let identity = Identity::from_pem_bytes(pem)?;
                if !self.marker.exists() {
                    secure_file::write(&self.marker, MARKER_TEXT)?;
                }
                Ok(identity)
            }
            Ok(None) => {
                if self.marker.exists() {
                    warn!(
                        "the keyring no longer holds the private key: making a new one, \
                         so phones have to be paired again"
                    );
                }
                let identity = Identity::generate()?;
                if let Err(error) = self.store(keyring, &identity).await {
                    warn!(%error, "keeping the new private key in a file");
                    identity.save(&self.file)?;
                    secure_file::erase(&self.marker)?;
                }
                Ok(identity)
            }
            Err(error) if self.marker.exists() => {
                Err(IdentityError::KeyringUnavailable(error.to_string()))
            }
            Err(error) => {
                warn!(%error, "no keyring: keeping the private key in a file");
                let identity = Identity::generate()?;
                identity.save(&self.file)?;
                Ok(identity)
            }
        }
    }

    /// Stores the key in the keyring and deletes its file.
    async fn move_to_keyring<S: SecretStore>(
        &self,
        keyring: &S,
        identity: &Identity,
    ) -> Result<(), MoveError> {
        self.store(keyring, identity).await?;
        secure_file::erase(&self.file)?;
        Ok(())
    }

    /// Writes the key to the keyring, checks that it reads back the same,
    /// and records that it is there.
    async fn store<S: SecretStore>(
        &self,
        keyring: &S,
        identity: &Identity,
    ) -> Result<(), MoveError> {
        let pem = identity.to_pkcs8_pem();
        keyring
            .write(&self.name, &self.label, pem.as_bytes())
            .await?;
        let stored = keyring.read(&self.name).await?;
        if stored.as_ref().map(|stored| stored.as_slice()) != Some(pem.as_bytes()) {
            return Err(MoveError::Mismatch);
        }
        secure_file::write(&self.marker, MARKER_TEXT)?;
        Ok(())
    }
}

/// The key that seals the presence keys in the pairing file, from the
/// keyring under `name`, or a new one kept there. `None` without a keyring
/// that takes it, or when the keyring fails: a pairing file that was never
/// sealed then keeps the presence keys in clear, and a sealed one keeps new
/// ones only in memory. A key the keyring holds is never replaced after a
/// failed read. Losing the key loses only the presence keys, which each
/// phone gives again at the start of its next session.
pub async fn load_store_key<S: SecretStore>(
    keyring: &S,
    name: &str,
    label: &str,
) -> Option<StoreKey> {
    match keyring.read(name).await {
        Ok(Some(bytes)) => match StoreKey::from_slice(&bytes) {
            Some(key) => return Some(key),
            None => warn!("the keyring holds a malformed store key: making a new one"),
        },
        Ok(None) => {}
        Err(error) => {
            warn!(%error, "no keyring: keeping presence keys in the pairing file");
            return None;
        }
    }
    let key = StoreKey::generate();
    let stored = match keyring.write(name, label, key.as_bytes()).await {
        Ok(()) => keyring.read(name).await,
        Err(error) => Err(error),
    };
    match stored {
        Ok(Some(bytes)) if bytes.as_slice() == key.as_bytes() => Some(key),
        Ok(_) => {
            warn!("the keyring did not keep the store key");
            None
        }
        Err(error) => {
            warn!(%error, "could not keep the store key in the keyring");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::Path;
    use std::sync::Mutex;

    use zeroize::Zeroizing;

    use super::*;
    use crate::identity::CertificateFingerprint;

    /// A keyring in memory that can be switched off or made to lose what
    /// is written to it.
    #[derive(Default)]
    struct FakeKeyring {
        items: Mutex<HashMap<String, Vec<u8>>>,
        unavailable: bool,
        drops_writes: bool,
    }

    impl FakeKeyring {
        fn unavailable() -> Self {
            Self {
                unavailable: true,
                ..Self::default()
            }
        }

        fn holding(name: &str, identity: &Identity) -> Self {
            let keyring = Self::default();
            keyring
                .items
                .lock()
                .unwrap()
                .insert(name.to_owned(), identity.to_pkcs8_pem().as_bytes().to_vec());
            keyring
        }

        fn fingerprint_of(&self, name: &str) -> Option<CertificateFingerprint> {
            let items = self.items.lock().unwrap();
            let pem = items.get(name)?;
            Some(
                Identity::from_pem_bytes(Zeroizing::new(pem.clone()))
                    .unwrap()
                    .fingerprint(),
            )
        }

        fn check(&self) -> Result<(), PlatformError> {
            if self.unavailable {
                return Err(PlatformError::Unavailable {
                    service: "Secret Service",
                    reason: "not running".into(),
                });
            }
            Ok(())
        }
    }

    impl SecretStore for FakeKeyring {
        async fn read(&self, name: &str) -> Result<Option<Zeroizing<Vec<u8>>>, PlatformError> {
            self.check()?;
            Ok(self
                .items
                .lock()
                .unwrap()
                .get(name)
                .cloned()
                .map(Zeroizing::new))
        }

        async fn write(&self, name: &str, label: &str, secret: &[u8]) -> Result<(), PlatformError> {
            self.check()?;
            assert!(label.starts_with("link2linux "), "{label}");
            if !self.drops_writes {
                self.items
                    .lock()
                    .unwrap()
                    .insert(name.to_owned(), secret.to_vec());
            }
            Ok(())
        }
    }

    const NAME: &str = "device-key";

    #[tokio::test]
    async fn the_store_key_is_made_once_and_kept_in_the_keyring() {
        let keyring = FakeKeyring::default();
        let label = "link2linux pairing data key";
        let first = load_store_key(&keyring, "store-key", label).await.unwrap();
        let again = load_store_key(&keyring, "store-key", label).await.unwrap();
        assert_eq!(first.as_bytes(), again.as_bytes());

        // Without a keyring that keeps it, there is no key.
        let none = FakeKeyring::unavailable();
        assert!(load_store_key(&none, "store-key", label).await.is_none());
        let forgetful = FakeKeyring {
            drops_writes: true,
            ..FakeKeyring::default()
        };
        assert!(
            load_store_key(&forgetful, "store-key", label)
                .await
                .is_none()
        );

        // A keyring that does not answer keeps the key it holds, for the
        // next time it does.
        let failing = FakeKeyring::unavailable();
        failing
            .items
            .lock()
            .unwrap()
            .insert("store-key".to_owned(), first.as_bytes().to_vec());
        assert!(load_store_key(&failing, "store-key", label).await.is_none());
        assert_eq!(failing.items.lock().unwrap()["store-key"], first.as_bytes());
    }

    fn storage(dir: &Path) -> KeyStorage {
        KeyStorage {
            name: NAME.into(),
            label: "link2linux device key".into(),
            file: dir.join("identity.pem"),
            marker: dir.join("identity-in-keyring"),
        }
    }

    #[tokio::test]
    async fn a_new_key_goes_to_the_keyring() {
        let dir = tempfile::tempdir().unwrap();
        let storage = storage(dir.path());
        let keyring = FakeKeyring::default();

        let created = storage.load_or_create(&keyring).await.unwrap();
        assert_eq!(keyring.fingerprint_of(NAME), Some(created.fingerprint()));
        assert!(!storage.file.exists());
        assert!(storage.marker.exists());

        let loaded = storage.load_or_create(&keyring).await.unwrap();
        assert_eq!(loaded.fingerprint(), created.fingerprint());
    }

    #[tokio::test]
    async fn without_a_keyring_the_key_lives_in_a_private_file() {
        let dir = tempfile::tempdir().unwrap();
        let storage = storage(dir.path());
        let keyring = FakeKeyring::unavailable();

        let created = storage.load_or_create(&keyring).await.unwrap();
        let mode = std::os::unix::fs::PermissionsExt::mode(
            &std::fs::metadata(&storage.file).unwrap().permissions(),
        );
        assert_eq!(mode & 0o777, 0o600);
        assert!(!storage.marker.exists());

        let loaded = storage.load_or_create(&keyring).await.unwrap();
        assert_eq!(loaded.fingerprint(), created.fingerprint());
    }

    #[tokio::test]
    async fn a_key_file_moves_into_the_keyring_once_it_is_there() {
        let dir = tempfile::tempdir().unwrap();
        let storage = storage(dir.path());
        let old = Identity::generate().unwrap();
        old.save(&storage.file).unwrap();

        // Unreachable at first: the file stays.
        let loaded = storage
            .load_or_create(&FakeKeyring::unavailable())
            .await
            .unwrap();
        assert_eq!(loaded.fingerprint(), old.fingerprint());
        assert!(storage.file.exists());

        let keyring = FakeKeyring::default();
        let loaded = storage.load_or_create(&keyring).await.unwrap();
        assert_eq!(loaded.fingerprint(), old.fingerprint());
        assert_eq!(keyring.fingerprint_of(NAME), Some(old.fingerprint()));
        assert!(!storage.file.exists());
        assert!(storage.marker.exists());
    }

    #[tokio::test]
    async fn the_file_stays_when_the_keyring_does_not_keep_the_key() {
        let dir = tempfile::tempdir().unwrap();
        let storage = storage(dir.path());
        let old = Identity::generate().unwrap();
        old.save(&storage.file).unwrap();
        let keyring = FakeKeyring {
            drops_writes: true,
            ..FakeKeyring::default()
        };

        let loaded = storage.load_or_create(&keyring).await.unwrap();
        assert_eq!(loaded.fingerprint(), old.fingerprint());
        assert!(storage.file.exists());
        assert!(!storage.marker.exists());
    }

    #[tokio::test]
    async fn the_file_wins_over_an_older_key_in_the_keyring() {
        let dir = tempfile::tempdir().unwrap();
        let storage = storage(dir.path());
        let older = Identity::generate().unwrap();
        let newer = Identity::generate().unwrap();
        newer.save(&storage.file).unwrap();
        let keyring = FakeKeyring::holding(NAME, &older);

        let loaded = storage.load_or_create(&keyring).await.unwrap();
        assert_eq!(loaded.fingerprint(), newer.fingerprint());
        assert_eq!(keyring.fingerprint_of(NAME), Some(newer.fingerprint()));
        assert!(!storage.file.exists());
    }

    #[tokio::test]
    async fn an_unreachable_keyring_holding_the_key_stops_the_start() {
        let dir = tempfile::tempdir().unwrap();
        let storage = storage(dir.path());
        let keyring = FakeKeyring::default();
        storage.load_or_create(&keyring).await.unwrap();

        let result = storage.load_or_create(&FakeKeyring::unavailable()).await;
        assert!(
            matches!(result, Err(IdentityError::KeyringUnavailable(_))),
            "{result:?}"
        );
        // No new key was made in its place.
        assert!(!storage.file.exists());
    }

    #[tokio::test]
    async fn a_key_removed_from_the_keyring_is_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let storage = storage(dir.path());
        let first = storage
            .load_or_create(&FakeKeyring::default())
            .await
            .unwrap();

        let emptied = FakeKeyring::default();
        let second = storage.load_or_create(&emptied).await.unwrap();
        assert_ne!(second.fingerprint(), first.fingerprint());
        assert_eq!(emptied.fingerprint_of(NAME), Some(second.fingerprint()));
    }

    #[tokio::test]
    async fn an_invalid_key_in_the_keyring_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let storage = storage(dir.path());
        let keyring = FakeKeyring::default();
        keyring
            .items
            .lock()
            .unwrap()
            .insert(NAME.into(), b"not a key".to_vec());

        assert!(storage.load_or_create(&keyring).await.is_err());
        assert!(!storage.file.exists());
    }
}
