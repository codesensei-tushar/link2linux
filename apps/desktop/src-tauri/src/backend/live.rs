//! The backend that runs the real runtime: discovery, pairing and sessions
//! with paired phones, and phone notifications as desktop banners.
//!
//! Everything it does lives in `link2linux-runtime`; this file only builds
//! the runtime from what the shell has (the event sink, the D-Bus
//! notification service, Bluetooth advertising) and forwards the trait's
//! calls.

use std::sync::Arc;

use link2linux_platform::PresenceAdvertiser;
use link2linux_platform::linux::DbusNotifications;
use link2linux_platform::linux::ble::BluezPresence;
use link2linux_runtime::notify::{Presenter, ServiceFactory};
use link2linux_runtime::paths::{DataDir, desktop_name};
use link2linux_runtime::{Runtime, RuntimeConfig, RuntimeError, Shell};

use super::{Backend, BackendError, Device, EventSink, PairingMethod, PairingState, PhoneTarget};

/// [`Backend`] over the real runtime.
pub struct LiveBackend {
    runtime: Arc<Runtime>,
}

impl LiveBackend {
    /// Starts the runtime on Tauri's async runtime, with data in the user's
    /// XDG data directory.
    ///
    /// # Errors
    ///
    /// Fails when the data directory, the identity or the list of paired
    /// phones cannot be used.
    pub fn start(events: Arc<dyn EventSink>) -> Result<Self, RuntimeError> {
        let data_dir = DataDir::from_env()?;
        let presenter: Arc<dyn Presenter> = Arc::new(DbusNotifications::new());
        let shell = Shell {
            drivers: Arc::new(ServiceFactory::new(presenter.clone(), data_dir.images())),
            presenter,
            events,
        };
        let config = RuntimeConfig::new(data_dir);
        let handle = tauri::async_runtime::handle().inner().clone();
        let runtime = Arc::new(Runtime::start(handle, &config, shell)?);
        advertise(runtime.clone());
        Ok(Self { runtime })
    }
}

/// Advertises over Bluetooth LE when BlueZ allows it, and tells the runtime
/// the address to put in QR codes. Without Bluetooth, everything else works.
fn advertise(runtime: Arc<Runtime>) {
    tauri::async_runtime::spawn(async move {
        let advertiser = BluezPresence::new();
        match advertiser.start(&desktop_name()).await {
            Ok(adapter) => {
                runtime.set_ble_address(Some(adapter.address.0));
                // The advertisement ends when the advertiser is dropped.
                std::future::pending::<()>().await;
            }
            Err(error) => tracing::info!(%error, "Bluetooth advertising is unavailable"),
        }
    });
}

impl Backend for LiveBackend {
    fn devices(&self) -> Vec<Device> {
        self.runtime.devices()
    }

    fn pairing(&self) -> PairingState {
        self.runtime.pairing()
    }

    fn start_pairing(&self, method: PairingMethod) -> Result<(), BackendError> {
        self.runtime.start_pairing(method)
    }

    fn pair_with(&self, target: PhoneTarget) -> Result<(), BackendError> {
        self.runtime.pair_with(target)
    }

    fn answer_code(&self, matches: bool) -> Result<(), BackendError> {
        self.runtime.answer_code(matches)
    }

    fn cancel_pairing(&self) {
        self.runtime.cancel_pairing();
    }

    fn unpair(&self, device_id: &str) -> Result<(), BackendError> {
        self.runtime.unpair(device_id)
    }

    fn set_active(&self, active: bool) {
        self.runtime.set_active(active);
    }
}
