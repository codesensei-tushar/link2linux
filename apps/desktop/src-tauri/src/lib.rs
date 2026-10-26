//! Tauri shell for the link2linux desktop app.
//!
//! This crate stays thin: it registers IPC commands, owns the managed state
//! and starts the window. Discovery, pairing, transport and feature logic
//! belong in the workspace crates, not here.

pub mod backend;
mod clipboard;
mod commands;
mod events;
mod state;

use std::sync::Arc;

use tauri::{DragDropEvent, Manager, WindowEvent};
use tauri_specta::Event as _;

use backend::Backend;
use tauri_specta::{Builder, collect_commands, collect_events};

pub use link2linux_runtime::qr;
pub use state::AppState;

/// The typed command set, shared by the runtime and the bindings export.
pub fn specta_builder() -> Builder<tauri::Wry> {
    Builder::<tauri::Wry>::new()
        .commands(collect_commands![
            commands::app::app_info,
            commands::app::startup_error,
            commands::app::restart_app,
            commands::devices::list_devices,
            commands::devices::unpair_device,
            commands::pairing::get_pairing_state,
            commands::pairing::start_pairing,
            commands::pairing::pair_with,
            commands::pairing::answer_code,
            commands::pairing::cancel_pairing,
            commands::notifications::list_notifications,
            commands::notifications::dismiss_notification,
            commands::notifications::run_notification_action,
            commands::notifications::notification_image,
            commands::clipboard::get_clipboard_status,
            commands::clipboard::send_clipboard,
            commands::files::list_transfers,
            commands::files::pick_files_to_send,
            commands::files::send_dropped_files,
            commands::files::cancel_transfer,
            commands::files::forget_transfer,
            commands::files::open_received_file,
            commands::files::show_received_file,
            commands::settings::get_settings,
            commands::settings::set_settings,
        ])
        .events(collect_events![
            events::PairingChanged,
            events::DevicesChanged,
            events::NotificationChanged,
            events::ClipboardChanged,
            events::TransferChanged,
            events::FileDrag,
            events::FilesDropped,
        ])
        // IPC types only travel from Rust to the UI for now, so one TypeScript
        // type per Rust type is enough (no separate input/output variants).
        .disable_serde_phases()
}

/// Starts the desktop app. Blocks until the last window closes.
pub fn run() {
    // This process holds the phone's notifications and the private key:
    // keep other programs of the user from reading its memory.
    if let Err(error) = link2linux_platform::linux::deny_memory_access() {
        tracing::warn!(%error, "could not protect the app's memory");
    }
    let builder = specta_builder();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(builder.invoke_handler())
        .setup(move |app| {
            builder.mount_events(app);
            let sink = events::TauriSink::new(app.handle().clone());
            // `L2L_FAKE_SCRIPT` selects the scripted fake for development and
            // tests; otherwise the real runtime pairs and connects phones.
            let backend: Arc<dyn Backend> = if std::env::var_os("L2L_FAKE_SCRIPT").is_some() {
                Arc::new(backend::fake::FakeBackend::from_env(sink))
            } else {
                let handle = app.handle().clone();
                let window_clipboard = clipboard::WindowClipboard::new(handle.clone());
                // A failed start keeps the window up to say why, rather than
                // quitting without a word.
                backend::failed::or_failed(backend::live::LiveBackend::start(
                    sink,
                    move || show_window(&handle),
                    window_clipboard,
                ))
            };
            app.manage(AppState::new(backend));
            Ok(())
        })
        .on_window_event(|window, event| {
            let app = window.app_handle();
            match event {
                // The window's focus sets how often sessions ping, and when a
                // clipboard that works only then is used.
                WindowEvent::Focused(focused) => {
                    if let Some(state) = app.try_state::<AppState>() {
                        state.backend().set_active(*focused);
                    }
                }
                WindowEvent::DragDrop(drag) => file_drag(app, drag),
                _ => {}
            }
        })
        .run(tauri::generate_context!())
        .expect("failed to start the link2linux desktop app");
}

/// Tells the page about files dragged over the window, and keeps the paths
/// of a drop for `send_dropped_files`: the shell reads them from the
/// desktop, so the page never names files itself.
fn file_drag(app: &tauri::AppHandle, drag: &DragDropEvent) {
    match drag {
        DragDropEvent::Enter { .. } => {
            let _ = events::FileDrag { over: true }.emit(app);
        }
        DragDropEvent::Drop { paths, .. } => {
            let _ = events::FileDrag { over: false }.emit(app);
            let Some(state) = app.try_state::<AppState>() else {
                return;
            };
            if paths.is_empty() {
                return;
            }
            let names = paths
                .iter()
                .map(|path| {
                    path.file_name().map_or_else(
                        || path.display().to_string(),
                        |name| name.to_string_lossy().into_owned(),
                    )
                })
                .collect();
            let drop_id = state.keep_drop(paths.clone());
            let _ = events::FilesDropped { drop_id, names }.emit(app);
        }
        DragDropEvent::Leave => {
            let _ = events::FileDrag { over: false }.emit(app);
        }
        _ => {}
    }
}

/// Brings the main window to the front, for a click on a pop-up.
fn show_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

#[cfg(test)]
mod tests {
    /// Writes `src/lib/bindings.ts`. Run it with `pnpm bindings` after
    /// changing a command or one of its types, and commit the result.
    #[test]
    fn export_bindings() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../src/lib/bindings.ts");
        super::specta_builder()
            .export(specta_typescript::Typescript::default(), path)
            .expect("failed to export TypeScript bindings");
    }
}
