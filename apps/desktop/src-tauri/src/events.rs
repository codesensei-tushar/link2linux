//! Events the backend pushes to the UI.
//!
//! Each event carries a full snapshot, so the UI never has to merge deltas.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::AppHandle;
use tauri_specta::Event;

use crate::backend::{
    CallsStatus, ClipboardStatus, Device, EventSink, FileTransferEvent, NotificationEvent,
    PairingState, PhoneStatus, RingStatus, ScreenStatus,
};

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

/// The shared clipboard's state changed.
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
pub struct ClipboardChanged(pub ClipboardStatus);

/// The phones' calls, or this computer's part in them, changed.
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
pub struct CallsChanged(pub CallsStatus);

/// A file transfer with a phone changed.
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct TransferChanged {
    pub device_id: String,
    pub event: FileTransferEvent,
}

/// What the UI shows of a phone's status changed.
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct PhoneStatusChanged {
    pub device_id: String,
    pub status: PhoneStatus,
}

/// What the UI shows of ringing a phone changed.
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct RingChanged {
    pub device_id: String,
    pub ring: RingStatus,
}

/// What the UI shows of a phone's screen changed. The frames themselves go
/// through the channel `start_screen` was given.
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct ScreenChanged {
    pub device_id: String,
    pub screen: ScreenStatus,
}

/// Files are dragged over the window (`over`), or not anymore.
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
pub struct FileDrag {
    pub over: bool,
}

/// Files were dropped on the window. `send_dropped_files` with `drop_id`
/// sends them; `names` are the dropped items, for the page to show.
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct FilesDropped {
    pub drop_id: u32,
    pub names: Vec<String>,
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

    fn clipboard_changed(&self, status: &ClipboardStatus) {
        let _ = ClipboardChanged(status.clone()).emit(&self.app);
    }

    fn transfer_changed(&self, device_id: &str, event: &FileTransferEvent) {
        let _ = TransferChanged {
            device_id: device_id.to_owned(),
            event: event.clone(),
        }
        .emit(&self.app);
    }

    fn phone_status_changed(&self, device_id: &str, status: &PhoneStatus) {
        let _ = PhoneStatusChanged {
            device_id: device_id.to_owned(),
            status: status.clone(),
        }
        .emit(&self.app);
    }

    fn ring_changed(&self, device_id: &str, ring: &RingStatus) {
        let _ = RingChanged {
            device_id: device_id.to_owned(),
            ring: *ring,
        }
        .emit(&self.app);
    }

    fn calls_changed(&self, status: &CallsStatus) {
        let _ = CallsChanged(status.clone()).emit(&self.app);
    }

    fn screen_changed(&self, device_id: &str, screen: &ScreenStatus) {
        let _ = ScreenChanged {
            device_id: device_id.to_owned(),
            screen: screen.clone(),
        }
        .emit(&self.app);
    }
}
