//! Tauri shell for the link2linux desktop app.
//!
//! This crate stays thin: it registers IPC commands, owns the managed state
//! and starts the window. Discovery, pairing, transport and feature logic
//! belong in the workspace crates, not here.

mod commands;
mod state;

use tauri_specta::{Builder, collect_commands};

pub use state::AppState;

/// The typed command set, shared by the runtime and the bindings export.
pub fn specta_builder() -> Builder<tauri::Wry> {
    Builder::<tauri::Wry>::new()
        .commands(collect_commands![
            commands::app::app_info,
            commands::devices::list_devices,
        ])
        // IPC types only travel from Rust to the UI for now, so one TypeScript
        // type per Rust type is enough (no separate input/output variants).
        .disable_serde_phases()
}

/// Starts the desktop app. Blocks until the last window closes.
pub fn run() {
    let builder = specta_builder();

    tauri::Builder::default()
        // TODO(core): build the state from a `crates/core` handle instead.
        .manage(AppState::default())
        .invoke_handler(builder.invoke_handler())
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
