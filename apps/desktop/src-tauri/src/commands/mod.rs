//! IPC commands the UI can call.
//!
//! Each handler stays small: it forwards to the [`crate::backend::Backend`]
//! in `AppState` and returns a serializable type. Commands that start or
//! answer something return nothing; the new state arrives as an event (see
//! [`crate::events`]). Types derive `specta::Type`, so the TypeScript
//! bindings in `src/lib/bindings.ts` are generated from them.
//!
//! To add a command: write it in a module here, list it in
//! `crate::specta_builder`, add it to `build.rs` and grant it in
//! `capabilities/default.json`.

pub mod app;
pub mod devices;
pub mod notifications;
pub mod pairing;
pub mod settings;

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::backend::BackendError;

/// Why a command was refused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CommandError {
    pub code: CommandErrorCode,
    /// Plain-language text the UI can show as is.
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum CommandErrorCode {
    WrongState,
    InvalidAddress,
    NotFound,
    NotSaved,
}

impl From<BackendError> for CommandError {
    fn from(error: BackendError) -> Self {
        let code = match error {
            BackendError::WrongState => CommandErrorCode::WrongState,
            BackendError::InvalidAddress => CommandErrorCode::InvalidAddress,
            BackendError::NotFound => CommandErrorCode::NotFound,
            BackendError::NotSaved => CommandErrorCode::NotSaved,
        };
        Self {
            code,
            message: error.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_keep_their_wording_and_code() {
        let error = CommandError::from(BackendError::InvalidAddress);
        assert_eq!(error.code, CommandErrorCode::InvalidAddress);
        assert!(error.message.contains("192.168.1.20"));
    }
}
