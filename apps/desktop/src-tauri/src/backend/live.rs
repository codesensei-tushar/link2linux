//! The backend that runs the real runtime: discovery, pairing and sessions
//! with paired phones, phone notifications as desktop banners, the shared
//! clipboard, file transfers, and the phones' status and ringing.
//!
//! Everything it does lives in `link2linux-runtime`; this file only builds
//! the runtime from what the shell has (the event sink, the D-Bus
//! notification service, the clipboard, the Downloads directory, Bluetooth
//! advertising) and forwards the trait's calls.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use link2linux_platform::linux::ble::BluezPresence;
use link2linux_platform::linux::{
    BluezBluetooth, DbusNotifications, LinuxClipboard, PipewireTelephony, SecretServiceStore,
    SessionLock, XdgFileStore,
};
use link2linux_platform::{Clipboard, ClipboardAccess, PresenceAdvertiser};
use link2linux_runtime::calls::CallStack;
use link2linux_runtime::clipboard::ClipboardPort;

use crate::clipboard::FallbackClipboard;
use link2linux_runtime::notify::{Presenter, PrivatePopups, ServiceFactory};
use link2linux_runtime::paths::{DataDir, desktop_name, runtime_images_dir};
use link2linux_runtime::{Runtime, RuntimeConfig, RuntimeError, Shell};

use super::{
    Backend, BackendError, CachedImage, CallsStatus, ClipboardStatus, Device, EventSink,
    FileTransfer, MirroredNotification, PairingMethod, PairingState, Pending, PhonePhotos,
    PhoneStatus, PhoneTarget, RingStatus, SavedPhoto, ScreenCodec, ScreenInput, ScreenStatus,
    ScreenViewer, Settings,
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
    /// `window_clipboard` is the clipboard as the window sees it, used when
    /// the desktop session lets no background app use the clipboard, or no
    /// longer does.
    ///
    /// # Errors
    ///
    /// Fails when the data directory, the identity or the list of paired
    /// phones cannot be used.
    pub fn start(
        events: Arc<dyn EventSink>,
        open_window: impl Fn() + Send + Sync + 'static,
        window_clipboard: impl Clipboard + 'static,
    ) -> Result<Self, RuntimeError> {
        let data_dir = DataDir::from_env()?;
        // Reaching the display server can take a few seconds: it happens
        // while the keyring is asked.
        let opening = std::thread::spawn(LinuxClipboard::open);
        let keyring = SecretServiceStore::new(APP_ID);
        // Waits while the keyring asks the user to unlock it, within the
        // store's timeouts.
        let identity = tauri::async_runtime::block_on(data_dir.load_identity(&keyring))?;
        let store_key = tauri::async_runtime::block_on(data_dir.load_store_key(&keyring));
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
            store_key,
            drivers: Arc::new(ServiceFactory::new(presenter.clone(), images)),
            presenter,
            clipboard: Some(clipboard(opening, window_clipboard)),
            files: Some(Arc::new(XdgFileStore::new())),
            calls: Some(Arc::new(CallStack {
                bluetooth: BluezBluetooth::new(),
                telephony: PipewireTelephony::new(),
            })),
            events,
        };
        let mut config = RuntimeConfig::new(data_dir.clone());
        config.clipboard = settings.clipboard();
        config.files = settings.share_files;
        config.show_phone_status = settings.show_phone_status;
        config.calls = settings.use_for_calls;
        let handle = tauri::async_runtime::handle().inner().clone();
        let runtime = Arc::new(Runtime::start(handle, &config, shell)?);
        if runtime.needs_store_key() {
            find_store_key(data_dir, keyring, Arc::downgrade(&runtime));
        }
        advertise(runtime.clone());
        Ok(Self {
            runtime,
            popups,
            settings: Mutex::new(settings),
            settings_path,
        })
    }
}

/// The desktop clipboard: followed in the background where the desktop
/// session allows it, else through the window while it has focus.
fn clipboard(
    opening: std::thread::JoinHandle<LinuxClipboard>,
    window_clipboard: impl Clipboard + 'static,
) -> Arc<dyn ClipboardPort> {
    let Ok(platform) = opening.join() else {
        tracing::warn!(
            "the clipboard could not be opened; it is shared while the window has focus"
        );
        return Arc::new(window_clipboard);
    };
    if Clipboard::access(&platform) == ClipboardAccess::FocusOnly {
        tracing::info!("the clipboard is shared while the window has focus");
    }
    Arc::new(FallbackClipboard::new(platform, window_clipboard))
}

/// Tells the pop-ups whether the screen is locked, as logind reports it.
/// While logind does not answer, the screen counts as locked, and the app
/// asks again, less often the longer it stays away.
fn follow_lock(popups: Arc<PrivatePopups>) {
    const FIRST_RETRY: Duration = Duration::from_secs(1);
    const LAST_RETRY: Duration = Duration::from_secs(60);
    tauri::async_runtime::spawn(async move {
        let mut retry = FIRST_RETRY;
        loop {
            match SessionLock::connect().await {
                Ok(mut lock) => match lock.is_locked().await {
                    Ok(locked) => {
                        retry = FIRST_RETRY;
                        popups.set_locked(locked).await;
                        while let Some(locked) = lock.changed().await {
                            popups.set_locked(locked).await;
                        }
                        tracing::info!("logind stopped reporting the lock screen state");
                    }
                    Err(error) => tracing::info!(%error, "the lock screen state is unavailable"),
                },
                Err(error) => tracing::info!(%error, "the lock screen state is unavailable"),
            }
            popups.set_locked(true).await;
            tokio::time::sleep(retry).await;
            retry = (retry * 2).min(LAST_RETRY);
        }
    });
}

/// Asks the keyring again, a few times and less often each time, for the
/// key of a sealed pairing file that it did not give at start, as on a slow
/// login.
fn find_store_key(data_dir: DataDir, keyring: SecretServiceStore, runtime: Weak<Runtime>) {
    const RETRIES: [Duration; 3] = [
        Duration::from_secs(30),
        Duration::from_secs(5 * 60),
        Duration::from_secs(30 * 60),
    ];
    tauri::async_runtime::spawn(async move {
        for delay in RETRIES {
            tokio::time::sleep(delay).await;
            let Some(key) = data_dir.load_store_key(&keyring).await else {
                continue;
            };
            if let Some(runtime) = runtime.upgrade() {
                runtime.use_store_key(key);
            }
            return;
        }
        tracing::warn!(
            "the keyring did not give the pairing file's key: new presence keys stay out of it"
        );
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
    fn shutdown(&self) {
        self.runtime
            .shutdown_and_wait(std::time::Duration::from_secs(2));
    }

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

    fn calls(&self) -> CallsStatus {
        self.runtime.calls()
    }

    fn start_bluetooth_pairing(&self, device_id: &str) -> Result<(), BackendError> {
        self.runtime.start_bluetooth_pairing(device_id)
    }

    fn cancel_bluetooth_pairing(&self) {
        self.runtime.cancel_bluetooth_pairing();
    }

    fn confirm_bluetooth_code(&self, prompt: u32, matches: bool) -> Result<(), BackendError> {
        self.runtime.confirm_bluetooth_code(prompt, matches)
    }

    fn choose_bluetooth_device(&self, device_id: &str, address: &str) -> Result<(), BackendError> {
        self.runtime.choose_bluetooth_device(device_id, address)
    }

    fn connect_calls(&self, device_id: &str) -> Result<(), BackendError> {
        self.runtime.connect_calls(device_id)
    }

    fn answer_call(&self, device_id: &str, call_id: &str) -> Result<(), BackendError> {
        self.runtime.answer_call(device_id, call_id)
    }

    fn decline_call(&self, device_id: &str, call_id: &str) -> Result<(), BackendError> {
        self.runtime.decline_call(device_id, call_id)
    }

    fn hang_up_call(&self, device_id: &str, call_id: &str) -> Result<(), BackendError> {
        self.runtime.hang_up_call(device_id, call_id)
    }

    fn dial(&self, device_id: &str, number: &str) -> Result<(), BackendError> {
        self.runtime.dial(device_id, number)
    }

    fn send_tones(&self, device_id: &str, tones: &str) -> Result<(), BackendError> {
        self.runtime.send_tones(device_id, tones)
    }

    fn set_call_muted(&self, device_id: &str, muted: bool) -> Result<(), BackendError> {
        self.runtime.set_call_muted(device_id, muted)
    }

    fn settings(&self) -> Settings {
        *self.settings.lock().expect("not poisoned")
    }

    fn clipboard(&self) -> ClipboardStatus {
        self.runtime.clipboard()
    }

    fn transfers(&self, device_id: &str) -> Result<Vec<FileTransfer>, BackendError> {
        self.runtime.transfers(device_id)
    }

    fn send_files(&self, device_id: &str, paths: Vec<PathBuf>) -> Result<(), BackendError> {
        self.runtime.send_files(device_id, paths)
    }

    fn cancel_transfer(&self, device_id: &str, transfer_id: &str) -> Result<(), BackendError> {
        self.runtime.cancel_transfer(device_id, transfer_id)
    }

    fn forget_transfer(&self, device_id: &str, transfer_id: &str) -> Result<(), BackendError> {
        self.runtime.forget_transfer(device_id, transfer_id)
    }

    fn open_received_file(
        &self,
        device_id: &str,
        transfer_id: &str,
        index: usize,
    ) -> Result<(), BackendError> {
        self.runtime
            .open_received_file(device_id, transfer_id, index)
    }

    fn show_received_file(
        &self,
        device_id: &str,
        transfer_id: &str,
        index: usize,
    ) -> Result<(), BackendError> {
        self.runtime
            .show_received_file(device_id, transfer_id, index)
    }

    fn send_clipboard(&self) -> Result<(), BackendError> {
        self.runtime.send_clipboard();
        Ok(())
    }

    fn phone_status(&self, device_id: &str) -> Result<PhoneStatus, BackendError> {
        self.runtime.phone_status(device_id)
    }

    fn ring_status(&self, device_id: &str) -> Result<RingStatus, BackendError> {
        self.runtime.ring_status(device_id)
    }

    fn ring_phone(&self, device_id: &str) -> Result<(), BackendError> {
        self.runtime.ring_phone(device_id)
    }

    fn stop_ringing(&self, device_id: &str) -> Result<(), BackendError> {
        self.runtime.stop_ringing(device_id)
    }

    fn screen_status(&self, device_id: &str) -> Result<ScreenStatus, BackendError> {
        self.runtime.screen_status(device_id)
    }

    fn start_screen(
        &self,
        device_id: &str,
        codecs: &[ScreenCodec],
        max_size: u32,
        viewer: Arc<dyn ScreenViewer>,
    ) -> Result<(), BackendError> {
        self.runtime
            .start_screen(device_id, codecs, max_size, viewer)
    }

    fn stop_screen(&self, device_id: &str) -> Result<(), BackendError> {
        self.runtime.stop_screen(device_id)
    }

    fn send_screen_input(&self, device_id: &str, input: ScreenInput) -> Result<(), BackendError> {
        self.runtime.send_screen_input(device_id, input)
    }

    fn request_screen_key_frame(&self, device_id: &str) -> Result<(), BackendError> {
        self.runtime.request_screen_key_frame(device_id)
    }

    fn photos(&self, device_id: &str) -> Result<PhonePhotos, BackendError> {
        self.runtime.photos(device_id)
    }

    fn photo_thumbnail(
        &self,
        device_id: &str,
        photo_id: &str,
        max_size: u32,
    ) -> Pending<Arc<[u8]>> {
        let thumbnail = self.runtime.photo_thumbnail(device_id, photo_id, max_size);
        Box::pin(async move { thumbnail.await.map(|thumbnail| thumbnail.data) })
    }

    fn save_photo(&self, device_id: &str, photo_id: &str) -> Pending<SavedPhoto> {
        Box::pin(self.runtime.save_photo(device_id, photo_id))
    }

    fn open_saved_photo(&self, device_id: &str, photo_id: &str) -> Result<(), BackendError> {
        self.runtime.open_saved_photo(device_id, photo_id)
    }

    fn show_saved_photo(&self, device_id: &str, photo_id: &str) -> Result<(), BackendError> {
        self.runtime.show_saved_photo(device_id, photo_id)
    }

    fn set_settings(&self, settings: Settings) -> Result<(), BackendError> {
        let before = std::mem::replace(&mut *self.settings.lock().expect("not poisoned"), settings);
        self.popups.set_show_text(settings.show_notification_text);
        if before.clipboard() != settings.clipboard() {
            self.runtime.set_clipboard_settings(settings.clipboard());
        }
        if before.share_files != settings.share_files {
            self.runtime.set_files_enabled(settings.share_files);
        }
        if before.show_phone_status != settings.show_phone_status {
            self.runtime
                .set_phone_status_shown(settings.show_phone_status);
        }
        if before.use_for_calls != settings.use_for_calls {
            self.runtime.set_calls_enabled(settings.use_for_calls);
        }
        settings.save(&self.settings_path).map_err(|error| {
            tracing::warn!(%error, "could not save the settings");
            BackendError::NotSaved
        })
    }
}
