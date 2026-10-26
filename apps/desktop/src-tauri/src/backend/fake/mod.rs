//! A scripted stand-in for the real pairing backend.
//!
//! [`Machine`] is a pure state machine: every method takes the current time
//! explicitly, so tests drive it with made-up clocks and never sleep.
//! [`FakeBackend`] wraps it with a real clock and one thread that sleeps until
//! the next deadline, so the UI sees the same events a real backend would
//! send. Choose a [`Script`] with the `L2L_FAKE_SCRIPT` environment variable
//! (`success`, `wrong-code`, `expired`, `not-found`, `declined`, `demo`).

pub mod machine;
pub mod script;
pub mod support;

use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::{Backend, BackendError, Device, EventSink, PairingMethod, PairingState, PhoneTarget};

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
    timer: Option<JoinHandle<()>>,
}

impl FakeBackend {
    pub fn new(script: Script, sink: Arc<dyn EventSink>) -> Self {
        let clock = Clock {
            start: Instant::now(),
        };
        let machine = Machine::new(script, desktop_name(), clock.now());
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
        Self { shared, timer }
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
}
