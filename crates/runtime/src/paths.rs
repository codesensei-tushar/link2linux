//! Where the desktop keeps its files, and who it says it is.

use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use link2linux_core::identity::{Identity, IdentityError};
use link2linux_core::key_storage::KeyStorage;
use link2linux_core::outbox::IdempotencyKey;
use link2linux_platform::SecretStore;

/// The directory under the user's data directory that holds this app's
/// files.
const APP_DIR: &str = "link2linux";

/// The files the runtime keeps, in one directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataDir {
    root: PathBuf,
}

impl DataDir {
    /// A data directory at `root`.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// `$XDG_DATA_HOME/link2linux`, or `~/.local/share/link2linux` when it is
    /// not set (or not absolute, which the XDG rules say to ignore).
    ///
    /// # Errors
    ///
    /// Fails when neither `XDG_DATA_HOME` nor `HOME` is usable.
    pub fn from_env() -> io::Result<Self> {
        Self::from_vars(std::env::var_os("XDG_DATA_HOME"), std::env::var_os("HOME"))
    }

    /// Like [`Self::from_env`] with the variables given, for tests.
    ///
    /// # Errors
    ///
    /// Fails when neither variable is usable.
    pub fn from_vars(xdg_data_home: Option<OsString>, home: Option<OsString>) -> io::Result<Self> {
        let base = xdg_data_home
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| {
                home.map(PathBuf::from)
                    .filter(|path| path.is_absolute())
                    .map(|home| home.join(".local/share"))
            })
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "neither XDG_DATA_HOME nor HOME names an absolute path",
                )
            })?;
        Ok(Self::new(base.join(APP_DIR)))
    }

    /// The directory itself.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// This desktop's private key, when no keyring holds it.
    #[must_use]
    pub fn identity(&self) -> PathBuf {
        self.root.join("identity.pem")
    }

    /// The note that the private key is in the keyring.
    #[must_use]
    pub fn identity_marker(&self) -> PathBuf {
        self.root.join("identity-in-keyring")
    }

    /// The paired phones.
    #[must_use]
    pub fn trust_store(&self) -> PathBuf {
        self.root.join("paired-devices.json")
    }

    /// This installation's device id.
    #[must_use]
    pub fn device_id(&self) -> PathBuf {
        self.root.join("device-id")
    }

    /// The desktop app's settings.
    #[must_use]
    pub fn settings(&self) -> PathBuf {
        self.root.join("settings.json")
    }

    /// The pictures phones shared with notifications, one folder per phone.
    #[must_use]
    pub fn images(&self) -> PathBuf {
        self.root.join("images")
    }

    /// Creates the directory, readable only by the user.
    ///
    /// # Errors
    ///
    /// Fails when the directory cannot be created.
    pub fn create(&self) -> io::Result<()> {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&self.root)
    }

    /// This desktop's identity, from the keyring or the private file, or
    /// a new one. The keyring keeps one key per installation, named after
    /// its device id. Unlocking the keyring may prompt the user.
    ///
    /// # Errors
    ///
    /// Fails when the key cannot be read or stored; see
    /// [`KeyStorage::load_or_create`].
    pub async fn load_identity<S: SecretStore>(
        &self,
        keyring: &S,
    ) -> Result<Identity, IdentityError> {
        self.create()?;
        let device_id = self.load_or_create_device_id()?;
        let storage = KeyStorage {
            name: format!("device-key {device_id}"),
            label: "link2linux device key".to_owned(),
            file: self.identity(),
            marker: self.identity_marker(),
        };
        storage.load_or_create(keyring).await
    }

    /// The id the phones know this desktop by: read from the data directory,
    /// or made and stored on first use.
    ///
    /// # Errors
    ///
    /// Fails when the file cannot be read or written.
    pub fn load_or_create_device_id(&self) -> io::Result<String> {
        let path = self.device_id();
        match fs::read_to_string(&path) {
            Ok(text) if !text.trim().is_empty() => return Ok(text.trim().to_owned()),
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let id = format!("desktop-{}", IdempotencyKey::random());
        fs::write(&path, &id)?;
        Ok(id)
    }
}

/// The name shown on the phone for this computer: its host name.
#[must_use]
pub fn desktop_name() -> String {
    fs::read_to_string("/etc/hostname")
        .ok()
        .map(|text| text.trim().to_owned())
        .filter(|name| !name.is_empty())
        .or_else(|| {
            std::env::var("HOSTNAME")
                .ok()
                .filter(|name| !name.is_empty())
        })
        .unwrap_or_else(|| "Linux desktop".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follows_the_xdg_rules() {
        let dir = DataDir::from_vars(Some("/data".into()), Some("/home/a".into())).unwrap();
        assert_eq!(dir.root(), Path::new("/data/link2linux"));
        let dir = DataDir::from_vars(Some("relative".into()), Some("/home/a".into())).unwrap();
        assert_eq!(dir.root(), Path::new("/home/a/.local/share/link2linux"));
        assert_eq!(
            dir.trust_store(),
            Path::new("/home/a/.local/share/link2linux/paired-devices.json")
        );
        assert!(DataDir::from_vars(None, Some("home".into())).is_err());
        assert!(DataDir::from_vars(None, None).is_err());
    }

    #[test]
    fn the_device_id_is_made_once_in_a_private_directory() {
        let temp = tempfile::tempdir().unwrap();
        let dir = DataDir::new(temp.path().join("nested/link2linux"));
        dir.create().unwrap();
        let first = dir.load_or_create_device_id().unwrap();
        assert!(first.starts_with("desktop-") && first.len() > 16);
        assert_eq!(dir.load_or_create_device_id().unwrap(), first);
        let mode = std::os::unix::fs::PermissionsExt::mode(
            &fs::metadata(dir.root()).unwrap().permissions(),
        );
        assert_eq!(mode & 0o777, 0o700);
    }

    #[test]
    fn the_name_is_never_empty() {
        assert!(!desktop_name().is_empty());
    }
}
