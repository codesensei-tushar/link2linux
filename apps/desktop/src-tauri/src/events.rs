//! Events the backend pushes to the UI.
//!
//! Each event carries a full snapshot, so the UI never has to merge deltas.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::AppHandle;
use tauri_specta::Event;

use crate::backend::{Device, EventSink, NotificationEvent, PairingState};

/// The pairing step changed.
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
pub struct PairingChanged(pub PairingState);

/// The list of paired phones, or one phone's status, changed.
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
pub struct DevicesChanged(pub Vec<Device>);

/// A phone's mirrored notifications changed.
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct NotificationChanged {
    pub device_id: String,
    pub event: NotificationEvent,
}

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

    fn notification_changed(&self, device_id: &str, event: &NotificationEvent) {
        let _ = NotificationChanged {
            device_id: device_id.to_owned(),
            event: event.clone(),
        }
        .emit(&self.app);
    }
}
