//! The desktop app's own settings, kept as JSON in the data directory.

use std::io;
use std::path::Path;

use link2linux_core::secure_file;
use serde::{Deserialize, Serialize};
use specta::Type;

/// What the user chose for the desktop app. Settings missing from the file
/// take their defaults, so older files keep working.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// Pop-ups show the notification's text, not just the app's name. Off
    /// by default, because pop-ups pass through the desktop's notification
    /// service, where other programs can read them.
    pub show_notification_text: bool,
}

impl Settings {
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
}
