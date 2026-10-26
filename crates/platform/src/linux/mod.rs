//! Linux implementations of the platform traits.
//!
//! Mostly stubs for now: each type names the Linux service it will use, and
//! until then its methods fail with [`PlatformError::Unsupported`], so the
//! app can start and show what is missing. [`XdgFileStore::downloads_dir`]
//! already works.

pub mod ble;
pub mod notifications;
pub mod secrets;
pub mod session_lock;

pub use notifications::DbusNotifications;
pub use secrets::SecretServiceStore;
pub use session_lock::SessionLock;

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use crate::{CallAudio, Clipboard, FileStore, PlatformError};

/// The desktop clipboard: Wayland data-control where the compositor offers
/// it, X11 selections otherwise.
#[derive(Debug, Default)]
pub struct LinuxClipboard;

impl Clipboard for LinuxClipboard {
    async fn read_text(&self) -> Result<Option<String>, PlatformError> {
        Err(PlatformError::Unsupported("clipboard"))
    }

    async fn write_text(&self, _text: &str) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported("clipboard"))
    }

    async fn changed(&self) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported("clipboard"))
    }
}

/// Files in the XDG user directories, opened through the desktop's default
/// applications.
#[derive(Debug, Default)]
pub struct XdgFileStore;

impl FileStore for XdgFileStore {
    /// `XDG_DOWNLOAD_DIR` from the environment or from
    /// `$XDG_CONFIG_HOME/user-dirs.dirs`, falling back to `~/Downloads`.
    fn downloads_dir(&self) -> Result<PathBuf, PlatformError> {
        let home = env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or(PlatformError::Unavailable {
                service: "XDG user directories",
                reason: "HOME is not set".into(),
            })?;
        if let Some(dir) = env::var_os("XDG_DOWNLOAD_DIR") {
            return Ok(PathBuf::from(dir));
        }
        let config_home = env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"));
        let from_file = fs::read_to_string(config_home.join("user-dirs.dirs"))
            .ok()
            .and_then(|contents| download_dir_from_user_dirs(&contents, &home));
        Ok(from_file.unwrap_or_else(|| home.join("Downloads")))
    }

    async fn open(&self, _path: &Path) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported("opening files"))
    }
}

/// Reads `XDG_DOWNLOAD_DIR` from the contents of a `user-dirs.dirs` file,
/// whose values look like `"$HOME/Downloads"` or `"/absolute/path"`.
fn download_dir_from_user_dirs(contents: &str, home: &Path) -> Option<PathBuf> {
    let value = contents.lines().find_map(|line| {
        line.trim()
            .strip_prefix("XDG_DOWNLOAD_DIR=")
            .map(|value| value.trim_matches('"'))
    })?;
    if let Some(rest) = value.strip_prefix("$HOME") {
        Some(home.join(rest.trim_start_matches('/')))
    } else if value.starts_with('/') {
        Some(PathBuf::from(value))
    } else {
        None
    }
}

/// Call audio through BlueZ (hands-free profile) and PipeWire.
#[derive(Debug, Default)]
pub struct BluezCallAudio;

impl CallAudio for BluezCallAudio {
    async fn connect(&self, _phone: &str) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported("call audio"))
    }

    async fn disconnect(&self) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported("call audio"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_download_dir_from_user_dirs() {
        let home = Path::new("/home/ana");
        let contents = "# written by xdg-user-dirs-update\n\
                        XDG_DESKTOP_DIR=\"$HOME/Desktop\"\n\
                        XDG_DOWNLOAD_DIR=\"$HOME/Téléchargements\"\n";
        assert_eq!(
            download_dir_from_user_dirs(contents, home),
            Some(PathBuf::from("/home/ana/Téléchargements"))
        );
        assert_eq!(
            download_dir_from_user_dirs("XDG_DOWNLOAD_DIR=\"/data/dl\"", home),
            Some(PathBuf::from("/data/dl"))
        );
        assert_eq!(
            download_dir_from_user_dirs("XDG_DOWNLOAD_DIR=\"$HOME/\"", home),
            Some(PathBuf::from("/home/ana/"))
        );
        assert_eq!(
            download_dir_from_user_dirs("XDG_MUSIC_DIR=\"$HOME/Music\"", home),
            None
        );
    }

    #[tokio::test]
    async fn stubs_report_unsupported() {
        assert!(matches!(
            LinuxClipboard.read_text().await,
            Err(PlatformError::Unsupported(_))
        ));
        assert!(matches!(
            BluezCallAudio.connect("00:11:22:33:44:55").await,
            Err(PlatformError::Unsupported(_))
        ));
    }
}
