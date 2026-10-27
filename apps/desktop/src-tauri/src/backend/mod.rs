//! The seam between the desktop shell and whatever does the pairing.
//!
//! The UI talks to the [`Backend`] trait only. The app runs [`live`], which
//! drives the real runtime, unless `L2L_FAKE_SCRIPT` names one of the
//! deterministic scripts of the [`fake`] backend for development and tests.
//! Commands, events and the generated TypeScript bindings all use the types
//! defined in `link2linux-runtime`, re-exported here.
//!
//! Commands never return state. They start or answer something, and the
//! resulting state arrives through [`EventSink`], so a slow reply can never
//! overwrite a newer event.

pub mod failed;
pub mod fake;
pub mod live;

pub use link2linux_runtime::notify::CachedImage;
pub use link2linux_runtime::settings::Settings;
pub use link2linux_runtime::view::{
    BackendError, CallsDesktop, CallsStatus, ClipboardMode, ClipboardStatus, Device, DeviceStatus,
    DiscoveredPhone, EventSink, FileTransfer, FileTransferEvent, MirroredNotification,
    NotificationEvent, PairingFailure, PairingMethod, PairingState, PhoneStatus, PhoneTarget,
    QrImage, RingStatus, StartupError,
};

use std::path::PathBuf;

/// What the desktop shell needs from the pairing and device layer.
pub trait Backend: Send + Sync {
    /// Paired phones, in pairing order.
    fn devices(&self) -> Vec<Device>;
    /// The current pairing step.
    fn pairing(&self) -> PairingState;
    /// Begins a pairing activation. Refused unless pairing is idle or
    /// finished.
    fn start_pairing(&self, method: PairingMethod) -> Result<(), BackendError>;
    /// Dials a phone: from the search list, or a typed address (also allowed
    /// while a QR code is showing).
    fn pair_with(&self, target: PhoneTarget) -> Result<(), BackendError>;
    /// The user's answer to "do the codes match?".
    fn answer_code(&self, matches: bool) -> Result<(), BackendError>;
    /// Stops pairing, or dismisses a finished one. Always ends in `Idle`.
    fn cancel_pairing(&self);
    /// Forgets a phone.
    fn unpair(&self, device_id: &str) -> Result<(), BackendError>;
    /// The window gained or lost focus, which sets how often sessions ping.
    fn set_active(&self, _active: bool) {}

    /// The notifications mirrored from a phone, in no particular order.
    fn notifications(&self, _device_id: &str) -> Result<Vec<MirroredNotification>, BackendError> {
        Ok(Vec::new())
    }
    /// Dismisses a notification on the phone.
    fn dismiss_notification(&self, _device_id: &str, _key: &str) -> Result<(), BackendError> {
        Ok(())
    }
    /// Runs a notification's action on the phone.
    fn run_notification_action(
        &self,
        _device_id: &str,
        _key: &str,
        _action_id: &str,
        _reply: Option<&str>,
    ) -> Result<(), BackendError> {
        Ok(())
    }
    /// A picture the phone shared, if it has arrived.
    fn notification_image(&self, _device_id: &str, _sha256: &str) -> Option<CachedImage> {
        None
    }

    /// The state of the shared clipboard.
    fn clipboard(&self) -> ClipboardStatus {
        ClipboardStatus {
            mode: ClipboardMode::Unavailable,
            phones: Vec::new(),
            activity: Vec::new(),
            problem: None,
            held: None,
        }
    }
    /// Sends what the clipboard holds to every phone. What came of it
    /// arrives as a new clipboard status.
    fn send_clipboard(&self) -> Result<(), BackendError> {
        Ok(())
    }
    /// The file transfers with a phone, oldest first.
    fn transfers(&self, _device_id: &str) -> Result<Vec<FileTransfer>, BackendError> {
        Ok(Vec::new())
    }
    /// Sends the files at `paths`, which the user chose, to a phone as one
    /// transfer. It looks at every file first, so it blocks.
    fn send_files(&self, _device_id: &str, _paths: Vec<PathBuf>) -> Result<(), BackendError> {
        Err(BackendError::NotStarted)
    }
    /// Cancels a transfer that has not finished, in either direction.
    fn cancel_transfer(&self, _device_id: &str, _transfer_id: &str) -> Result<(), BackendError> {
        Err(BackendError::NotFound)
    }
    /// Removes a finished transfer from the list.
    fn forget_transfer(&self, _device_id: &str, _transfer_id: &str) -> Result<(), BackendError> {
        Err(BackendError::NotFound)
    }
    /// Opens file `index` of a transfer from a phone, once it has arrived,
    /// with the desktop's default application.
    fn open_received_file(
        &self,
        _device_id: &str,
        _transfer_id: &str,
        _index: usize,
    ) -> Result<(), BackendError> {
        Err(BackendError::NotFound)
    }
    /// Shows file `index` of a transfer from a phone in its folder.
    fn show_received_file(
        &self,
        _device_id: &str,
        _transfer_id: &str,
        _index: usize,
    ) -> Result<(), BackendError> {
        Err(BackendError::NotFound)
    }

    /// A phone's battery, signal and sound mode, as the UI shows them.
    fn phone_status(&self, _device_id: &str) -> Result<PhoneStatus, BackendError> {
        Ok(PhoneStatus::Disconnected)
    }
    /// Whether a phone rings, and why it cannot or did not.
    fn ring_status(&self, _device_id: &str) -> Result<RingStatus, BackendError> {
        Ok(RingStatus::default())
    }
    /// Makes a phone ring for a minute, even when it is set to silent. What
    /// came of it arrives as a new ring status.
    fn ring_phone(&self, _device_id: &str) -> Result<(), BackendError> {
        Err(BackendError::NotStarted)
    }
    /// Stops a phone ringing.
    fn stop_ringing(&self, _device_id: &str) -> Result<(), BackendError> {
        Err(BackendError::NotStarted)
    }
    /// The phones' calls, and this computer's part in them.
    fn calls(&self) -> CallsStatus {
        CallsStatus {
            desktop: CallsDesktop::Unsupported,
            enabled: false,
            pairing: None,
            phones: Vec::new(),
        }
    }
    /// Opens Bluetooth pairing with a phone for 2 minutes.
    fn start_bluetooth_pairing(&self, _device_id: &str) -> Result<(), BackendError> {
        Err(BackendError::CallsUnavailable)
    }
    /// Closes Bluetooth pairing.
    fn cancel_bluetooth_pairing(&self) {}
    /// Answers a pairing prompt: whether both devices show the same code.
    fn confirm_bluetooth_code(&self, _prompt: u32, _matches: bool) -> Result<(), BackendError> {
        Err(BackendError::WrongState)
    }
    /// Picks the paired Bluetooth device at `address` as a phone's.
    fn choose_bluetooth_device(
        &self,
        _device_id: &str,
        _address: &str,
    ) -> Result<(), BackendError> {
        Err(BackendError::NotFound)
    }
    /// Connects a phone's hands-free link with this computer.
    fn connect_calls(&self, _device_id: &str) -> Result<(), BackendError> {
        Err(BackendError::CallsUnavailable)
    }
    /// Answers a ringing call.
    fn answer_call(&self, _device_id: &str, _call_id: &str) -> Result<(), BackendError> {
        Err(BackendError::NotFound)
    }
    /// Declines a ringing call.
    fn decline_call(&self, _device_id: &str, _call_id: &str) -> Result<(), BackendError> {
        Err(BackendError::NotFound)
    }
    /// Ends a call.
    fn hang_up_call(&self, _device_id: &str, _call_id: &str) -> Result<(), BackendError> {
        Err(BackendError::NotFound)
    }
    /// Calls `number` from a phone.
    fn dial(&self, _device_id: &str, _number: &str) -> Result<(), BackendError> {
        Err(BackendError::CallsUnavailable)
    }
    /// Sends touch tones in a phone's active call.
    fn send_tones(&self, _device_id: &str, _tones: &str) -> Result<(), BackendError> {
        Err(BackendError::CallsUnavailable)
    }
    /// Mutes or unmutes this computer's microphone in a phone's calls.
    fn set_call_muted(&self, _device_id: &str, _muted: bool) -> Result<(), BackendError> {
        Err(BackendError::CallsUnavailable)
    }

    /// The desktop app's settings.
    fn settings(&self) -> Settings {
        Settings::default()
    }
    /// Changes the settings; they apply at once.
    fn set_settings(&self, _settings: Settings) -> Result<(), BackendError> {
        Ok(())
    }

    /// The app is closing: ends the sessions and cleans up what is
    /// unfinished, without waiting long.
    fn shutdown(&self) {}

    /// Why the backend did not start, if it did not.
    fn startup_error(&self) -> Option<StartupError> {
        None
    }
}
