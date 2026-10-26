use tauri::State;

use crate::backend::Settings;
use crate::commands::CommandError;
use crate::state::AppState;

/// The desktop app's settings.
#[tauri::command]
#[specta::specta]
pub fn get_settings(state: State<'_, AppState>) -> Settings {
    state.backend().settings()
}

/// Changes the settings. They apply at once and are kept for the next
/// start.
#[tauri::command]
#[specta::specta]
pub fn set_settings(state: State<'_, AppState>, settings: Settings) -> Result<(), CommandError> {
    state.backend().set_settings(settings).map_err(Into::into)
}
