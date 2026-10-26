use tauri::State;

use crate::backend::ClipboardStatus;
use crate::commands::CommandError;
use crate::state::AppState;

/// The state of the shared clipboard: how this computer follows it, each
/// phone's state and what went where lately, never the text. Later changes
/// arrive as `ClipboardChanged` events: subscribe first, then call this.
#[tauri::command]
#[specta::specta]
pub fn get_clipboard_status(state: State<'_, AppState>) -> ClipboardStatus {
    state.backend().clipboard()
}

/// Sends what the clipboard holds to every phone that takes it. What came
/// of it arrives as a `ClipboardChanged` event.
#[tauri::command]
#[specta::specta]
pub fn send_clipboard(state: State<'_, AppState>) -> Result<(), CommandError> {
    state.backend().send_clipboard().map_err(Into::into)
}
