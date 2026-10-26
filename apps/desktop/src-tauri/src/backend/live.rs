//! The backend that runs the real runtime: discovery, pairing and sessions
//! with paired phones, and phone notifications as desktop banners.
//!
//! Everything it does lives in `link2linux-runtime`; this file only builds
//! the runtime from what the shell has (the event sink, the D-Bus
//! notification service, Bluetooth advertising) and forwards the trait's
//! calls.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use link2linux_platform::PresenceAdvertiser;
use link2linux_platform::linux::ble::BluezPresence;
use link2linux_platform::linux::{DbusNotifications, SecretServiceStore, SessionLock};
use link2linux_runtime::notify::{Presenter, PrivatePopups, ServiceFactory};
use link2linux_runtime::paths::{DataDir, desktop_name, runtime_images_dir};
use link2linux_runtime::{Runtime, RuntimeConfig, RuntimeError, Shell};

use super::{
    Backend, BackendError, CachedImage, Device, EventSink, MirroredNotification, PairingMethod,
    PairingState, PhoneTarget, Settings,
};

/// The app id, which also names this app's items in the keyring.
const APP_ID: &str = "dev.link2linux.desktop";

/// [`Backend`] over the real runtime.
pub struct LiveBackend {
    runtime: Arc<Runtime>,
    popups: Arc<PrivatePopups>,
    settings: Mutex<Settings>,
    settings_path: PathBuf,
}

impl LiveBackend {
    /// Starts the runtime on Tauri's async runtime, with data in the user's
    /// XDG data directory and the private key in the keyring.
    /// `open_window` brings up the window when the user clicks a pop-up.
    ///
    /// # Errors
    ///
    /// Fails when the data directory, the identity or the list of paired
    /// phones cannot be used.
    pub fn start(
        events: Arc<dyn EventSink>,
        open_window: impl Fn() + Send + Sync + 'static,
    ) -> Result<Self, RuntimeError> {
        let data_dir = DataDir::from_env()?;
        // Waits while the keyring asks the user to unlock it, within the
        // store's timeouts.
        let identity = tauri::async_runtime::block_on(
            data_dir.load_identity(&SecretServiceStore::new(APP_ID)),
        )?;
        // Pictures stay off the disk: in the runtime directory, or else
        // only in memory.
        if let Err(error) = data_dir.remove_old_images() {
            tracing::warn!(%error, "could not delete the pictures kept on disk");
        }
        let images = runtime_images_dir();
        let settings_path = data_dir.settings();
        let settings = Settings::load(&settings_path);
        let popups = Arc::new(PrivatePopups::new(
            Arc::new(DbusNotifications::new()),
            settings.show_notification_text,
            open_window,
        ));
        follow_lock(popups.clone());
        let presenter: Arc<dyn Presenter> = popups.clone();
        let shell = Shell {
            identity,
            drivers: Arc::new(ServiceFactory::new(presenter.clone(), images)),
            presenter,
            events,
        };
        let config = RuntimeConfig::new(data_dir);
        let handle = tauri::async_runtime::handle().inner().clone();
        let runtime = Arc::new(Runtime::start(handle, &config, shell)?);
        advertise(runtime.clone());
        Ok(Self {
            runtime,
            popups,
            settings: Mutex::new(settings),
            settings_path,
        })
    }
}

/// Tells the pop-ups whether the screen is locked, as logind reports it.
/// Without logind the screen counts as unlocked.
fn follow_lock(popups: Arc<PrivatePopups>) {
    tauri::async_runtime::spawn(async move {
        let mut lock = match SessionLock::connect().await {
            Ok(lock) => lock,
            Err(error) => {
                tracing::info!(%error, "the lock screen state is unavailable");
                return;
            }
        };
        match lock.is_locked().await {
            Ok(locked) => popups.set_locked(locked),
            Err(error) => tracing::info!(%error, "the lock screen state is unavailable"),
        }
        while let Some(locked) = lock.changed().await {
            popups.set_locked(locked);
        }
    });
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

    fn notifications(&self, device_id: &str) -> Result<Vec<MirroredNotification>, BackendError> {
        self.runtime.notifications(device_id)
    }

    fn dismiss_notification(&self, device_id: &str, key: &str) -> Result<(), BackendError> {
        self.runtime.dismiss_notification(device_id, key)
    }

    fn run_notification_action(
        &self,
        device_id: &str,
        key: &str,
        action_id: &str,
        reply: Option<&str>,
    ) -> Result<(), BackendError> {
        self.runtime
            .run_notification_action(device_id, key, action_id, reply)
    }

    fn notification_image(&self, device_id: &str, sha256: &str) -> Option<CachedImage> {
        self.runtime.notification_image(device_id, sha256)
    }

    fn settings(&self) -> Settings {
        *self.settings.lock().expect("not poisoned")
    }

    fn set_settings(&self, settings: Settings) -> Result<(), BackendError> {
        *self.settings.lock().expect("not poisoned") = settings;
        self.popups.set_show_text(settings.show_notification_text);
        settings.save(&self.settings_path).map_err(|error| {
            tracing::warn!(%error, "could not save the settings");
            BackendError::NotSaved
        })
    }
}
