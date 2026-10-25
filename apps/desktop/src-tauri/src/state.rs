//! State that Tauri manages for the lifetime of the app.
//!
//! For now this is an in-memory stub so the UI has something real to call.
//!
//! TODO(core): back `AppState` with a handle to `crates/core` (device
//! registry, pairing, sessions) once that crate is merged. Command handlers
//! only call the methods on `AppState`, so swapping the backend here does not
//! change them or the generated TypeScript bindings.

use std::sync::RwLock;

use crate::commands::devices::Device;

/// Shared app state, registered with `tauri::Builder::manage`.
#[derive(Debug, Default)]
pub struct AppState {
    devices: RwLock<Vec<Device>>,
}

impl AppState {
    /// Creates state that already knows about `devices`.
    pub fn with_devices(devices: Vec<Device>) -> Self {
        Self {
            devices: RwLock::new(devices),
        }
    }

    /// Phones this desktop has paired with, in pairing order.
    pub fn devices(&self) -> Vec<Device> {
        match self.devices.read() {
            Ok(devices) => devices.clone(),
            // A panic while holding the lock cannot leave a Vec half-written,
            // so the data is still safe to read.
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::devices::DeviceStatus;

    #[test]
    fn starts_with_no_devices() {
        assert!(AppState::default().devices().is_empty());
    }

    #[test]
    fn returns_known_devices_in_order() {
        let phone = |id: &str| Device {
            id: id.to_owned(),
            name: format!("Phone {id}"),
            model: None,
            status: DeviceStatus::Disconnected,
        };
        let state = AppState::with_devices(vec![phone("a"), phone("b")]);

        let ids: Vec<_> = state.devices().into_iter().map(|d| d.id).collect();
        assert_eq!(ids, ["a", "b"]);
    }
}
