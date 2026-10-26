//! State that Tauri manages for the lifetime of the app.
//!
//! `AppState` holds the [`Backend`]; command handlers only call it through
//! the trait. To use the real pairing backend, build it in `lib.rs` where the
//! fake one is created today.

use std::sync::Arc;

use crate::backend::Backend;

/// Shared app state, registered with `tauri::Builder::manage`.
pub struct AppState {
    backend: Arc<dyn Backend>,
}

impl AppState {
    pub fn new(backend: Arc<dyn Backend>) -> Self {
        Self { backend }
    }

    pub fn backend(&self) -> &dyn Backend {
        self.backend.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::fake::{FakeBackend, Script};
    use crate::backend::{Device, EventSink, PairingState};

    struct Silent;
    impl EventSink for Silent {
        fn pairing_changed(&self, _: &PairingState) {}
        fn devices_changed(&self, _: &[Device]) {}
    }

    #[test]
    fn exposes_the_backend_it_was_given() {
        let state = AppState::new(Arc::new(FakeBackend::new(Script::Demo, Arc::new(Silent))));
        let names: Vec<_> = state
            .backend()
            .devices()
            .into_iter()
            .map(|d| d.name)
            .collect();
        assert_eq!(names, ["Pixel 9", "Old Galaxy"]);
        assert_eq!(state.backend().pairing(), PairingState::Idle);
    }
}
