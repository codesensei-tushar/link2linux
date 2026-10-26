//! Tauri shell for the link2linux desktop app.
//!
//! This crate stays thin: it registers IPC commands, owns the managed state
//! and starts the window. Discovery, pairing, transport and feature logic
//! belong in the workspace crates, not here.

pub mod backend;
mod commands;
mod events;
mod state;

use std::sync::Arc;

use tauri::Manager;

use backend::Backend;
use tauri_specta::{Builder, collect_commands, collect_events};

pub use link2linux_runtime::qr;
pub use state::AppState;

/// The typed command set, shared by the runtime and the bindings export.
pub fn specta_builder() -> Builder<tauri::Wry> {
    Builder::<tauri::Wry>::new()
        .commands(collect_commands![
            commands::app::app_info,
            commands::devices::list_devices,
            commands::devices::unpair_device,
            commands::pairing::get_pairing_state,
            commands::pairing::start_pairing,
            commands::pairing::pair_with,
            commands::pairing::answer_code,
            commands::pairing::cancel_pairing,
        ])
        .events(collect_events![
            events::PairingChanged,
            events::DevicesChanged,
        ])
        // IPC types only travel from Rust to the UI for now, so one TypeScript
        // type per Rust type is enough (no separate input/output variants).
        .disable_serde_phases()
}

/// Starts the desktop app. Blocks until the last window closes.
pub fn run() {
    let builder = specta_builder();

    tauri::Builder::default()
        .invoke_handler(builder.invoke_handler())
        .setup(move |app| {
            builder.mount_events(app);
            let sink = events::TauriSink::new(app.handle().clone());
            // `L2L_FAKE_SCRIPT` selects the scripted fake for development and
            // tests; otherwise the real runtime pairs and connects phones.
            let backend: Arc<dyn Backend> = if std::env::var_os("L2L_FAKE_SCRIPT").is_some() {
                Arc::new(backend::fake::FakeBackend::from_env(sink))
            } else {
                Arc::new(backend::live::LiveBackend::start(sink)?)
            };
            app.manage(AppState::new(backend));
            Ok(())
        })
        .on_window_event(|window, event| {
            // The window's focus sets how often sessions ping.
            if let tauri::WindowEvent::Focused(focused) = event
                && let Some(state) = window.app_handle().try_state::<AppState>()
            {
                state.backend().set_active(*focused);
            }
        })
        .run(tauri::generate_context!())
        .expect("failed to start the link2linux desktop app");
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
