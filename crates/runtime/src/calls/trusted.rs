//! The Bluetooth devices this desktop trusted itself to connect their
//! hands-free link, kept across runs in `calls-trusted.json` in the data
//! directory: forgetting a phone takes back only a trust the desktop gave,
//! not one the user gave in the desktop's Bluetooth settings.

use std::collections::{BTreeSet, HashSet};
use std::io;
use std::path::Path;

use link2linux_core::secure_file;
use link2linux_platform::BluetoothAddress;
use serde::{Deserialize, Serialize};

/// The file's content.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct File {
    devices: BTreeSet<String>,
}

/// The devices stored at `path`; none when there are none or they cannot
/// be read.
pub(crate) fn load(path: &Path) -> HashSet<BluetoothAddress> {
    let parsed = secure_file::read(path).map(|bytes| {
        bytes.map(|bytes| serde_json::from_slice::<File>(&bytes).map_err(io::Error::other))
    });
    let file = match parsed {
        Ok(Some(Ok(file))) => file,
        Ok(None) => File::default(),
        Ok(Some(Err(error))) | Err(error) => {
            tracing::warn!(%error, "could not read the Bluetooth devices this computer trusted");
            File::default()
        }
    };
    file.devices
        .into_iter()
        .filter_map(|address| address.parse().ok())
        .collect()
}

/// Stores `devices` at `path`, readable only by the user.
///
/// # Errors
///
/// Fails when the file cannot be written.
pub(crate) fn save(path: &Path, devices: &HashSet<BluetoothAddress>) -> io::Result<()> {
    let file = File {
        devices: devices.iter().map(ToString::to_string).collect(),
    };
    let json = serde_json::to_vec_pretty(&file).map_err(io::Error::other)?;
    secure_file::write(path, &json)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trusted_devices_survive_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("calls-trusted.json");
        assert!(load(&path).is_empty());
        let devices: HashSet<BluetoothAddress> =
            ["0C:1A:2B:3C:4D:5E".parse().unwrap()].into_iter().collect();
        save(&path, &devices).unwrap();
        assert_eq!(load(&path), devices);
        std::fs::write(&path, "not json").unwrap();
        assert!(load(&path).is_empty());
    }
}
