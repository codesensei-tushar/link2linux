use tauri::State;

use crate::backend::{PairingMethod, PairingState, PhoneTarget};
use crate::commands::CommandError;
use crate::state::AppState;

/// The current pairing step. Call it once after subscribing to
/// `PairingChanged`; later changes arrive as events.
#[tauri::command]
#[specta::specta]
pub fn get_pairing_state(state: State<'_, AppState>) -> PairingState {
    state.backend().pairing()
}

/// Starts pairing a new phone. Progress arrives as `PairingChanged` events.
#[tauri::command]
#[specta::specta]
pub fn start_pairing(
    state: State<'_, AppState>,
    method: PairingMethod,
) -> Result<(), CommandError> {
    state.backend().start_pairing(method).map_err(Into::into)
}

/// Dials a phone from the search list, or an address the user typed.
#[tauri::command]
#[specta::specta]
pub fn pair_with(state: State<'_, AppState>, target: PhoneTarget) -> Result<(), CommandError> {
    state.backend().pair_with(target).map_err(Into::into)
}

/// Answers "do both screens show the same code?".
#[tauri::command]
#[specta::specta]
pub fn answer_code(state: State<'_, AppState>, matches: bool) -> Result<(), CommandError> {
    state.backend().answer_code(matches).map_err(Into::into)
}

/// Stops pairing, or dismisses a finished attempt.
#[tauri::command]
#[specta::specta]
pub fn cancel_pairing(state: State<'_, AppState>) {
    state.backend().cancel_pairing();
}
