//! File transfers: sending files to a phone, cancelling, and opening what
//! arrived.
//!
//! The page never names local paths. Files to send come from the file
//! chooser this side opens, or from a drop on the window that the shell
//! saw itself (`FilesDropped`); received files are named by their transfer
//! and position.

use std::path::PathBuf;

use tauri::{AppHandle, Manager, State};
use tauri_plugin_dialog::DialogExt;

use crate::backend::{BackendError, FileTransfer};
use crate::commands::CommandError;
use crate::state::AppState;

/// The file transfers with a phone, oldest first. Later changes arrive as
/// `TransferChanged` events: subscribe first, then call this.
#[tauri::command]
#[specta::specta]
pub fn list_transfers(
    state: State<'_, AppState>,
    device_id: String,
) -> Result<Vec<FileTransfer>, CommandError> {
    state.backend().transfers(&device_id).map_err(Into::into)
}

/// Asks the user for files with the desktop's file chooser and sends them
/// to the phone as one transfer. Resolves with whether the user chose any.
#[tauri::command]
#[specta::specta]
pub async fn pick_files_to_send(
    app: AppHandle,
    state: State<'_, AppState>,
    device_id: String,
) -> Result<bool, CommandError> {
    let (chosen, answer) = tokio::sync::oneshot::channel();
    let mut dialog = app.dialog().file().set_title("Send files");
    if let Some(window) = app.get_webview_window("main") {
        dialog = dialog.set_parent(&window);
    }
    dialog.pick_files(move |files| {
        let _ = chosen.send(files);
    });
    let Some(files) = answer.await.ok().flatten() else {
        return Ok(false);
    };
    let paths: Vec<PathBuf> = files
        .into_iter()
        .filter_map(|file| file.into_path().ok())
        .collect();
    if paths.is_empty() {
        return Ok(false);
    }
    send(&state, device_id, paths).await?;
    Ok(true)
}

/// Sends the files of the latest drop on the window to a phone, as one
/// transfer. `drop_id` comes from the `FilesDropped` event; an earlier drop
/// is gone.
#[tauri::command]
#[specta::specta]
pub async fn send_dropped_files(
    state: State<'_, AppState>,
    drop_id: u32,
    device_id: String,
) -> Result<(), CommandError> {
    let paths = state.take_drop(drop_id).ok_or(BackendError::NotFound)?;
    require_absolute(&paths)?;
    send(&state, device_id, paths).await
}

/// Refuses dropped paths that are not absolute: the window reports a
/// dragged link or an odd URI as a relative path, which would be read from
/// this process's directory.
fn require_absolute(paths: &[PathBuf]) -> Result<(), BackendError> {
    if paths.iter().all(|path| path.is_absolute()) {
        Ok(())
    } else {
        Err(BackendError::FileUnreadable)
    }
}

/// Checks and sends the files away from the async threads: looking at
/// hundreds of files can take a moment.
async fn send(
    state: &State<'_, AppState>,
    device_id: String,
    paths: Vec<PathBuf>,
) -> Result<(), CommandError> {
    let backend = state.shared_backend();
    tauri::async_runtime::spawn_blocking(move || backend.send_files(&device_id, paths))
        .await
        .map_err(|_| BackendError::WrongState)?
        .map_err(Into::into)
}

/// Cancels a transfer that has not finished, in either direction.
#[tauri::command]
#[specta::specta]
pub fn cancel_transfer(
    state: State<'_, AppState>,
    device_id: String,
    transfer_id: String,
) -> Result<(), CommandError> {
    state
        .backend()
        .cancel_transfer(&device_id, &transfer_id)
        .map_err(Into::into)
}

/// Removes a finished transfer from the list. Received files stay.
#[tauri::command]
#[specta::specta]
pub fn forget_transfer(
    state: State<'_, AppState>,
    device_id: String,
    transfer_id: String,
) -> Result<(), CommandError> {
    state
        .backend()
        .forget_transfer(&device_id, &transfer_id)
        .map_err(Into::into)
}

/// Opens a file that arrived from a phone with the desktop's default
/// application.
#[tauri::command]
#[specta::specta]
pub fn open_received_file(
    state: State<'_, AppState>,
    device_id: String,
    transfer_id: String,
    file_index: u32,
) -> Result<(), CommandError> {
    state
        .backend()
        .open_received_file(&device_id, &transfer_id, file_index as usize)
        .map_err(Into::into)
}

/// Shows a file that arrived from a phone in its folder.
#[tauri::command]
#[specta::specta]
pub fn show_received_file(
    state: State<'_, AppState>,
    device_id: String,
    transfer_id: String,
    file_index: u32,
) -> Result<(), CommandError> {
    state
        .backend()
        .show_received_file(&device_id, &transfer_id, file_index as usize)
        .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dropped_paths_must_all_be_absolute() {
        assert_eq!(
            require_absolute(&["/home/ana/a.txt".into(), "/tmp/b".into()]),
            Ok(())
        );
        for relative in ["https://host/x", "a.txt", "../secret", "-rf"] {
            assert_eq!(
                require_absolute(&["/home/ana/a.txt".into(), relative.into()]),
                Err(BackendError::FileUnreadable),
                "{relative}"
            );
        }
    }
}
