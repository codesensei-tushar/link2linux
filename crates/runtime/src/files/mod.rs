//! File transfers with each phone.
//!
//! The core's [`FileService`](link2linux_core::files::FileService) does the
//! transfers; [`dto`] has what the files page receives.

pub mod dto;

use std::path::PathBuf;

use link2linux_core::files::{SendError, check_sendable};

use crate::view::BackendError;

/// Checks that the files at `paths` can go to a phone in one transfer,
/// telling the user why not. It looks at every file, so it blocks.
///
/// # Errors
///
/// Fails for a folder, a file that cannot be read, and for none or too
/// many files.
pub fn check_files(paths: &[PathBuf]) -> Result<(), BackendError> {
    check_sendable(paths).map_err(|error| send_error(&error))
}

/// What the user is told when files cannot be sent.
#[must_use]
pub fn send_error(error: &SendError) -> BackendError {
    match error {
        SendError::Directory(_) => BackendError::FolderNotSendable,
        SendError::TooMany => BackendError::TooManyFiles,
        SendError::NoFiles => BackendError::WrongState,
        _ => BackendError::FileUnreadable,
    }
}
