//! A scripted stand-in for the real pairing backend.
//!
//! [`Machine`] is a pure state machine: every method takes the current time
//! explicitly, so tests drive it with made-up clocks and never sleep.
//! [`FakeBackend`] wraps it with a real clock and one thread that sleeps until
//! the next deadline, so the UI sees the same events a real backend would
//! send. Choose a [`Script`] with the `L2L_FAKE_SCRIPT` environment variable
//! (`success`, `wrong-code`, `expired`, `not-found`, `declined`, `demo`,
//! `incoming-call`).

pub mod calls;
pub mod files;
pub mod machine;
pub mod phone;
pub mod photos;
pub mod screen;
pub mod script;
pub mod support;

use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use link2linux_runtime::view::{
    ClipboardActivity, ClipboardDirection, ClipboardMode, ClipboardPhone, ClipboardPhoneState,
    ClipboardProblem, ClipboardProblemKind, ClipboardStatus, DeviceStatus,
};

use super::{
    Backend, BackendError, CallsStatus, Device, EventSink, FileTransfer, PairingMethod,
    PairingState, Pending, PhonePhotos, PhoneStatus, PhoneTarget, RingStatus, SavedPhoto,
    ScreenCodec, ScreenInput, ScreenStatus, ScreenViewer, Settings,
};

use calls::{Delays, FakeCalls};
use files::FakeFiles;
use phone::FakePhone;
use photos::FakePhotos;
use screen::{FakeScreen, ScreenDelays};

pub use machine::Machine;
pub use script::{Changed, Now, Script};

/// The computer's name, shown on the phone before it connects.
pub fn desktop_name() -> String {
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
        .or_else(|| {
            std::env::var("HOSTNAME")
                .ok()
                .filter(|name| !name.is_empty())
        })
        .unwrap_or_else(|| "This computer".to_owned())
}

struct Clock {
    start: Instant,
}

impl Clock {
    fn now(&self) -> Now {
        let unix_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|d| i64::try_from(d.as_millis()).ok())
            .unwrap_or(0);
        Now {
            mono_ms: u64::try_from(self.start.elapsed().as_millis()).unwrap_or(u64::MAX),
            unix_ms,
        }
    }
}

struct Inner {
    machine: Machine,
    stop: bool,
}

struct Shared {
    inner: Mutex<Inner>,
    wake: Condvar,
    clock: Clock,
    sink: Arc<dyn EventSink>,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        // The machine is plain data, so a panic elsewhere cannot leave it
        // half-updated.
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Reports what changed, then wakes the timer thread to re-read deadlines.
    fn publish(&self, inner: &Inner, changed: Changed, now: Now) {
        if changed.devices {
            self.sink.devices_changed(&inner.machine.devices());
        }
        if changed.pairing {
            self.sink.pairing_changed(&inner.machine.pairing(now));
        }
        self.wake.notify_all();
    }

    fn run<T>(
        &self,
        f: impl FnOnce(&mut Machine, Now) -> Result<(Changed, T), BackendError>,
    ) -> Result<T, BackendError> {
        let mut inner = self.lock();
        let now = self.clock.now();
        // Bring the machine up to date first, so a late timer never makes a
        // command act on a stale step.
        let due = inner.machine.tick(now);
        let (changed, value) = f(&mut inner.machine, now)?;
        let mut all = due;
        all.merge(changed);
        self.publish(&inner, all, now);
        Ok(value)
    }
}

/// The scripted backend the app uses until the real one is wired in.
pub struct FakeBackend {
    shared: Arc<Shared>,
    files: FakeFiles,
    calls: FakeCalls,
    timer: Option<JoinHandle<()>>,
    clipboard: Mutex<FakeClipboard>,
    phone: FakePhone,
    screen: FakeScreen,
    photos: FakePhotos,
    /// The settings, kept in memory only.
    settings: Mutex<Settings>,
}

/// What the clipboard page shows of the scripted phones.
#[derive(Default)]
struct FakeClipboard {
    /// Newest first.
    activity: Vec<ClipboardActivity>,
    problem: Option<ClipboardProblem>,
}

impl FakeClipboard {
    /// The page for `devices`, which share automatically while connected.
    fn status(&self, devices: &[Device]) -> ClipboardStatus {
        ClipboardStatus {
            mode: ClipboardMode::Automatic,
            phones: devices
                .iter()
                .map(|device| ClipboardPhone {
                    device_id: device.id.clone(),
                    name: device.name.clone(),
                    state: if device.status == DeviceStatus::Connected {
                        ClipboardPhoneState::Ready
                    } else {
                        ClipboardPhoneState::Disconnected
                    },
                })
                .collect(),
            activity: self.activity.clone(),
            problem: self.problem.clone(),
            held: None,
        }
    }

    /// The user sent the clipboard at `now`: it reaches every connected
    /// phone, or none is there to take it.
    fn send(&mut self, devices: &[Device], now: i64) {
        let ready: Vec<_> = devices
            .iter()
            .filter(|device| device.status == DeviceStatus::Connected)
            .collect();
        self.problem = ready.is_empty().then_some(ClipboardProblem {
            kind: ClipboardProblemKind::NoPhone,
            device_id: None,
            phone_name: None,
            bytes: None,
            at_unix_ms: now,
        });
        for device in ready {
            self.activity.insert(
                0,
                ClipboardActivity {
                    device_id: device.id.clone(),
                    phone_name: device.name.clone(),
                    direction: ClipboardDirection::Sent,
                    bytes: 18,
                    sensitive: false,
                    at_unix_ms: now,
                },
            );
        }
        self.activity
            .truncate(link2linux_runtime::clipboard::MAX_ACTIVITY);
    }
}

impl FakeBackend {
    pub fn new(script: Script, sink: Arc<dyn EventSink>) -> Self {
        let clock = Clock {
            start: Instant::now(),
        };
        let machine = Machine::new(script, desktop_name(), clock.now());
        let files = FakeFiles::new(script, sink.clone(), clock.now().unix_ms);
        let phone = FakePhone::new(sink.clone(), phone::RING_FOR);
        let screen = FakeScreen::new(sink.clone(), ScreenDelays::real());
        let photos = FakePhotos::new(clock.now().unix_ms);
        let calls = FakeCalls::new(script, sink.clone(), desktop_name(), Delays::real());
        let shared = Arc::new(Shared {
            inner: Mutex::new(Inner {
                machine,
                stop: false,
            }),
            wake: Condvar::new(),
            clock,
            sink,
        });
        let timer = std::thread::Builder::new()
            .name("fake-backend-timer".to_owned())
            .spawn({
                let shared = shared.clone();
                move || timer_loop(&shared)
            })
            .ok();
        Self {
            shared,
            files,
            calls,
            timer,
            clipboard: Mutex::default(),
            phone,
            screen,
            photos,
            settings: Mutex::default(),
        }
    }

    /// Uses the script named by `L2L_FAKE_SCRIPT`, defaulting to `success`.
    pub fn from_env(sink: Arc<dyn EventSink>) -> Self {
        let script = std::env::var("L2L_FAKE_SCRIPT")
            .ok()
            .and_then(|name| Script::parse(&name))
            .unwrap_or_default();
        Self::new(script, sink)
    }
}

fn timer_loop(shared: &Shared) {
    let mut inner = shared.lock();
    while !inner.stop {
        let now = shared.clock.now();
        let changed = inner.machine.tick(now);
        shared.publish(&inner, changed, now);
        inner = match inner.machine.next_deadline() {
            Some(deadline) => {
                let wait = Duration::from_millis(deadline.saturating_sub(now.mono_ms).max(1));
                shared
                    .wake
                    .wait_timeout(inner, wait)
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .0
            }
            None => shared
                .wake
                .wait(inner)
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        };
    }
}

impl Drop for FakeBackend {
    fn drop(&mut self) {
        self.shared.lock().stop = true;
        self.shared.wake.notify_all();
        if let Some(timer) = self.timer.take() {
            let _ = timer.join();
        }
    }
}

impl Backend for FakeBackend {
    fn devices(&self) -> Vec<Device> {
        self.shared.lock().machine.devices()
    }

    fn pairing(&self) -> PairingState {
        let mut inner = self.shared.lock();
        let now = self.shared.clock.now();
        // Catch up on anything that came due, so a snapshot is never stale.
        let due = inner.machine.tick(now);
        self.shared.publish(&inner, due, now);
        inner.machine.pairing(now)
    }

    fn start_pairing(&self, method: PairingMethod) -> Result<(), BackendError> {
        self.shared
            .run(|m, now| m.start_pairing(method, now).map(|c| (c, ())))
    }

    fn pair_with(&self, target: PhoneTarget) -> Result<(), BackendError> {
        self.shared
            .run(|m, now| m.pair_with(target, now).map(|c| (c, ())))
    }

    fn answer_code(&self, matches: bool) -> Result<(), BackendError> {
        self.shared
            .run(|m, now| m.answer_code(matches, now).map(|c| (c, ())))
    }

    fn cancel_pairing(&self) {
        let _ = self.shared.run(|m, _| Ok((m.cancel_pairing(), ())));
    }

    fn unpair(&self, device_id: &str) -> Result<(), BackendError> {
        self.shared.run(|m, _| m.unpair(device_id).map(|c| (c, ())))
    }

    fn clipboard(&self) -> ClipboardStatus {
        let devices = self.devices();
        self.lock_clipboard().status(&devices)
    }

    fn send_clipboard(&self) -> Result<(), BackendError> {
        let devices = self.devices();
        let status = {
            let mut clipboard = self.lock_clipboard();
            clipboard.send(&devices, self.shared.clock.now().unix_ms);
            clipboard.status(&devices)
        };
        self.shared.sink.clipboard_changed(&status);
        Ok(())
    }

    fn transfers(&self, device_id: &str) -> Result<Vec<FileTransfer>, BackendError> {
        Ok(self.files.transfers(device_id))
    }

    fn phone_status(&self, device_id: &str) -> Result<PhoneStatus, BackendError> {
        FakePhone::status(self.device(device_id).as_ref())
    }

    fn ring_status(&self, device_id: &str) -> Result<RingStatus, BackendError> {
        self.phone.ring_status(self.device(device_id).as_ref())
    }

    fn ring_phone(&self, device_id: &str) -> Result<(), BackendError> {
        self.phone.ring(self.device(device_id).as_ref())
    }

    fn stop_ringing(&self, device_id: &str) -> Result<(), BackendError> {
        self.phone.stop(self.device(device_id).as_ref())
    }

    fn screen_status(&self, device_id: &str) -> Result<ScreenStatus, BackendError> {
        self.screen.status(self.device(device_id).as_ref())
    }

    fn start_screen(
        &self,
        device_id: &str,
        codecs: &[ScreenCodec],
        _max_size: u32,
        viewer: Arc<dyn ScreenViewer>,
    ) -> Result<(), BackendError> {
        self.screen
            .start(self.device(device_id).as_ref(), codecs, viewer)
    }

    fn stop_screen(&self, device_id: &str) -> Result<(), BackendError> {
        self.screen.stop(self.device(device_id).as_ref())
    }

    fn send_screen_input(&self, device_id: &str, input: ScreenInput) -> Result<(), BackendError> {
        self.screen.input(self.device(device_id).as_ref(), &input)
    }

    fn photos(&self, device_id: &str) -> Result<PhonePhotos, BackendError> {
        self.photos.list(self.device(device_id).as_ref())
    }

    fn photo_thumbnail(
        &self,
        device_id: &str,
        photo_id: &str,
        _max_size: u32,
    ) -> Pending<Arc<[u8]>> {
        let thumbnail = self
            .photos
            .thumbnail(self.device(device_id).as_ref(), photo_id);
        Box::pin(std::future::ready(thumbnail))
    }

    fn save_photo(&self, device_id: &str, photo_id: &str) -> Pending<SavedPhoto> {
        let saved = self.photos.save(self.device(device_id).as_ref(), photo_id);
        Box::pin(std::future::ready(saved))
    }

    fn open_saved_photo(&self, device_id: &str, photo_id: &str) -> Result<(), BackendError> {
        self.photos.saved(device_id, photo_id)
    }

    fn show_saved_photo(&self, device_id: &str, photo_id: &str) -> Result<(), BackendError> {
        self.photos.saved(device_id, photo_id)
    }

    fn send_files(&self, device_id: &str, paths: Vec<PathBuf>) -> Result<(), BackendError> {
        self.files
            .send(device_id, &paths, self.shared.clock.now().unix_ms)
    }

    fn cancel_transfer(&self, device_id: &str, transfer_id: &str) -> Result<(), BackendError> {
        self.files.cancel(device_id, transfer_id)
    }

    fn forget_transfer(&self, device_id: &str, transfer_id: &str) -> Result<(), BackendError> {
        self.files.forget(device_id, transfer_id)
    }

    fn open_received_file(
        &self,
        device_id: &str,
        transfer_id: &str,
        index: usize,
    ) -> Result<(), BackendError> {
        self.files.received(device_id, transfer_id, index)
    }

    fn show_received_file(
        &self,
        device_id: &str,
        transfer_id: &str,
        index: usize,
    ) -> Result<(), BackendError> {
        self.files.received(device_id, transfer_id, index)
    }

    fn calls(&self) -> CallsStatus {
        self.calls.status(&self.devices())
    }

    fn start_bluetooth_pairing(&self, device_id: &str) -> Result<(), BackendError> {
        self.calls.status(&self.devices());
        self.calls.start_pairing(device_id)
    }

    fn cancel_bluetooth_pairing(&self) {
        self.calls.cancel_pairing();
    }

    fn confirm_bluetooth_code(&self, prompt: u32, matches: bool) -> Result<(), BackendError> {
        self.calls.confirm_code(prompt, matches)
    }

    fn choose_bluetooth_device(&self, device_id: &str, address: &str) -> Result<(), BackendError> {
        self.calls.choose_device(device_id, address)
    }

    fn connect_calls(&self, device_id: &str) -> Result<(), BackendError> {
        self.calls.connect(device_id)
    }

    fn answer_call(&self, device_id: &str, call_id: &str) -> Result<(), BackendError> {
        self.calls.answer(device_id, call_id)
    }

    fn decline_call(&self, device_id: &str, call_id: &str) -> Result<(), BackendError> {
        self.calls.end(device_id, call_id)
    }

    fn hang_up_call(&self, device_id: &str, call_id: &str) -> Result<(), BackendError> {
        self.calls.end(device_id, call_id)
    }

    fn dial(&self, device_id: &str, number: &str) -> Result<(), BackendError> {
        self.calls.dial(device_id, number)
    }

    fn send_tones(&self, device_id: &str, tones: &str) -> Result<(), BackendError> {
        self.calls.send_tones(device_id, tones)
    }

    fn set_call_muted(&self, device_id: &str, muted: bool) -> Result<(), BackendError> {
        self.calls.set_muted(device_id, muted)
    }

    fn settings(&self) -> Settings {
        *self.settings.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn set_settings(&self, settings: Settings) -> Result<(), BackendError> {
        let before = std::mem::replace(
            &mut *self.settings.lock().unwrap_or_else(|p| p.into_inner()),
            settings,
        );
        if before.use_for_calls != settings.use_for_calls {
            self.calls.set_enabled(settings.use_for_calls);
        }
        Ok(())
    }
}

impl FakeBackend {
    /// The paired phone `id`, if there is one.
    fn device(&self, id: &str) -> Option<Device> {
        self.devices().into_iter().find(|device| device.id == id)
    }

    fn lock_clipboard(&self) -> MutexGuard<'_, FakeClipboard> {
        self.clipboard
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex as StdMutex;

    use super::*;

    #[derive(Default)]
    struct Recorder {
        log: StdMutex<Vec<String>>,
    }

    impl EventSink for Recorder {
        fn pairing_changed(&self, state: &PairingState) {
            let step = match state {
                PairingState::QrOffer { .. } => "qrOffer",
                PairingState::Idle => "idle",
                _ => "other",
            };
            self.log.lock().unwrap().push(format!("pairing:{step}"));
        }

        fn devices_changed(&self, devices: &[Device]) {
            self.log
                .lock()
                .unwrap()
                .push(format!("devices:{}", devices.len()));
        }

        fn clipboard_changed(&self, status: &ClipboardStatus) {
            self.log
                .lock()
                .unwrap()
                .push(format!("clipboard:{}", status.activity.len()));
        }
    }

    #[test]
    fn backend_publishes_commands_immediately_and_cancels_cleanly() {
        let recorder = Arc::new(Recorder::default());
        let backend = FakeBackend::new(Script::Success, recorder.clone());
        assert_eq!(backend.pairing(), PairingState::Idle);

        backend.start_pairing(PairingMethod::QrCode).unwrap();
        assert!(matches!(backend.pairing(), PairingState::QrOffer { .. }));
        assert_eq!(
            backend.start_pairing(PairingMethod::Sas),
            Err(BackendError::WrongState)
        );
        backend.cancel_pairing();
        assert_eq!(backend.pairing(), PairingState::Idle);

        assert_eq!(
            *recorder.log.lock().unwrap(),
            ["pairing:qrOffer", "pairing:idle"]
        );
        drop(backend); // joins the timer thread
    }

    #[test]
    fn the_scripted_phones_share_the_clipboard_while_connected() {
        let recorder = Arc::new(Recorder::default());
        let backend = FakeBackend::new(Script::Demo, recorder.clone());
        let status = backend.clipboard();
        assert_eq!(status.mode, ClipboardMode::Automatic);
        let states: Vec<_> = status
            .phones
            .iter()
            .map(|phone| (phone.name.as_str(), phone.state))
            .collect();
        assert_eq!(
            states,
            [
                ("Pixel 9", ClipboardPhoneState::Ready),
                ("Old Galaxy", ClipboardPhoneState::Disconnected)
            ]
        );

        backend.send_clipboard().unwrap();
        let status = backend.clipboard();
        assert_eq!(status.activity.len(), 1);
        assert_eq!(status.activity[0].phone_name, "Pixel 9");
        assert!(status.problem.is_none());
        assert_eq!(*recorder.log.lock().unwrap(), ["clipboard:1"]);

        // Without a connected phone, nobody takes it.
        let alone = FakeBackend::new(Script::Success, Arc::new(Recorder::default()));
        alone.send_clipboard().unwrap();
        let problem = alone.clipboard().problem.unwrap();
        assert_eq!(problem.kind, ClipboardProblemKind::NoPhone);
    }

    #[test]
    fn the_connected_demo_phone_shows_photos_that_can_be_saved() {
        use link2linux_runtime::view::PhotosStatus;
        let backend = FakeBackend::new(Script::Demo, Arc::new(Recorder::default()));
        let listed = backend.photos("demo-1").unwrap();
        assert_eq!(listed.state, PhotosStatus::Listed);
        assert_eq!(
            backend.photos("demo-2").unwrap().state,
            PhotosStatus::Disconnected
        );
        assert_eq!(backend.photos("nobody"), Err(BackendError::NotFound));

        let id = &listed.photos[0].id;
        let thumbnail =
            tauri::async_runtime::block_on(backend.photo_thumbnail("demo-1", id, 256)).unwrap();
        assert!(thumbnail.starts_with(b"\x89PNG"));
        assert_eq!(
            backend.open_saved_photo("demo-1", id),
            Err(BackendError::NotFound)
        );
        let saved = tauri::async_runtime::block_on(backend.save_photo("demo-1", id)).unwrap();
        assert_eq!(saved.name, listed.photos[0].name);
        assert_eq!(saved.folder, "Pictures");
        assert_eq!(
            backend.photos("demo-1").unwrap().photos[0].saved.as_ref(),
            Some(&saved)
        );
        assert_eq!(backend.open_saved_photo("demo-1", id), Ok(()));
        assert_eq!(backend.show_saved_photo("demo-1", id), Ok(()));
    }
}
