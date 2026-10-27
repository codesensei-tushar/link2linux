//! The phones' calls, which the user takes and makes on this computer over
//! Bluetooth. Each command starts something and returns at once; the new
//! state arrives as a `CallsChanged` event. "Use this computer for calls"
//! is a setting (`useForCalls`).

use tauri::State;

use crate::backend::CallsStatus;
use crate::commands::CommandError;
use crate::state::AppState;

/// The phones' calls and this computer's part in them: whether it can take
/// calls, Bluetooth pairing, and each phone's link and calls. Later changes
/// arrive as `CallsChanged` events: subscribe first, then call this.
#[tauri::command]
#[specta::specta]
pub fn get_calls_status(state: State<'_, AppState>) -> CallsStatus {
    state.backend().calls()
}

/// Opens Bluetooth pairing with a phone for 2 minutes, and asks the phone
/// to pair.
#[tauri::command]
#[specta::specta]
pub fn start_bluetooth_pairing(
    state: State<'_, AppState>,
    device_id: String,
) -> Result<(), CommandError> {
    state
        .backend()
        .start_bluetooth_pairing(&device_id)
        .map_err(Into::into)
}

/// Closes Bluetooth pairing.
#[tauri::command]
#[specta::specta]
pub fn cancel_bluetooth_pairing(state: State<'_, AppState>) {
    state.backend().cancel_bluetooth_pairing();
}

/// The user compared the code of pairing prompt `prompt_id`: `matches`
/// says whether both devices show the same one. Pairing goes ahead only
/// when it does.
#[tauri::command]
#[specta::specta]
pub fn confirm_bluetooth_code(
    state: State<'_, AppState>,
    prompt_id: u32,
    matches: bool,
) -> Result<(), CommandError> {
    state
        .backend()
        .confirm_bluetooth_code(prompt_id, matches)
        .map_err(Into::into)
}

/// The user picked the paired Bluetooth device at `address` as the phone's.
#[tauri::command]
#[specta::specta]
pub fn choose_bluetooth_device(
    state: State<'_, AppState>,
    device_id: String,
    address: String,
) -> Result<(), CommandError> {
    state
        .backend()
        .choose_bluetooth_device(&device_id, &address)
        .map_err(Into::into)
}

/// Connects the phone's hands-free link with this computer.
#[tauri::command]
#[specta::specta]
pub fn connect_calls(state: State<'_, AppState>, device_id: String) -> Result<(), CommandError> {
    state
        .backend()
        .connect_calls(&device_id)
        .map_err(Into::into)
}

/// Answers a ringing call.
#[tauri::command]
#[specta::specta]
pub fn answer_call(
    state: State<'_, AppState>,
    device_id: String,
    call_id: String,
) -> Result<(), CommandError> {
    state
        .backend()
        .answer_call(&device_id, &call_id)
        .map_err(Into::into)
}

/// Declines a ringing call.
#[tauri::command]
#[specta::specta]
pub fn decline_call(
    state: State<'_, AppState>,
    device_id: String,
    call_id: String,
) -> Result<(), CommandError> {
    state
        .backend()
        .decline_call(&device_id, &call_id)
        .map_err(Into::into)
}

/// Ends a call.
#[tauri::command]
#[specta::specta]
pub fn hang_up_call(
    state: State<'_, AppState>,
    device_id: String,
    call_id: String,
) -> Result<(), CommandError> {
    state
        .backend()
        .hang_up_call(&device_id, &call_id)
        .map_err(Into::into)
}

/// Calls `number` from the phone, over its hands-free link.
#[tauri::command]
#[specta::specta]
pub fn dial_number(
    state: State<'_, AppState>,
    device_id: String,
    number: String,
) -> Result<(), CommandError> {
    state
        .backend()
        .dial(&device_id, &number)
        .map_err(Into::into)
}

/// Sends touch tones, such as `1` or `#`, in the phone's active call.
#[tauri::command]
#[specta::specta]
pub fn send_call_tones(
    state: State<'_, AppState>,
    device_id: String,
    tones: String,
) -> Result<(), CommandError> {
    state
        .backend()
        .send_tones(&device_id, &tones)
        .map_err(Into::into)
}

/// Mutes or unmutes this computer's microphone in the phone's calls.
#[tauri::command]
#[specta::specta]
pub fn set_call_muted(
    state: State<'_, AppState>,
    device_id: String,
    muted: bool,
) -> Result<(), CommandError> {
    state
        .backend()
        .set_call_muted(&device_id, muted)
        .map_err(Into::into)
}
