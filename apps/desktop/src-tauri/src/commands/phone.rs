//! The phone's status, and ringing the phone so the user finds it.

use tauri::State;

use crate::backend::{PhoneStatus, RingStatus};
use crate::commands::CommandError;
use crate::state::AppState;

/// A phone's battery, signal and sound mode. Later changes arrive as
/// `PhoneStatusChanged` events: subscribe first, then call this.
#[tauri::command]
#[specta::specta]
pub fn get_phone_status(
    state: State<'_, AppState>,
    device_id: String,
) -> Result<PhoneStatus, CommandError> {
    state.backend().phone_status(&device_id).map_err(Into::into)
}

/// Whether a phone rings, and why it cannot or did not. Later changes
/// arrive as `RingChanged` events: subscribe first, then call this.
#[tauri::command]
#[specta::specta]
pub fn get_ring_status(
    state: State<'_, AppState>,
    device_id: String,
) -> Result<RingStatus, CommandError> {
    state.backend().ring_status(&device_id).map_err(Into::into)
}

/// Makes a phone ring for a minute, even when it is set to silent. What
/// came of it arrives as a `RingChanged` event.
#[tauri::command]
#[specta::specta]
pub fn ring_phone(state: State<'_, AppState>, device_id: String) -> Result<(), CommandError> {
    state.backend().ring_phone(&device_id).map_err(Into::into)
}

/// Stops a phone ringing. The new state arrives as a `RingChanged` event.
#[tauri::command]
#[specta::specta]
pub fn stop_ringing(state: State<'_, AppState>, device_id: String) -> Result<(), CommandError> {
    state.backend().stop_ringing(&device_id).map_err(Into::into)
}
