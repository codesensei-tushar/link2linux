//! The backend of an app whose runtime did not start.
//!
//! Quitting would leave the user with nothing on screen. Instead the window
//! opens on a page that says what failed and offers to try again, and every
//! command is refused with [`BackendError::NotStarted`].

use std::sync::Arc;

use link2linux_runtime::RuntimeError;

use super::{
    Backend, BackendError, Device, MirroredNotification, PairingMethod, PairingState, PhoneTarget,
    Settings, StartupError,
};

/// The started backend, or a [`FailedBackend`] that explains why the start
/// failed.
pub fn or_failed<B: Backend + 'static>(started: Result<B, RuntimeError>) -> Arc<dyn Backend> {
    match started {
        Ok(backend) => Arc::new(backend),
        Err(error) => {
            tracing::error!(%error, "the desktop app could not start");
            Arc::new(FailedBackend {
                error: StartupError::from(&error),
            })
        }
    }
}

/// [`Backend`] for a runtime that did not start.
pub struct FailedBackend {
    error: StartupError,
}

impl Backend for FailedBackend {
    fn devices(&self) -> Vec<Device> {
        Vec::new()
    }

    fn pairing(&self) -> PairingState {
        PairingState::Idle
    }

    fn start_pairing(&self, _method: PairingMethod) -> Result<(), BackendError> {
        Err(BackendError::NotStarted)
    }

    fn pair_with(&self, _target: PhoneTarget) -> Result<(), BackendError> {
        Err(BackendError::NotStarted)
    }

    fn answer_code(&self, _matches: bool) -> Result<(), BackendError> {
        Err(BackendError::NotStarted)
    }

    fn cancel_pairing(&self) {}

    fn unpair(&self, _device_id: &str) -> Result<(), BackendError> {
        Err(BackendError::NotStarted)
    }

    fn notifications(&self, _device_id: &str) -> Result<Vec<MirroredNotification>, BackendError> {
        Err(BackendError::NotStarted)
    }

    fn dismiss_notification(&self, _device_id: &str, _key: &str) -> Result<(), BackendError> {
        Err(BackendError::NotStarted)
    }

    fn run_notification_action(
        &self,
        _device_id: &str,
        _key: &str,
        _action_id: &str,
        _reply: Option<&str>,
    ) -> Result<(), BackendError> {
        Err(BackendError::NotStarted)
    }

    fn send_clipboard(&self) -> Result<(), BackendError> {
        Err(BackendError::NotStarted)
    }

    fn set_settings(&self, _settings: Settings) -> Result<(), BackendError> {
        Err(BackendError::NotStarted)
    }

    fn startup_error(&self) -> Option<StartupError> {
        Some(self.error.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::EventSink;
    use crate::backend::fake::{FakeBackend, Script};

    struct Silent;
    impl EventSink for Silent {
        fn pairing_changed(&self, _: &PairingState) {}
        fn devices_changed(&self, _: &[Device]) {}
    }

    #[test]
    fn a_failed_start_keeps_the_app_up_with_the_reason() {
        let backend =
            or_failed::<FakeBackend>(Err(RuntimeError::Data(std::io::Error::other("disk full"))));

        let error = backend.startup_error().unwrap();
        assert_eq!(error.message, "link2linux couldn't use its data folder.");
        assert!(error.detail.contains("disk full"));
        assert!(backend.devices().is_empty());
        assert_eq!(backend.pairing(), PairingState::Idle);
        assert_eq!(
            backend.start_pairing(PairingMethod::QrCode),
            Err(BackendError::NotStarted)
        );
        assert!(matches!(
            backend.notifications("phone-1"),
            Err(BackendError::NotStarted)
        ));
        assert_eq!(backend.send_clipboard(), Err(BackendError::NotStarted));
        assert!(backend.clipboard().phones.is_empty());
    }

    #[test]
    fn a_started_backend_reports_no_startup_error() {
        let backend = or_failed(Ok(FakeBackend::new(Script::Demo, Arc::new(Silent))));
        assert_eq!(backend.startup_error(), None);
    }
}
