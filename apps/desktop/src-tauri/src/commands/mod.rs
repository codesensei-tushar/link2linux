//! IPC commands the UI can call.
//!
//! Each handler stays small: it reads from `AppState` (or the app handle) and
//! returns a serializable type. Types derive `specta::Type`, so the
//! TypeScript bindings in `src/lib/bindings.ts` are generated from them.
//!
//! To add a command: write it in a module here, list it in
//! `crate::specta_builder`, add it to `build.rs` and grant it in
//! `capabilities/default.json`.

pub mod app;
pub mod devices;
