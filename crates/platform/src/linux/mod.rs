//! Linux implementations of the platform traits.
//!
//! Mostly stubs for now: each type names the Linux service it will use, and
//! until then its methods fail with [`PlatformError::Unsupported`], so the
//! app can start and show what is missing. [`XdgFileStore`] already
//! works.

pub mod ble;
pub mod clipboard;
mod files;
pub mod notifications;
pub mod process;
pub mod secrets;
pub mod session_lock;

pub use clipboard::LinuxClipboard;
pub use files::XdgFileStore;
pub use notifications::DbusNotifications;
pub use process::deny_memory_access;
pub use secrets::SecretServiceStore;
pub use session_lock::SessionLock;

use crate::{CallAudio, PlatformError};

/// Call audio through BlueZ (hands-free profile) and PipeWire.
#[derive(Debug, Default)]
pub struct BluezCallAudio;

impl CallAudio for BluezCallAudio {
    async fn connect(&self, _phone: &str) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported("call audio"))
    }

    async fn disconnect(&self) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported("call audio"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stubs_report_unsupported() {
        assert!(matches!(
            BluezCallAudio.connect("00:11:22:33:44:55").await,
            Err(PlatformError::Unsupported(_))
        ));
    }
}
