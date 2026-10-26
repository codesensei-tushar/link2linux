//! Events the backend pushes to the UI.
//!
//! Each event carries a full snapshot, so the UI never has to merge deltas.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::AppHandle;
use tauri_specta::Event;

use crate::backend::{Device, EventSink, PairingState};

/// The pairing step changed.
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
pub struct PairingChanged(pub PairingState);

/// The list of paired phones, or one phone's status, changed.
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
pub struct DevicesChanged(pub Vec<Device>);

/// Forwards backend changes to the window as Tauri events.
pub struct TauriSink {
    app: AppHandle,
}

impl TauriSink {
    pub fn new(app: AppHandle) -> Arc<Self> {
        Arc::new(Self { app })
    }
}

impl EventSink for TauriSink {
    fn pairing_changed(&self, state: &PairingState) {
        // A failed emit means the window is gone, which needs no handling.
        let _ = PairingChanged(state.clone()).emit(&self.app);
    }

    fn devices_changed(&self, devices: &[Device]) {
        let _ = DevicesChanged(devices.to_vec()).emit(&self.app);
    }
}
