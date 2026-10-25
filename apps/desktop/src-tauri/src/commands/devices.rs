use serde::Serialize;
use specta::Type;
use tauri::State;

use crate::state::AppState;

/// A phone paired with this desktop, as the UI sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    /// Stable identifier assigned at pairing time.
    pub id: String,
    /// Name the user gave the phone.
    pub name: String,
    /// Hardware model, when the phone reports one.
    pub model: Option<String>,
    pub status: DeviceStatus,
}

/// Whether a paired phone currently has a live session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum DeviceStatus {
    Connected,
    Disconnected,
}

/// Lists paired phones.
#[tauri::command]
#[specta::specta]
pub fn list_devices(state: State<'_, AppState>) -> Vec<Device> {
    state.devices()
}
