//! Which Bluetooth device is each paired phone, kept across runs in
//! `calls.json` in the data directory.

use std::collections::BTreeMap;
use std::io;
use std::path::Path;

use link2linux_core::secure_file;
use link2linux_platform::BluetoothAddress;
use serde::{Deserialize, Serialize};

/// The file's content.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct File {
    /// Each phone's device address, by the phone's device id.
    devices: BTreeMap<String, String>,
}

/// The choices stored at `path`; none when there are none or they cannot
/// be read.
pub(crate) fn load(path: &Path) -> BTreeMap<String, BluetoothAddress> {
    let parsed = secure_file::read(path).map(|bytes| {
        bytes.map(|bytes| serde_json::from_slice::<File>(&bytes).map_err(io::Error::other))
    });
    let file = match parsed {
        Ok(Some(Ok(file))) => file,
        Ok(None) => File::default(),
        Ok(Some(Err(error))) | Err(error) => {
            tracing::warn!(%error, "could not read the phones' Bluetooth devices");
            File::default()
        }
    };
    file.devices
        .into_iter()
        .filter_map(|(id, address)| Some((id, address.parse().ok()?)))
        .collect()
}

/// Stores `choices` at `path`, readable only by the user.
///
/// # Errors
///
/// Fails when the file cannot be written.
pub(crate) fn save(path: &Path, choices: &BTreeMap<String, BluetoothAddress>) -> io::Result<()> {
    let file = File {
        devices: choices
            .iter()
            .map(|(id, address)| (id.clone(), address.to_string()))
            .collect(),
    };
    let json = serde_json::to_vec_pretty(&file).map_err(io::Error::other)?;
    secure_file::write(path, &json)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choices_survive_a_restart_and_bad_entries_are_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("calls.json");
        assert!(load(&path).is_empty());
        let mut choices = BTreeMap::new();
        choices.insert("p1".to_owned(), "0C:1A:2B:3C:4D:5E".parse().unwrap());
        save(&path, &choices).unwrap();
        assert_eq!(load(&path), choices);

        std::fs::write(
            &path,
            r#"{"devices":{"p1":"nonsense","p2":"00:11:22:33:44:55"}}"#,
        )
        .unwrap();
        let loaded = load(&path);
        assert_eq!(loaded.len(), 1);
        assert!(loaded.contains_key("p2"));
        std::fs::write(&path, "not json").unwrap();
        assert!(load(&path).is_empty());
    }
}
