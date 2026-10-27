//! The runtime the shell holds.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use link2linux_core::calls::CallAction;
use link2linux_core::clipboard::ClipboardSettings;
use link2linux_core::files::TransferId;
use link2linux_core::notifications::CachedImage;
use link2linux_core::pairing::{StoreKey, TrustStore, records};
use link2linux_core::photos::Thumbnail;
use link2linux_core::session::Availability;
use tokio::runtime::Handle;
use tokio::sync::oneshot;

use crate::calls;
use crate::clipboard;
use crate::discovery::{pump_responses, run_discovery, watch_network};
use crate::files;
use crate::link::{Command, lock};
use crate::pairing;
use crate::phone::PhoneCommand;
use crate::photos::{self, PhotoCommand};
use crate::screen::{ScreenCodec, ScreenCommand, ScreenInput, ScreenStatus, ScreenViewer};
use crate::shared::{Core, RuntimeConfig, RuntimeError, Shell};
use crate::view::{
    BackendError, CallsStatus, ClipboardStatus, Device, FileTransfer, MirroredNotification,
    PairingMethod, PairingState, PhonePhotos, PhoneStatus, PhoneTarget, RingStatus, SavedPhoto,
};

/// Discovery, pairing and a session with each paired phone.
///
/// Every method returns at once. Changes are reported through the event
/// sink the runtime was started with, which also receives the state a
/// command leads to. Dropping the runtime stops it.
pub struct Runtime {
    core: Arc<Core>,
}

impl Runtime {
    /// Loads the identity and the paired phones and starts working on
    /// `handle`: discovery runs, and every paired phone is dialed.
    ///
    /// # Errors
    ///
    /// Fails when the data directory, the identity or the list of paired
    /// phones cannot be used.
    pub fn start(
        handle: Handle,
        config: &RuntimeConfig,
        shell: Shell,
    ) -> Result<Self, RuntimeError> {
        let core = Core::open(handle.clone(), config, shell)?;
        if let Some(lan) = config.discovery {
            handle.spawn(run_discovery(core.clone(), lan));
        }
        handle.spawn(watch_network(core.clone()));
        handle.spawn(pump_responses(core.clone()));
        handle.spawn(clipboard::watch(core.clone()));
        Ok(Self { core })
    }

    /// The paired phones, in pairing order.
    #[must_use]
    pub fn devices(&self) -> Vec<Device> {
        self.core.devices()
    }

    /// The current pairing step.
    #[must_use]
    pub fn pairing(&self) -> PairingState {
        pairing::state(&self.core)
    }

    /// Begins a pairing activation.
    ///
    /// # Errors
    ///
    /// Refused unless pairing is idle or finished.
    pub fn start_pairing(&self, method: PairingMethod) -> Result<(), BackendError> {
        pairing::start(&self.core, method)
    }

    /// Dials a phone from the search list, or a typed address.
    ///
    /// # Errors
    ///
    /// Refused when the target does not fit the current step.
    pub fn pair_with(&self, target: PhoneTarget) -> Result<(), BackendError> {
        pairing::pair_with(&self.core, target)
    }

    /// The user's answer to "do the codes match?".
    ///
    /// # Errors
    ///
    /// Refused when no comparison is waiting.
    pub fn answer_code(&self, matches: bool) -> Result<(), BackendError> {
        pairing::answer_code(&self.core, matches)
    }

    /// Stops pairing, or dismisses a finished one.
    pub fn cancel_pairing(&self) {
        pairing::cancel(&self.core);
    }

    /// Forgets a phone: tells it, if a session is up, and deletes the
    /// record.
    ///
    /// # Errors
    ///
    /// Refused for a phone that is not paired.
    pub fn unpair(&self, device_id: &str) -> Result<(), BackendError> {
        let core = &self.core;
        let record = lock(&core.store)
            .list()
            .into_iter()
            .find(|record| record.id == device_id)
            .ok_or(BackendError::NotFound)?;
        let removed = records::unpair(&mut *lock(&core.store), &record.fingerprint);
        match removed {
            Ok(Some(_)) => {}
            Ok(None) => return Err(BackendError::NotFound),
            Err(error) => {
                tracing::error!(%error, "could not remove the pairing record");
                return Err(BackendError::WrongState);
            }
        }
        // The task says goodbye if a session is up, and then ends.
        if let Some(slot) = lock(&core.slots).get(device_id) {
            let _ = slot.commands.send(Command::Unpair);
        }
        core.forget_device(device_id, false);
        Ok(())
    }

    /// Asks a phone to connect again right away, for example after the
    /// user disconnected on the phone.
    pub fn reconnect(&self, device_id: &str) {
        if let Some(slot) = lock(&self.core.slots).get(device_id) {
            let _ = slot.commands.send(Command::Reconnect);
        }
    }

    /// The notifications mirrored from a phone, in no particular order.
    ///
    /// # Errors
    ///
    /// Refused for a phone that is not paired.
    pub fn notifications(
        &self,
        device_id: &str,
    ) -> Result<Vec<MirroredNotification>, BackendError> {
        let slots = lock(&self.core.slots);
        let slot = slots.get(device_id).ok_or(BackendError::NotFound)?;
        let board = lock(&slot.board);
        Ok(board.values().cloned().collect())
    }

    /// Dismisses a notification on the phone. It is queued, and sent when
    /// the phone is connected.
    ///
    /// # Errors
    ///
    /// Refused for a phone that is not paired.
    pub fn dismiss_notification(&self, device_id: &str, key: &str) -> Result<(), BackendError> {
        self.send(
            device_id,
            Command::Dismiss {
                key: key.to_owned(),
            },
        )
    }

    /// Runs a notification's action on the phone, with the text of a reply
    /// for actions that take one. It is queued, and sent when the phone is
    /// connected.
    ///
    /// # Errors
    ///
    /// Refused for a phone that is not paired.
    pub fn run_notification_action(
        &self,
        device_id: &str,
        key: &str,
        action_id: &str,
        reply: Option<&str>,
    ) -> Result<(), BackendError> {
        self.send(
            device_id,
            Command::Action {
                key: key.to_owned(),
                action_id: action_id.to_owned(),
                reply: reply.map(str::to_owned),
            },
        )
    }

    /// A picture a phone shared with a notification, named by the SHA-256
    /// of its bytes in hexadecimal, if it has arrived.
    #[must_use]
    pub fn notification_image(&self, device_id: &str, sha256: &str) -> Option<CachedImage> {
        let bytes = decode_hex(sha256)?;
        self.core.drivers.image(device_id, &bytes)
    }

    fn send(&self, device_id: &str, command: Command) -> Result<(), BackendError> {
        let slots = lock(&self.core.slots);
        let slot = slots.get(device_id).ok_or(BackendError::NotFound)?;
        slot.commands
            .send(command)
            .map_err(|_| BackendError::NotFound)
    }

    /// Tells the sessions whether the user can see the window, which sets
    /// how often they ping. On a desktop that lets the app use its
    /// clipboard only while the window has focus, gaining focus also puts
    /// text from a phone on the clipboard, or shares what the user copied
    /// meanwhile.
    pub fn set_active(&self, active: bool) {
        self.core.active.store(active, Ordering::Relaxed);
        for slot in lock(&self.core.slots).values() {
            let _ = slot.commands.send(Command::SetActive(active));
        }
        clipboard::window_focus(&self.core, active);
    }

    /// What the clipboard page shows.
    #[must_use]
    pub fn clipboard(&self) -> ClipboardStatus {
        self.core.clipboard_status()
    }

    /// Sends what the clipboard holds to every phone, as the user asked.
    /// What came of it arrives as a new clipboard status.
    pub fn send_clipboard(&self) {
        clipboard::send_now(&self.core);
    }

    /// The file transfers with phone `device_id`, oldest first.
    ///
    /// # Errors
    ///
    /// Fails for a phone that is not paired.
    pub fn transfers(&self, device_id: &str) -> Result<Vec<FileTransfer>, BackendError> {
        let transfers = self
            .core
            .files
            .transfers(device_id)
            .ok_or(BackendError::NotFound)?;
        Ok(transfers.iter().map(files::dto::transfer).collect())
    }

    /// Sends the files at `paths` to phone `device_id` as one transfer: at
    /// once when it is connected, otherwise once it is back, for up to an
    /// hour. It looks at every file first, so it blocks.
    ///
    /// # Errors
    ///
    /// Fails while files are turned off here, for a phone that is not
    /// paired, and for files that cannot be sent: a folder, a file that
    /// cannot be read, none or too many.
    pub fn send_files(&self, device_id: &str, paths: Vec<PathBuf>) -> Result<(), BackendError> {
        if self.core.files.availability() != Availability::Ready {
            return Err(BackendError::FilesOff);
        }
        files::check_files(&paths)?;
        self.send(device_id, Command::SendFiles { paths })
    }

    /// Cancels a transfer with phone `device_id` that has not finished.
    ///
    /// # Errors
    ///
    /// Fails for a phone that is not paired, or a malformed transfer id.
    pub fn cancel_transfer(&self, device_id: &str, transfer_id: &str) -> Result<(), BackendError> {
        let id = TransferId::from_hex(transfer_id).ok_or(BackendError::NotFound)?;
        self.send(device_id, Command::CancelTransfer { id })
    }

    /// Removes a finished transfer with phone `device_id` from the list.
    ///
    /// # Errors
    ///
    /// Fails for a phone that is not paired, or a malformed transfer id.
    pub fn forget_transfer(&self, device_id: &str, transfer_id: &str) -> Result<(), BackendError> {
        let id = TransferId::from_hex(transfer_id).ok_or(BackendError::NotFound)?;
        self.send(device_id, Command::ForgetTransfer { id })
    }

    /// Opens file `index` of a transfer from phone `device_id` with the
    /// desktop's default application, once it has arrived.
    ///
    /// # Errors
    ///
    /// Fails when there is no such received file.
    pub fn open_received_file(
        &self,
        device_id: &str,
        transfer_id: &str,
        index: usize,
    ) -> Result<(), BackendError> {
        self.with_received_file(device_id, transfer_id, index, false)
    }

    /// Shows file `index` of a transfer from phone `device_id` in its
    /// folder, once it has arrived.
    ///
    /// # Errors
    ///
    /// Fails when there is no such received file.
    pub fn show_received_file(
        &self,
        device_id: &str,
        transfer_id: &str,
        index: usize,
    ) -> Result<(), BackendError> {
        self.with_received_file(device_id, transfer_id, index, true)
    }

    fn with_received_file(
        &self,
        device_id: &str,
        transfer_id: &str,
        index: usize,
        show: bool,
    ) -> Result<(), BackendError> {
        let files = &self.core.files;
        let path = files
            .received_file(device_id, transfer_id, index)
            .ok_or(BackendError::NotFound)?;
        let opener = files.opener();
        self.core.handle.spawn(async move {
            let done = if show {
                opener.show_in_folder(&path).await
            } else {
                opener.open(&path).await
            };
            if let Err(error) = done {
                tracing::warn!(%error, "could not open a received file");
                // The file stays reachable: offer its folder instead.
                if !show && let Err(error) = opener.show_in_folder(&path).await {
                    tracing::warn!(%error, "could not show a received file");
                }
            }
        });
        Ok(())
    }

    /// Turns files on or off. It applies at once, and the phones hear of
    /// it.
    pub fn set_files_enabled(&self, enabled: bool) {
        let core = &self.core;
        core.files.set_enabled(enabled);
        for slot in lock(&core.slots).values() {
            let _ = slot.commands.send(Command::FilesSwitch);
        }
    }

    /// Changes the clipboard settings. They apply at once, and the phones
    /// are told when sharing turns on or off.
    pub fn set_clipboard_settings(&self, settings: ClipboardSettings) {
        let core = &self.core;
        if core.clipboard_hub.settings() == settings {
            return;
        }
        core.clipboard_hub.set_settings(settings);
        if core.clipboard.is_some() {
            for slot in lock(&core.slots).values() {
                let _ = slot.commands.send(Command::ClipboardSharing);
            }
        }
        core.clipboard_changed();
    }

    /// Phone `device_id`'s battery, signal and sound mode, as the UI shows
    /// them. Changes arrive through the event sink.
    ///
    /// # Errors
    ///
    /// Fails for a phone that is not paired.
    pub fn phone_status(&self, device_id: &str) -> Result<PhoneStatus, BackendError> {
        self.check_paired(device_id)?;
        Ok(self.core.phone.status(device_id))
    }

    /// Whether phone `device_id` rings, and why it cannot or did not.
    /// Changes arrive through the event sink.
    ///
    /// # Errors
    ///
    /// Fails for a phone that is not paired.
    pub fn ring_status(&self, device_id: &str) -> Result<RingStatus, BackendError> {
        self.check_paired(device_id)?;
        Ok(self.core.phone.ring(device_id))
    }

    /// Makes phone `device_id` ring for a minute, even when it is set to
    /// silent. What came of it arrives as a new ring status.
    ///
    /// # Errors
    ///
    /// Fails for a phone that is not paired.
    pub fn ring_phone(&self, device_id: &str) -> Result<(), BackendError> {
        self.send(device_id, Command::Phone(PhoneCommand::Ring))
    }

    /// Stops phone `device_id` ringing.
    ///
    /// # Errors
    ///
    /// Fails for a phone that is not paired.
    pub fn stop_ringing(&self, device_id: &str) -> Result<(), BackendError> {
        self.send(device_id, Command::Phone(PhoneCommand::StopRinging))
    }

    /// Shows or hides the phones' status. It applies at once, and the
    /// phones hear of it.
    pub fn set_phone_status_shown(&self, shown: bool) {
        let core = &self.core;
        if !core.phone.set_shown(shown) {
            return;
        }
        for slot in lock(&core.slots).values() {
            let _ = slot
                .commands
                .send(Command::Phone(PhoneCommand::StatusSwitch));
        }
    }

    /// Phone `device_id`'s screen as the UI shows it. Changes arrive
    /// through the event sink.
    ///
    /// # Errors
    ///
    /// Fails for a phone that is not paired.
    pub fn screen_status(&self, device_id: &str) -> Result<ScreenStatus, BackendError> {
        self.check_paired(device_id)?;
        Ok(self.core.phone.screen(device_id))
    }

    /// Asks phone `device_id` to show its screen in `viewer`, in one of
    /// `codecs` (most preferred first) with its longer side at most
    /// `max_size` pixels, or as the phone likes with 0. The phone asks its
    /// user first; what came of it arrives as a new screen status.
    ///
    /// # Errors
    ///
    /// Fails for a phone that is not paired.
    pub fn start_screen(
        &self,
        device_id: &str,
        codecs: &[ScreenCodec],
        max_size: u32,
        viewer: Arc<dyn ScreenViewer>,
    ) -> Result<(), BackendError> {
        let request = link2linux_core::screen::ScreenRequest {
            codecs: codecs.iter().map(|codec| codec.to_core()).collect(),
            max_size,
        };
        self.send(
            device_id,
            Command::Screen(ScreenCommand::Start { request, viewer }),
        )
    }

    /// Stops showing phone `device_id`'s screen.
    ///
    /// # Errors
    ///
    /// Fails for a phone that is not paired.
    pub fn stop_screen(&self, device_id: &str) -> Result<(), BackendError> {
        self.send(device_id, Command::Screen(ScreenCommand::Stop))
    }

    /// Sends what the user did in the window that shows phone
    /// `device_id`'s screen. Input the phone did not take shows in the
    /// screen status.
    ///
    /// # Errors
    ///
    /// Fails for a phone that is not paired.
    pub fn send_screen_input(
        &self,
        device_id: &str,
        input: ScreenInput,
    ) -> Result<(), BackendError> {
        self.send(
            device_id,
            Command::Screen(ScreenCommand::Input(input.into_core())),
        )
    }

    /// Asks phone `device_id` for a key frame, because the window could not
    /// decode a frame or fell behind.
    ///
    /// # Errors
    ///
    /// Fails for a phone that is not paired.
    pub fn request_screen_key_frame(&self, device_id: &str) -> Result<(), BackendError> {
        self.send(device_id, Command::Screen(ScreenCommand::KeyFrame))
    }

    /// Phone `device_id`'s recent photos, as the photos page shows them.
    /// Changes arrive through the event sink.
    ///
    /// # Errors
    ///
    /// Fails for a phone that is not paired.
    pub fn photos(&self, device_id: &str) -> Result<PhonePhotos, BackendError> {
        self.check_paired(device_id)?;
        Ok(self.core.phone.photos(device_id))
    }

    /// A thumbnail of photo `photo_id` of phone `device_id`, at most
    /// `max_size` pixels on its longest side: from memory when the desktop
    /// has it, otherwise from the phone. The future holds no borrow of the
    /// runtime; dropping it tells the phone's task the thumbnail is no
    /// longer wanted.
    pub fn photo_thumbnail(
        &self,
        device_id: &str,
        photo_id: &str,
        max_size: u32,
    ) -> impl Future<Output = Result<Thumbnail, BackendError>> + Send + 'static {
        let (reply, answer) = oneshot::channel();
        let sent = self.send(
            device_id,
            Command::Photos(PhotoCommand::Thumbnail {
                id: photo_id.to_owned(),
                max_size,
                reply,
            }),
        );
        async move {
            sent?;
            answer
                .await
                .map_err(|_| BackendError::PhoneNotConnected)?
                .map_err(photos::dto::error)
        }
    }

    /// Fetches photo `photo_id` of phone `device_id` and saves it in the
    /// user's Pictures directory, as a received file is saved: under a free
    /// name, never replacing a file. The future holds no borrow of the
    /// runtime.
    pub fn save_photo(
        &self,
        device_id: &str,
        photo_id: &str,
    ) -> impl Future<Output = Result<SavedPhoto, BackendError>> + Send + 'static {
        let (reply, answer) = oneshot::channel();
        let sent = self.send(
            device_id,
            Command::Photos(PhotoCommand::Fetch {
                id: photo_id.to_owned(),
                reply,
            }),
        );
        let core = self.core.clone();
        let (device_id, photo_id) = (device_id.to_owned(), photo_id.to_owned());
        async move {
            sent?;
            let photo = answer
                .await
                .map_err(|_| BackendError::PhoneNotConnected)?
                .map_err(photos::dto::error)?;
            let opener = core.files.opener();
            let path = core
                .handle
                .spawn_blocking(move || {
                    let dir = opener.pictures_dir().map_err(|error| {
                        tracing::warn!(%error, "no directory to save photos in");
                        BackendError::PhotoNotSaved
                    })?;
                    // The error only: its message may name the folder.
                    link2linux_core::photos::save(&dir, &photo).map_err(|error| {
                        tracing::warn!(kind = %error.kind(), "could not save a photo");
                        BackendError::PhotoNotSaved
                    })
                })
                .await
                .map_err(|_| BackendError::PhotoNotSaved)??;
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            core.saved_photos.insert(&device_id, &photo_id, path);
            Ok(SavedPhoto { name })
        }
    }

    /// Opens the photo `photo_id` of phone `device_id` that the user saved
    /// with the desktop's default app, or shows its folder where that fails.
    ///
    /// # Errors
    ///
    /// Fails when the user has not saved that photo.
    pub fn open_saved_photo(&self, device_id: &str, photo_id: &str) -> Result<(), BackendError> {
        self.with_saved_photo(device_id, photo_id, false)
    }

    /// Shows the photo `photo_id` of phone `device_id` that the user saved
    /// in its folder.
    ///
    /// # Errors
    ///
    /// Fails when the user has not saved that photo.
    pub fn show_saved_photo(&self, device_id: &str, photo_id: &str) -> Result<(), BackendError> {
        self.with_saved_photo(device_id, photo_id, true)
    }

    fn with_saved_photo(
        &self,
        device_id: &str,
        photo_id: &str,
        show: bool,
    ) -> Result<(), BackendError> {
        let path = self
            .core
            .saved_photos
            .get(device_id, photo_id)
            .ok_or(BackendError::NotFound)?;
        let opener = self.core.files.opener();
        self.core.handle.spawn(async move {
            let done = if show {
                opener.show_in_folder(&path).await
            } else {
                opener.open(&path).await
            };
            if let Err(error) = done {
                tracing::warn!(%error, "could not open a saved photo");
                if !show && let Err(error) = opener.show_in_folder(&path).await {
                    tracing::warn!(%error, "could not show a saved photo");
                }
            }
        });
        Ok(())
    }

    fn check_paired(&self, device_id: &str) -> Result<(), BackendError> {
        if lock(&self.core.slots).contains_key(device_id) {
            Ok(())
        } else {
            Err(BackendError::NotFound)
        }
    }

    /// The phones' calls, and this computer's part in them.
    #[must_use]
    pub fn calls(&self) -> CallsStatus {
        calls::status(&self.core)
    }

    /// Turns "Use this computer for calls" on or off. Turned off, the
    /// phones' hands-free links are disconnected, so the phones keep their
    /// calls; the phones hear of it.
    pub fn set_calls_enabled(&self, enabled: bool) {
        calls::set_enabled(&self.core, enabled);
    }

    /// Opens Bluetooth pairing with phone `device_id` for 2 minutes, and
    /// asks the phone to pair once it can.
    ///
    /// # Errors
    ///
    /// Fails for an unknown phone, or when calls can't be used now.
    pub fn start_bluetooth_pairing(&self, device_id: &str) -> Result<(), BackendError> {
        calls::start_pairing(&self.core, device_id)
    }

    /// Closes Bluetooth pairing.
    pub fn cancel_bluetooth_pairing(&self) {
        calls::cancel_pairing(&self.core);
    }

    /// The user compared the code of pairing prompt `prompt`: `matches`
    /// says whether both devices show the same one.
    ///
    /// # Errors
    ///
    /// Fails when the prompt is over.
    pub fn confirm_bluetooth_code(&self, prompt: u32, matches: bool) -> Result<(), BackendError> {
        calls::confirm_code(&self.core, prompt, matches)
    }

    /// The user picked the paired Bluetooth device at `address` as phone
    /// `device_id`'s.
    ///
    /// # Errors
    ///
    /// Fails for an unknown phone or device.
    pub fn choose_bluetooth_device(
        &self,
        device_id: &str,
        address: &str,
    ) -> Result<(), BackendError> {
        calls::choose_device(&self.core, device_id, address)
    }

    /// Connects phone `device_id`'s hands-free link with this computer.
    ///
    /// # Errors
    ///
    /// Fails for an unknown phone, one not paired over Bluetooth, or when
    /// calls can't be used now.
    pub fn connect_calls(&self, device_id: &str) -> Result<(), BackendError> {
        calls::connect(&self.core, device_id)
    }

    /// Answers call `call_id` of phone `device_id`.
    ///
    /// # Errors
    ///
    /// Fails when the call is gone or does not ring.
    pub fn answer_call(&self, device_id: &str, call_id: &str) -> Result<(), BackendError> {
        calls::act(&self.core, device_id, call_id, CallAction::Answer)
    }

    /// Declines the ringing call `call_id` of phone `device_id`.
    ///
    /// # Errors
    ///
    /// Fails when the call is gone.
    pub fn decline_call(&self, device_id: &str, call_id: &str) -> Result<(), BackendError> {
        calls::act(&self.core, device_id, call_id, CallAction::Decline)
    }

    /// Ends call `call_id` of phone `device_id`.
    ///
    /// # Errors
    ///
    /// Fails when the call is gone.
    pub fn hang_up_call(&self, device_id: &str, call_id: &str) -> Result<(), BackendError> {
        calls::act(&self.core, device_id, call_id, CallAction::HangUp)
    }

    /// Calls `number` from phone `device_id`.
    ///
    /// # Errors
    ///
    /// Fails for a number a phone can't dial, or while the phone's
    /// hands-free link is down.
    pub fn dial(&self, device_id: &str, number: &str) -> Result<(), BackendError> {
        calls::dial(&self.core, device_id, number)
    }

    /// Sends touch tones in phone `device_id`'s active call.
    ///
    /// # Errors
    ///
    /// Fails for keys that are not on a keypad, or while the phone's
    /// hands-free link is down.
    pub fn send_tones(&self, device_id: &str, tones: &str) -> Result<(), BackendError> {
        calls::send_tones(&self.core, device_id, tones)
    }

    /// Mutes or unmutes this computer's microphone in phone `device_id`'s
    /// calls.
    ///
    /// # Errors
    ///
    /// Fails while the phone's hands-free link is down, or where it can't
    /// be muted.
    pub fn set_call_muted(&self, device_id: &str, muted: bool) -> Result<(), BackendError> {
        calls::set_muted(&self.core, device_id, muted)
    }

    /// The Bluetooth LE address this computer advertises from, which QR
    /// codes and new sessions carry.
    pub fn set_ble_address(&self, address: Option<[u8; 6]>) {
        *lock(&self.core.ble_address) = address;
    }

    /// Whether the pairing file is sealed but the runtime started without
    /// its key: the shell may ask the keyring again later.
    #[must_use]
    pub fn needs_store_key(&self) -> bool {
        lock(&self.core.store).needs_key()
    }

    /// Hands over the key that seals the presence keys in the pairing file,
    /// when the runtime started without it. The presence keys sealed under
    /// it are found again.
    pub fn use_store_key(&self, key: StoreKey) {
        if let Err(error) = lock(&self.core.store).use_key(key) {
            tracing::warn!(%error, "could not seal the presence keys in the pairing file");
        }
        self.core.publish_keys();
    }

    /// The UDP port the beacon listener is bound to.
    #[must_use]
    pub fn beacon_port(&self) -> u16 {
        self.core.beacon_port.load(Ordering::Relaxed)
    }

    /// Ends every session with a `Goodbye` and stops all tasks.
    pub fn shutdown(&self) {
        self.core.cancel.cancel();
    }

    /// As [`Runtime::shutdown`], then blocks the calling thread, which must
    /// not be one of the runtime's workers, until every phone's task has
    /// finished (unfinished downloads deleted) or `limit` has passed.
    pub fn shutdown_and_wait(&self, limit: Duration) {
        self.shutdown();
        let end = std::time::Instant::now() + limit;
        while self.core.supervisors.load(Ordering::SeqCst) > 0 && std::time::Instant::now() < end {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Reads lowercase or uppercase hexadecimal; `None` for anything else.
fn decode_hex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::decode_hex;

    #[test]
    fn hex_is_read_strictly() {
        assert_eq!(decode_hex("00ffAb"), Some(vec![0, 255, 171]));
        assert_eq!(decode_hex(""), Some(vec![]));
        for bad in ["0", "zz", "0g", "+1", "\u{e9}\u{e9}"] {
            assert_eq!(decode_hex(bad), None, "{bad}");
        }
    }
}
