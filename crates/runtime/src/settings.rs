//! The desktop app's own settings, kept as JSON in the data directory.

use std::io;
use std::path::Path;

use link2linux_core::clipboard::ClipboardSettings;
use link2linux_core::secure_file;
use serde::{Deserialize, Serialize};
use specta::Type;

/// What the user chose for the desktop app. Settings missing from the file
/// take their defaults, so older files keep working.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// Pop-ups show the notification's text, not just the app's name. Off
    /// by default, because pop-ups pass through the desktop's notification
    /// service, where other programs can read them.
    pub show_notification_text: bool,
    /// The clipboard is shared with the phones. On by default: each phone
    /// has its own switch, off until its user turns it on.
    pub share_clipboard: bool,
    /// Text that a password manager marked as secret is shared too. Off by
    /// default.
    pub share_passwords: bool,
    /// Files can be sent to the phones and received from them. On by
    /// default: each phone has its own switch, off until its user turns it
    /// on.
    pub share_files: bool,
    /// The connected phone's battery, signal and sound mode are shown. On
    /// by default: they are what the phone's own status bar shows.
    pub show_phone_status: bool,
    /// The phones' calls ring on this computer, and are taken on it over
    /// Bluetooth. On by default: nothing happens until the user pairs a
    /// phone over Bluetooth.
    pub use_for_calls: bool,
}

impl Default for Settings {
    fn default() -> Self {
        let clipboard = ClipboardSettings::default();
        Self {
            show_notification_text: false,
            share_clipboard: clipboard.sharing,
            share_passwords: clipboard.share_passwords,
            share_files: true,
            show_phone_status: true,
            use_for_calls: true,
        }
    }
}

impl Settings {
    /// The clipboard part.
    #[must_use]
    pub fn clipboard(&self) -> ClipboardSettings {
        ClipboardSettings {
            sharing: self.share_clipboard,
            share_passwords: self.share_passwords,
        }
    }

    /// The settings stored at `path`, or the defaults when there are none
    /// or they cannot be read.
    #[must_use]
    pub fn load(path: &Path) -> Self {
        let parsed = secure_file::read(path).map(|bytes| {
            bytes.map(|bytes| serde_json::from_slice::<Self>(&bytes).map_err(io::Error::other))
        });
        match parsed {
            Ok(Some(Ok(settings))) => settings,
            Ok(None) => Self::default(),
            Ok(Some(Err(error))) | Err(error) => {
                tracing::warn!(%error, path = %path.display(), "using the default settings");
                Self::default()
            }
        }
    }

    /// Stores the settings at `path`, readable only by the user.
    ///
    /// # Errors
    ///
    /// Fails when the file cannot be written.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        let json = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        secure_file::write(path, &json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_survive_a_round_trip_and_default_to_hidden_text() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        assert_eq!(Settings::load(&path), Settings::default());
        assert!(!Settings::default().show_notification_text);

        let settings = Settings {
            show_notification_text: true,
            ..Settings::default()
        };
        settings.save(&path).unwrap();
        assert_eq!(Settings::load(&path), settings);
        assert!(
            String::from_utf8(std::fs::read(&path).unwrap())
                .unwrap()
                .contains("\"showNotificationText\": true")
        );

        std::fs::write(&path, b"{ not json").unwrap();
        assert_eq!(Settings::load(&path), Settings::default());
        std::fs::write(&path, b"{}").unwrap();
        assert_eq!(Settings::load(&path), Settings::default());
    }

    #[test]
    fn calls_are_on_unless_the_user_turned_them_off() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        // A file from before calls.
        std::fs::write(&path, br#"{ "shareFiles": false }"#).unwrap();
        assert!(Settings::load(&path).use_for_calls);
        let off = Settings {
            use_for_calls: false,
            ..Settings::default()
        };
        off.save(&path).unwrap();
        assert!(!Settings::load(&path).use_for_calls);
    }

    #[test]
    fn the_clipboard_is_shared_without_passwords_unless_the_user_chose_otherwise() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        // A file from before the clipboard was shared.
        std::fs::write(&path, br#"{ "showNotificationText": true }"#).unwrap();
        let settings = Settings::load(&path);
        assert!(settings.show_notification_text);
        assert_eq!(settings.clipboard(), ClipboardSettings::default());
        assert!(settings.share_clipboard && !settings.share_passwords);

        let chosen = Settings {
            share_clipboard: false,
            share_passwords: true,
            ..settings
        };
        chosen.save(&path).unwrap();
        let json = String::from_utf8(std::fs::read(&path).unwrap()).unwrap();
        assert!(json.contains("\"shareClipboard\": false"), "{json}");
        assert!(json.contains("\"sharePasswords\": true"), "{json}");
        assert_eq!(
            Settings::load(&path).clipboard(),
            ClipboardSettings {
                sharing: false,
                share_passwords: true,
            }
        );
    }

    #[test]
    fn files_are_on_unless_the_user_turned_them_off() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        // A file from before files were sent.
        std::fs::write(&path, br#"{ "shareClipboard": false }"#).unwrap();
        assert!(Settings::load(&path).share_files);

        let off = Settings {
            share_files: false,
            ..Settings::default()
        };
        off.save(&path).unwrap();
        assert_eq!(Settings::load(&path), off);
        assert!(
            String::from_utf8(std::fs::read(&path).unwrap())
                .unwrap()
                .contains("\"shareFiles\": false")
        );
    }

    #[test]
    fn the_phone_status_shows_unless_the_user_hid_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        // A file from before the phone's status was shown.
        std::fs::write(&path, br#"{ "shareFiles": false }"#).unwrap();
        assert!(Settings::load(&path).show_phone_status);

        let hidden = Settings {
            show_phone_status: false,
            ..Settings::default()
        };
        hidden.save(&path).unwrap();
        assert_eq!(Settings::load(&path), hidden);
        assert!(
            String::from_utf8(std::fs::read(&path).unwrap())
                .unwrap()
                .contains("\"showPhoneStatus\": false")
        );
    }
}
