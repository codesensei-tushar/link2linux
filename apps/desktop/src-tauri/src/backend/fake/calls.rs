//! Pretend calls for the fake backend. The demo's first phone is connected
//! for calls over Bluetooth and the second one waits for the user to pick
//! its device; the `incoming-call` script rings the first phone a few
//! seconds after start. Pairing over Bluetooth and outgoing calls go
//! through their steps on a timer.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use link2linux_runtime::calls::{are_tones, dialable};
use link2linux_runtime::view::{
    BluetoothChoice, BluetoothPairing, CallLink, CallProblem, CallStateView, CallView,
    CallsDesktop, CallsStatus, PairingCode, PairingRequestState, PhoneCalls,
};

use super::script::Script;
use crate::backend::{BackendError, Device, EventSink};

/// How long the pretend steps take.
#[derive(Debug, Clone, Copy)]
pub struct Delays {
    /// From the start to the scripted incoming call.
    pub ring: Duration,
    /// One step of pairing or of an outgoing call.
    pub step: Duration,
    /// How long pairing stays open.
    pub window: Duration,
}

impl Delays {
    /// What a person would see: the pairing window is as long as the real
    /// runtime's.
    pub fn real() -> Self {
        Self {
            ring: Duration::from_secs(4),
            step: Duration::from_millis(1_500),
            window: Duration::from_secs(120),
        }
    }
}

struct State {
    enabled: bool,
    /// The paired phones as last seen: ids and names, in order.
    phones: Vec<(String, String)>,
    /// Each phone's link; phones without one are not paired over
    /// Bluetooth.
    links: HashMap<String, CallLink>,
    calls: HashMap<String, Vec<CallView>>,
    pairing: Option<BluetoothPairing>,
    problems: HashMap<String, CallProblem>,
    /// Ends the pending steps of an earlier pairing.
    generation: u64,
    next_call: u32,
}

struct Inner {
    state: Mutex<State>,
    sink: Arc<dyn EventSink>,
    delays: Delays,
    desktop_name: String,
}

/// The phones' pretend calls.
pub struct FakeCalls {
    inner: Arc<Inner>,
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_millis()).ok())
        .unwrap_or(0)
}

fn choice(address: &str, name: &str) -> BluetoothChoice {
    BluetoothChoice {
        address: address.into(),
        name: name.into(),
    }
}

fn connected(device: BluetoothChoice) -> CallLink {
    CallLink::Connected {
        device,
        can_mute: true,
        muted: false,
    }
}

fn call(id: String, number: &str, name: &str, state: CallStateView) -> CallView {
    let ringing = matches!(state, CallStateView::Incoming | CallStateView::Waiting);
    CallView {
        id,
        number: number.into(),
        name: name.into(),
        state,
        app_name: None,
        audio_here: true,
        can_answer: ringing,
        can_decline: ringing,
        can_hang_up: !ringing,
        can_send_tones: state == CallStateView::Active,
        active_since_unix_ms: (state == CallStateView::Active).then(now_ms),
    }
}

impl Inner {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn status(&self, state: &State) -> CallsStatus {
        CallsStatus {
            desktop: if state.enabled {
                CallsDesktop::Ready
            } else {
                CallsDesktop::Disabled
            },
            enabled: state.enabled,
            pairing: state.pairing.clone(),
            phones: state
                .phones
                .iter()
                .map(|(id, name)| PhoneCalls {
                    device_id: id.clone(),
                    phone_name: name.clone(),
                    link: if state.enabled {
                        state.links.get(id).cloned().unwrap_or(CallLink::NotPaired)
                    } else {
                        CallLink::Off
                    },
                    calls: state.calls.get(id).cloned().unwrap_or_default(),
                    problem: state.problems.get(id).copied(),
                })
                .collect(),
        }
    }

    fn publish(&self, state: &State) {
        self.sink.calls_changed(&self.status(state));
    }

    /// Changes the state, and reports it.
    fn change<T>(&self, f: impl FnOnce(&mut State) -> T) -> T {
        let mut state = self.lock();
        let value = f(&mut state);
        self.publish(&state);
        value
    }
}

/// Runs `step` after `delay`, on a thread of its own.
fn after(inner: &Arc<Inner>, delay: Duration, step: impl FnOnce(&mut State) + Send + 'static) {
    let inner = inner.clone();
    let spawned = std::thread::Builder::new()
        .name("fake-calls".to_owned())
        .spawn(move || {
            std::thread::sleep(delay);
            inner.change(step);
        });
    if let Err(error) = spawned {
        tracing::debug!(%error, "the fake calls could not take a step");
    }
}

impl FakeCalls {
    pub fn new(
        script: Script,
        sink: Arc<dyn EventSink>,
        desktop_name: String,
        delays: Delays,
    ) -> Self {
        let mut links = HashMap::new();
        if script.has_demo_phones() {
            links.insert(
                "demo-1".to_owned(),
                connected(choice("0C:1A:2B:3C:4D:5E", "Pixel 9")),
            );
            links.insert(
                "demo-2".to_owned(),
                CallLink::Choose {
                    candidates: vec![
                        choice("00:11:22:33:44:55", "Galaxy S21"),
                        choice("66:77:88:99:AA:BB", "Work phone"),
                    ],
                },
            );
        }
        let inner = Arc::new(Inner {
            state: Mutex::new(State {
                enabled: true,
                phones: Vec::new(),
                links,
                calls: HashMap::new(),
                pairing: None,
                problems: HashMap::new(),
                generation: 0,
                next_call: 1,
            }),
            sink,
            delays,
            desktop_name,
        });
        if script == Script::IncomingCall {
            after(&inner, delays.ring, |state| {
                let id = format!("bt:/ag0/call{}", state.next_call);
                state.next_call += 1;
                state
                    .calls
                    .entry("demo-1".to_owned())
                    .or_default()
                    .push(call(
                        id,
                        "+49 151 2345678",
                        "Anna Müller",
                        CallStateView::Incoming,
                    ));
            });
        }
        Self { inner }
    }

    /// The page, for the paired phones `devices`.
    pub fn status(&self, devices: &[Device]) -> CallsStatus {
        let mut state = self.inner.lock();
        state.phones = devices
            .iter()
            .map(|d| (d.id.clone(), d.name.clone()))
            .collect();
        self.inner.status(&state)
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.inner.change(|state| {
            state.enabled = enabled;
            if !enabled {
                state.pairing = None;
                state.calls.clear();
            }
        });
    }

    pub fn start_pairing(&self, device_id: &str) -> Result<(), BackendError> {
        let generation = self.inner.change(|state| {
            if !state.enabled {
                return Err(BackendError::CallsUnavailable);
            }
            if !state.phones.iter().any(|(id, _)| id == device_id) {
                return Err(BackendError::NotFound);
            }
            state.generation += 1;
            state.problems.remove(device_id);
            let window = i64::try_from(self.inner.delays.window.as_millis()).unwrap_or(i64::MAX);
            state.pairing = Some(BluetoothPairing {
                device_id: device_id.to_owned(),
                adapter_name: self.inner.desktop_name.clone(),
                adapter_address: "A0:B1:C2:D3:E4:F5".into(),
                ends_unix_ms: now_ms().saturating_add(window),
                phone: PairingRequestState::Waiting,
                prompt: None,
            });
            Ok(state.generation)
        })?;
        let step = self.inner.delays.step;
        after(&self.inner, step, move |state| {
            if let Some(pairing) = state.pairing.as_mut()
                && state.generation == generation
            {
                pairing.phone = PairingRequestState::Asked;
            }
        });
        // The window runs out as the real one does.
        after(&self.inner, self.inner.delays.window, move |state| {
            if state.generation == generation
                && let Some(pairing) = state.pairing.take()
            {
                state
                    .problems
                    .insert(pairing.device_id, CallProblem::PairingTimedOut);
            }
        });
        let name = self
            .inner
            .lock()
            .phones
            .iter()
            .find(|(id, _)| id == device_id)
            .map(|(_, name)| name.clone())
            .unwrap_or_default();
        after(&self.inner, step * 2, move |state| {
            if let Some(pairing) = state.pairing.as_mut()
                && state.generation == generation
            {
                pairing.prompt = Some(PairingCode {
                    id: u32::try_from(generation).unwrap_or(u32::MAX),
                    device_name: name,
                    code: "482913".into(),
                });
            }
        });
        Ok(())
    }

    pub fn cancel_pairing(&self) {
        self.inner.change(|state| {
            state.generation += 1;
            state.pairing = None;
        });
    }

    pub fn confirm_code(&self, prompt: u32, matches: bool) -> Result<(), BackendError> {
        let (generation, device_id, name) = self.inner.change(|state| {
            let pairing = state.pairing.as_mut().ok_or(BackendError::WrongState)?;
            let code = pairing.prompt.take_if(|code| code.id == prompt);
            let code = code.ok_or(BackendError::WrongState)?;
            Ok((
                state.generation,
                pairing.device_id.clone(),
                code.device_name,
            ))
        })?;
        if matches {
            after(&self.inner, self.inner.delays.step, move |state| {
                if state.generation == generation {
                    state.pairing = None;
                    state
                        .links
                        .insert(device_id, connected(choice("0C:1A:2B:3C:4D:6F", &name)));
                }
            });
        }
        Ok(())
    }

    pub fn choose_device(&self, device_id: &str, address: &str) -> Result<(), BackendError> {
        self.inner.change(|state| {
            let Some(CallLink::Choose { candidates }) = state.links.get(device_id) else {
                return Err(BackendError::WrongState);
            };
            let device = candidates
                .iter()
                .find(|c| c.address == address)
                .cloned()
                .ok_or(BackendError::NotFound)?;
            state.links.insert(device_id.to_owned(), connected(device));
            Ok(())
        })
    }

    pub fn connect(&self, device_id: &str) -> Result<(), BackendError> {
        self.inner.change(|state| {
            let Some(CallLink::Disconnected { device }) = state.links.get(device_id) else {
                return Err(BackendError::WrongState);
            };
            let device = device.clone();
            state.links.insert(device_id.to_owned(), connected(device));
            Ok(())
        })
    }

    /// Changes call `call_id` of phone `device_id` with `f`, which drops
    /// the call when it returns false.
    fn with_call(
        &self,
        device_id: &str,
        call_id: &str,
        f: impl FnOnce(&mut CallView) -> Result<bool, BackendError>,
    ) -> Result<(), BackendError> {
        self.inner.change(|state| {
            let calls = state
                .calls
                .get_mut(device_id)
                .ok_or(BackendError::NotFound)?;
            let index = calls
                .iter()
                .position(|c| c.id == call_id)
                .ok_or(BackendError::NotFound)?;
            if !f(&mut calls[index])? {
                calls.remove(index);
            }
            Ok(())
        })
    }

    pub fn answer(&self, device_id: &str, call_id: &str) -> Result<(), BackendError> {
        self.with_call(device_id, call_id, |c| {
            if !c.can_answer {
                return Err(BackendError::WrongState);
            }
            *c = call(c.id.clone(), &c.number, &c.name, CallStateView::Active);
            Ok(true)
        })
    }

    pub fn end(&self, device_id: &str, call_id: &str) -> Result<(), BackendError> {
        self.with_call(device_id, call_id, |_| Ok(false))
    }

    fn is_connected(&self, device_id: &str) -> bool {
        let state = self.inner.lock();
        state.enabled && matches!(state.links.get(device_id), Some(CallLink::Connected { .. }))
    }

    pub fn dial(&self, device_id: &str, number: &str) -> Result<(), BackendError> {
        let number = dialable(number).ok_or(BackendError::NotDialable)?;
        if !self.is_connected(device_id) {
            return Err(BackendError::CallsNotConnected);
        }
        let id = self.inner.change(|state| {
            let id = format!("bt:/ag0/call{}", state.next_call);
            state.next_call += 1;
            state
                .calls
                .entry(device_id.to_owned())
                .or_default()
                .push(call(id.clone(), &number, "", CallStateView::Dialing));
            id
        });
        let step = self.inner.delays.step;
        for (delay, next) in [
            (step, CallStateView::Alerting),
            (step * 2, CallStateView::Active),
        ] {
            let (device_id, id) = (device_id.to_owned(), id.clone());
            after(&self.inner, delay, move |state| {
                if let Some(c) = state
                    .calls
                    .get_mut(&device_id)
                    .and_then(|calls| calls.iter_mut().find(|c| c.id == id))
                {
                    *c = call(c.id.clone(), &c.number, &c.name, next);
                }
            });
        }
        Ok(())
    }

    pub fn send_tones(&self, device_id: &str, tones: &str) -> Result<(), BackendError> {
        if !are_tones(tones) {
            return Err(BackendError::NotDialable);
        }
        if !self.is_connected(device_id) {
            return Err(BackendError::CallsNotConnected);
        }
        Ok(())
    }

    pub fn set_muted(&self, device_id: &str, muted: bool) -> Result<(), BackendError> {
        self.inner
            .change(|state| match state.links.get_mut(device_id) {
                Some(CallLink::Connected { muted: now, .. }) if state.enabled => {
                    *now = muted;
                    Ok(())
                }
                _ => Err(BackendError::CallsNotConnected),
            })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex as StdMutex;

    use link2linux_runtime::view::DeviceStatus;

    use super::*;

    #[derive(Default)]
    struct Recorder {
        calls: StdMutex<Vec<CallsStatus>>,
    }

    impl EventSink for Recorder {
        fn pairing_changed(&self, _state: &crate::backend::PairingState) {}
        fn devices_changed(&self, _devices: &[Device]) {}
        fn calls_changed(&self, status: &CallsStatus) {
            self.calls.lock().unwrap().push(status.clone());
        }
    }

    impl Recorder {
        /// Waits for a reported status that `check` accepts.
        fn wait_for(&self, check: impl Fn(&CallsStatus) -> bool) -> CallsStatus {
            for _ in 0..500 {
                if let Some(found) = self.calls.lock().unwrap().iter().rev().find(|s| check(s)) {
                    return found.clone();
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            panic!("no such calls status")
        }
    }

    fn instant() -> Delays {
        Delays {
            ring: Duration::ZERO,
            step: Duration::ZERO,
            window: Duration::from_secs(3600),
        }
    }

    fn devices() -> Vec<Device> {
        ["demo-1", "demo-2", "new-1"]
            .into_iter()
            .map(|id| Device {
                id: id.into(),
                name: id.into(),
                model: None,
                status: DeviceStatus::Connected,
                paired_at_unix_ms: 0,
                last_connected_unix_ms: None,
            })
            .collect()
    }

    #[test]
    fn the_incoming_call_script_rings_the_connected_phone() {
        let recorder = Arc::new(Recorder::default());
        let calls = FakeCalls::new(
            Script::IncomingCall,
            recorder.clone(),
            "Desk".into(),
            instant(),
        );
        // The call may ring before the page first asks, so this asks until
        // it shows.
        let mut status = calls.status(&devices());
        for _ in 0..500 {
            if !status.phones[0].calls.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
            status = calls.status(&devices());
        }
        let ringing = status.phones[0].calls[0].clone();
        assert_eq!(ringing.state, CallStateView::Incoming);
        assert!(matches!(status.phones[1].link, CallLink::Choose { .. }));
        assert_eq!(status.phones[2].link, CallLink::NotPaired);

        calls.answer("demo-1", &ringing.id).unwrap();
        let active = &calls.status(&devices()).phones[0].calls[0];
        assert_eq!(active.state, CallStateView::Active);
        assert!(active.active_since_unix_ms.is_some());
        calls.send_tones("demo-1", "1#").unwrap();
        calls.end("demo-1", &ringing.id).unwrap();
        assert!(calls.status(&devices()).phones[0].calls.is_empty());
    }

    #[test]
    fn pairing_asks_the_phone_shows_a_code_and_connects() {
        let recorder = Arc::new(Recorder::default());
        let calls = FakeCalls::new(Script::Demo, recorder.clone(), "Desk".into(), instant());
        calls.status(&devices());
        calls.start_pairing("new-1").unwrap();
        let status = recorder.wait_for(|s| s.pairing.as_ref().is_some_and(|p| p.prompt.is_some()));
        let pairing = status.pairing.unwrap();
        assert_eq!(pairing.adapter_name, "Desk");
        let prompt = pairing.prompt.unwrap();
        assert_eq!(prompt.code, "482913");
        assert_eq!(
            calls.confirm_code(prompt.id + 1, true),
            Err(BackendError::WrongState)
        );
        calls.confirm_code(prompt.id, true).unwrap();
        let status = recorder.wait_for(|s| s.pairing.is_none());
        assert!(matches!(status.phones[2].link, CallLink::Connected { .. }));
    }

    #[test]
    fn dialing_needs_a_connected_phone_and_a_number() {
        let recorder = Arc::new(Recorder::default());
        let calls = FakeCalls::new(Script::Demo, recorder.clone(), "Desk".into(), instant());
        calls.status(&devices());
        assert_eq!(
            calls.dial("demo-1", "call me"),
            Err(BackendError::NotDialable)
        );
        assert_eq!(
            calls.dial("demo-2", "123"),
            Err(BackendError::CallsNotConnected)
        );
        calls.dial("demo-1", "+1 555 0100").unwrap();
        let status = recorder.wait_for(|s| {
            s.phones[0]
                .calls
                .first()
                .is_some_and(|c| c.state == CallStateView::Active)
        });
        assert_eq!(status.phones[0].calls[0].number, "+15550100");
        calls.choose_device("demo-2", "66:77:88:99:AA:BB").unwrap();
        calls.set_muted("demo-2", true).unwrap();
        assert_eq!(
            calls.status(&devices()).phones[1].link,
            CallLink::Connected {
                device: choice("66:77:88:99:AA:BB", "Work phone"),
                can_mute: true,
                muted: true
            }
        );
        assert_eq!(
            calls.send_tones("demo-1", "1x"),
            Err(BackendError::NotDialable)
        );
        calls.set_enabled(false);
        let status = calls.status(&devices());
        assert_eq!(status.desktop, CallsDesktop::Disabled);
        assert_eq!(status.phones[0].link, CallLink::Off);
        assert_eq!(
            calls.start_pairing("demo-1"),
            Err(BackendError::CallsUnavailable)
        );
        assert_eq!(
            calls.set_muted("demo-1", true),
            Err(BackendError::CallsNotConnected)
        );
    }

    #[test]
    fn a_pairing_window_that_runs_out_says_so() {
        let recorder = Arc::new(Recorder::default());
        let delays = Delays {
            window: Duration::from_millis(50),
            ..instant()
        };
        let calls = FakeCalls::new(Script::Demo, recorder.clone(), "Desk".into(), delays);
        calls.status(&devices());
        calls.start_pairing("new-1").unwrap();
        let status = recorder.wait_for(|s| s.phones.get(2).is_some_and(|p| p.problem.is_some()));
        assert!(status.pairing.is_none());
        assert_eq!(status.phones[2].problem, Some(CallProblem::PairingTimedOut));
        // Opening it again clears the problem.
        calls.start_pairing("new-1").unwrap();
        assert_eq!(calls.status(&devices()).phones[2].problem, None);
    }
}
