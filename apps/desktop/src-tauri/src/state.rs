//! State that Tauri manages for the lifetime of the app.
//!
//! `AppState` holds the [`Backend`]; command handlers only call it through
//! the trait. To use the real pairing backend, build it in `lib.rs` where the
//! fake one is created today. It also keeps the files of the latest drop on
//! the window until the page says which phone they go to.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use crate::backend::Backend;

/// Shared app state, registered with `tauri::Builder::manage`.
pub struct AppState {
    backend: Arc<dyn Backend>,
    /// The latest drop: its number and the dropped paths.
    drop: Mutex<(u32, Option<Vec<PathBuf>>)>,
}

impl AppState {
    pub fn new(backend: Arc<dyn Backend>) -> Self {
        Self {
            backend,
            drop: Mutex::new((0, None)),
        }
    }

    pub fn backend(&self) -> &dyn Backend {
        self.backend.as_ref()
    }

    /// The backend, for work that outlives a command's borrow of the state.
    pub fn shared_backend(&self) -> Arc<dyn Backend> {
        self.backend.clone()
    }

    /// Keeps the paths of a drop, replacing an earlier one, and returns its
    /// number.
    pub fn keep_drop(&self, paths: Vec<PathBuf>) -> u32 {
        let mut drop = self.drop.lock().unwrap_or_else(PoisonError::into_inner);
        drop.0 = drop.0.wrapping_add(1);
        drop.1 = Some(paths);
        drop.0
    }

    /// The paths of drop `id`, once, if it is the latest.
    pub fn take_drop(&self, id: u32) -> Option<Vec<PathBuf>> {
        let mut drop = self.drop.lock().unwrap_or_else(PoisonError::into_inner);
        if drop.0 == id { drop.1.take() } else { None }
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

    #[test]
    fn keeps_only_the_latest_drop_and_hands_it_out_once() {
        let state = AppState::new(Arc::new(FakeBackend::new(Script::Demo, Arc::new(Silent))));
        let first = state.keep_drop(vec![PathBuf::from("/a")]);
        let second = state.keep_drop(vec![PathBuf::from("/b")]);
        assert_eq!(state.take_drop(first), None);
        assert_eq!(state.take_drop(second), Some(vec![PathBuf::from("/b")]));
        assert_eq!(state.take_drop(second), None);
    }
}
