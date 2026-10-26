use tauri::State;

use crate::backend::Device;
use crate::commands::CommandError;
use crate::state::AppState;

/// Lists paired phones.
#[tauri::command]
#[specta::specta]
pub fn list_devices(state: State<'_, AppState>) -> Vec<Device> {
    state.backend().devices()
}

/// Forgets a paired phone. The new list arrives as a `DevicesChanged` event.
#[tauri::command]
#[specta::specta]
pub fn unpair_device(state: State<'_, AppState>, device_id: String) -> Result<(), CommandError> {
    state.backend().unpair(&device_id).map_err(Into::into)
}
