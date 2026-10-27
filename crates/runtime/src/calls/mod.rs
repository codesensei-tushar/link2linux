//! The phones' calls through this desktop's Bluetooth.
//!
//! Every phone shares the desktop's adapter and its telephony service, so
//! [`Calls`] follows them once for all phones: [`follow`] reads them
//! whenever they change, feeds what they report into the [`Center`], and
//! carries out what it decides (keeping each phone's device, ending the
//! pairing window, connecting or disconnecting the hands-free link,
//! incoming-call pop-ups). Each phone's [`CallsDriver`] carries the
//! feature in that phone's sessions. The user's commands are the
//! functions here that take the [`Core`].

mod center;
mod choices;
mod driver;
pub mod dto;
mod port;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use link2linux_core::calls::{
    CallAction, CallNotification, Operation, PAIRING_WINDOW, PairingWindow, PhoneAnswer, operation,
};
use link2linux_core::notifications::Change;
use link2linux_core::session::Availability;
use link2linux_platform::{BluetoothAddress, NotificationResponse, PairingEvent, PlatformError};
use link2linux_protocol::v1::BluetoothPairingRequest;
use tokio::sync::Notify;
use tokio::time::Instant;

pub use driver::CallsDriver;
/// The rules for what can be dialed and sent as tones.
pub use link2linux_core::calls::{are_tones, dialable};
pub use port::{CallStack, CallsPort};

use center::{ANSWER_KEY, Center, DECLINE_KEY, Phone, Seen};
use dto::{CallProblem, CallsStatus};

use crate::link::{Command, lock, sleep_until};
use crate::shared::{Core, now_unix_ms};
use crate::view::BackendError;

/// How long to wait before watching a service again after watching it
/// failed, as when the system bus is gone.
const RETRY: Duration = Duration::from_secs(30);
/// How often the desktop connects a phone's hands-free link by itself, at
/// most, when the phone connects to the app without it.
const RECONNECT_EVERY: Duration = Duration::from_secs(10 * 60);
/// How long to wait before showing an incoming-call pop-up again after
/// the desktop refused it: a ringing call may change nothing else.
const POPUP_RETRY: Duration = Duration::from_secs(2);

/// What each phone's driver was last told: the desktop's availability,
/// whether the desktop shows the phone's calls, and whether a pairing
/// request waits for it.
type Told = (Availability, bool, bool);

/// The desktop's part in the phones' calls, shared by every phone.
pub struct Calls {
    port: Option<Arc<dyn CallsPort>>,
    center: Mutex<Center>,
    /// The paired phones, in pairing order. Take this lock before the
    /// center's.
    phones: Mutex<Vec<Phone>>,
    /// When the desktop last connected each phone's hands-free link by
    /// itself.
    reconnected: Mutex<HashMap<String, Instant>>,
    told: Mutex<HashMap<String, Told>>,
    /// Wakes [`follow`] to read the services again and report.
    nudge: Notify,
    /// Held while pairing opens or closes at the adapter, so that a close
    /// meant for one window never lands after a newer window opened.
    pairing: tokio::sync::Mutex<()>,
    choices_path: PathBuf,
}

impl Calls {
    /// Calls through `port`, or none without one, turned on when
    /// `enabled`, keeping each phone's device in `choices_path`.
    pub(crate) fn new(
        port: Option<Arc<dyn CallsPort>>,
        enabled: bool,
        choices_path: PathBuf,
    ) -> Self {
        let choices = choices::load(&choices_path);
        Self {
            center: Mutex::new(Center::new(port.is_some(), enabled, choices)),
            port,
            phones: Mutex::default(),
            reconnected: Mutex::default(),
            told: Mutex::default(),
            nudge: Notify::new(),
            pairing: tokio::sync::Mutex::new(()),
            choices_path,
        }
    }

    /// Closes pairing at the adapter for the window that opened as the
    /// `opened`-th, unless a newer one opened since: that one replaced it
    /// at the adapter, and stays open.
    async fn close_adapter(&self, port: &Arc<dyn CallsPort>, opened: u64) {
        let _pairing = self.pairing.lock().await;
        if lock(&self.center).windows_opened() != opened {
            return;
        }
        if let Err(error) = port.close_pairing().await {
            tracing::debug!(%error, "could not close Bluetooth pairing");
        }
    }

    fn phone(&self, device_id: &str) -> Option<(Phone, Vec<Phone>)> {
        let phones = lock(&self.phones).clone();
        let phone = phones.iter().find(|p| p.id == device_id)?.clone();
        Some((phone, phones))
    }

    /// What this desktop reports for calls in `FeatureStatus`.
    pub fn availability(&self) -> Availability {
        lock(&self.center).availability()
    }

    /// Whether the desktop shows phone `device_id`'s calls itself.
    pub fn shows_calls(&self, device_id: &str) -> bool {
        let Some((phone, phones)) = self.phone(device_id) else {
            return false;
        };
        lock(&self.center).shows_calls(&phone, &phones)
    }

    /// The pairing request phone `device_id` is to get now, if any.
    pub fn request_for(&self, device_id: &str) -> Option<BluetoothPairingRequest> {
        lock(&self.center).request_for(device_id)
    }

    /// What phone `device_id` made of the pairing request.
    pub fn phone_answered(&self, device_id: &str, answer: PhoneAnswer) {
        lock(&self.center).phone_answered(device_id, answer);
        self.nudge.notify_one();
    }

    /// Phone `device_id` reported whether calls are ready on it. While
    /// they are not, the desktop neither takes its calls nor keeps its
    /// hands-free link up; once its user turns them on again, the link is
    /// connected.
    pub fn phone_ready(&self, device_id: &str, ready: bool) {
        let mut center = lock(&self.center);
        if center.set_phone_ready(device_id, ready) {
            center.want_connect(device_id);
        }
        drop(center);
        self.nudge.notify_one();
    }

    /// Phone `device_id` connected with calls on: its hands-free link is
    /// connected too, unless that was tried lately.
    pub fn phone_connected(&self, device_id: &str) {
        let now = Instant::now();
        let mut reconnected = lock(&self.reconnected);
        if reconnected
            .get(device_id)
            .is_some_and(|at| now.duration_since(*at) < RECONNECT_EVERY)
        {
            return;
        }
        reconnected.insert(device_id.to_owned(), now);
        drop(reconnected);
        lock(&self.center).want_connect(device_id);
        self.nudge.notify_one();
    }

    /// A session with phone `device_id` started. Its call notifications
    /// count only while it has one: a call that ended while it was away
    /// would show until the next sync. Only the phone's current task
    /// reports this (see [`CallsDriver`]).
    pub fn session_started(&self, device_id: &str) {
        lock(&self.center).session_started(device_id);
        self.nudge.notify_one();
    }

    /// A session with phone `device_id` ended.
    pub fn session_ended(&self, device_id: &str) {
        lock(&self.center).session_ended(device_id);
        self.nudge.notify_one();
    }

    /// A change to phone `device_id`'s mirrored notifications.
    pub(crate) fn notification_changed(&self, device_id: &str, change: &Change) {
        let (key, note) = match change {
            Change::Posted { notification, .. } => (
                notification.key.as_str(),
                CallNotification::of(notification),
            ),
            Change::Removed { key, .. } => (key.as_str(), None),
        };
        let mut center = lock(&self.center);
        let was_call = center.has_note(device_id, key);
        if note.is_some() || was_call {
            center.set_note(device_id, key, note);
            drop(center);
            self.nudge.notify_one();
        }
    }

    /// The paired phones changed.
    pub(crate) fn set_phones(&self, phones: Vec<(String, String)>) {
        *lock(&self.phones) = phones
            .into_iter()
            .map(|(id, name)| Phone { id, name })
            .collect();
        self.nudge.notify_one();
    }

    fn save(&self, choices: &std::collections::BTreeMap<String, BluetoothAddress>) {
        if let Err(error) = choices::save(&self.choices_path, choices) {
            tracing::warn!(%error, "could not keep the phones' Bluetooth devices");
        }
    }

    fn deadline(&self) -> Option<Instant> {
        lock(&self.center).window().map(PairingWindow::deadline)
    }
}

/// The calls page's snapshot.
pub(crate) fn status(core: &Core) -> CallsStatus {
    let phones = lock(&core.calls.phones).clone();
    lock(&core.calls.center).status(&phones, Instant::now(), now_unix_ms())
}

/// Reports the calls page to the shell.
pub(crate) fn emit(core: &Core) {
    core.events.calls_changed(&status(core));
}

/// Follows the desktop's Bluetooth and telephony service until the
/// runtime stops, and keeps the calls page, the phones and the pop-ups in
/// step with them. It counts as one of [`Core::supervisors`] until pairing
/// is closed at the end.
pub(crate) async fn follow(core: Arc<Core>) {
    following(core.clone()).await;
    core.supervisors.fetch_sub(1, Ordering::SeqCst);
}

async fn following(core: Arc<Core>) {
    let calls = core.calls.clone();
    let Some(port) = calls.port.clone() else {
        tell(&core);
        emit(&core);
        return;
    };
    let mut bluetooth_retry: Option<Instant> = None;
    let mut telephony_retry: Option<Instant> = None;
    loop {
        refresh(&core, &port).await;
        let deadline = calls.deadline();
        let bluetooth = async {
            match bluetooth_retry {
                Some(at) => {
                    tokio::time::sleep_until(at).await;
                    Ok(())
                }
                None => port.bluetooth_changed().await,
            }
        };
        let telephony = async {
            match telephony_retry {
                Some(at) => {
                    tokio::time::sleep_until(at).await;
                    Ok(())
                }
                None => port.telephony_changed().await,
            }
        };
        tokio::select! {
            () = core.cancel.cancelled() => {
                stop_pairing(&core, &port).await;
                return;
            }
            changed = bluetooth => bluetooth_retry = retry_after(changed),
            changed = telephony => telephony_retry = retry_after(changed),
            event = port.next_pairing_event() => pairing_event(&core, &port, event),
            () = sleep_until(deadline) => {
                let _pairing = calls.pairing.lock().await;
                let waiting = lock(&calls.center).expire(Instant::now());
                if let Some(waiting) = waiting {
                    tracing::info!("the Bluetooth pairing window ran out");
                    refuse(&port, &waiting);
                    if let Err(error) = port.close_pairing().await {
                        tracing::debug!(%error, "could not close Bluetooth pairing");
                    }
                }
            }
            () = calls.nudge.notified() => {}
        }
    }
}

fn retry_after(changed: Result<(), PlatformError>) -> Option<Instant> {
    match changed {
        Ok(()) => None,
        Err(error) => {
            tracing::debug!(%error, "watching for changes failed; trying again later");
            Some(Instant::now() + RETRY)
        }
    }
}

/// Reads the services, takes in what they report, and acts on it.
async fn refresh(core: &Arc<Core>, port: &Arc<dyn CallsPort>) {
    let calls = &core.calls;
    let adapter = port
        .adapter()
        .await
        .inspect_err(|error| tracing::debug!(%error, "no Bluetooth adapter"))
        .ok()
        .flatten();
    let devices = if adapter.is_some() {
        port.devices().await.unwrap_or_default()
    } else {
        Vec::new()
    };
    let gateways = port
        .gateways()
        .await
        .inspect_err(|error| tracing::debug!(%error, "no hands-free call control"))
        .ok();
    let phones = lock(&calls.phones).clone();
    let (choices, paired, connect, disconnect) = {
        let mut center = lock(&calls.center);
        center.update(
            Seen {
                adapter,
                devices,
                gateways,
            },
            now_unix_ms(),
        );
        let settled = center.settle_choices(&phones);
        // With the window it closed, to close at the adapter once the
        // phone is trusted, and whether its link may come up now.
        let paired = center.pairing_done().map(|done| {
            let takes = center.takes_calls(&done.0);
            (done, center.windows_opened(), takes)
        });
        let choices = (settled || paired.is_some()).then(|| center.choices().clone());
        let connect = center.take_connects(&phones);
        // While calls are off, here or on the phone, a phone that connects
        // its hands-free link by itself is disconnected again, so it keeps
        // its calls.
        let disconnect = center.to_disconnect(&phones);
        (choices, paired, connect, disconnect)
    };
    if let Some(choices) = choices {
        calls.save(&choices);
    }
    if let Some(((device_id, address), opened, takes)) = paired {
        tracing::info!(%address, "the phone paired over Bluetooth");
        if takes {
            connect_link(core, port, device_id, address, Connect::Paired(opened));
        } else {
            // Calls went off meanwhile, here or on the phone: it stays
            // paired, and its link waits until they are on.
            let calls = calls.clone();
            let port = port.clone();
            core.handle.spawn(async move {
                calls.close_adapter(&port, opened).await;
            });
        }
    }
    for (device_id, address) in connect {
        connect_link(core, port, device_id, address, Connect::Auto);
    }
    for address in disconnect {
        tracing::info!(%address, "calls are off: disconnecting a phone's hands-free link");
        if let Err(error) = port.disconnect_hands_free(address).await {
            tracing::debug!(%error, "could not disconnect the hands-free link");
        }
    }
    popups(core).await;
    tell(core);
    emit(core);
}

/// Why the desktop connects a phone's hands-free link.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Connect {
    /// The phone just paired in the window that opened as the given one:
    /// pairing closes once the phone is trusted, so its own connection
    /// right after pairing is still let in.
    Paired(u64),
    /// The user asked to.
    User,
    /// The phone connected to the app. A failure is not reported: the
    /// phone may be out of Bluetooth range, and nobody asked.
    Auto,
}

/// Connects the hands-free link of phone `device_id`, without waiting.
fn connect_link(
    core: &Arc<Core>,
    port: &Arc<dyn CallsPort>,
    device_id: String,
    address: BluetoothAddress,
    why: Connect,
) {
    lock(&core.calls.center).set_connecting(&device_id, true);
    let core = core.clone();
    let port = port.clone();
    core.clone().handle.spawn(async move {
        let result = port.connect_hands_free(address).await;
        if let Connect::Paired(opened) = why {
            core.calls.close_adapter(&port, opened).await;
        }
        let mut center = lock(&core.calls.center);
        center.set_connecting(&device_id, false);
        match result {
            Ok(()) => center.set_problem(&device_id, None),
            Err(error) => {
                tracing::info!(%error, "could not connect the phone's hands-free link");
                if why != Connect::Auto {
                    center.set_problem(&device_id, Some(CallProblem::ConnectFailed));
                }
            }
        }
        drop(center);
        core.calls.nudge.notify_one();
    });
}

/// Tells each phone's task about a change to its calls.
fn tell(core: &Core) {
    let calls = &core.calls;
    let phones = lock(&calls.phones).clone();
    let now: Vec<(String, Told)> = {
        let center = lock(&calls.center);
        let availability = center.availability();
        phones
            .iter()
            .map(|phone| {
                (
                    phone.id.clone(),
                    (
                        availability.clone(),
                        center.shows_calls(phone, &phones),
                        center.request_for(&phone.id).is_some(),
                    ),
                )
            })
            .collect()
    };
    let changed: Vec<String> = {
        let mut told = lock(&calls.told);
        now.into_iter()
            .filter_map(|(id, state)| {
                (told.get(&id) != Some(&state)).then(|| {
                    told.insert(id.clone(), state);
                    id
                })
            })
            .collect()
    };
    let slots = lock(&core.slots);
    for id in changed {
        if let Some(slot) = slots.get(&id) {
            let _ = slot.commands.send(Command::CallsChanged);
        }
    }
}

/// Shows, updates and takes down the incoming-call pop-ups.
async fn popups(core: &Core) {
    let phones = lock(&core.calls.phones).clone();
    let plan = lock(&core.calls.center).plan_popups(&phones);
    for id in plan.close {
        if let Err(error) = core.presenter.close(id).await {
            tracing::debug!(%error, "could not take down a call pop-up");
        }
    }
    let mut failed = false;
    for planned in plan.show {
        // The caller shows only where the phone's notifications show
        // their text.
        let shown = core
            .presenter
            .show_own_with_text(
                &planned.popup,
                &planned.without_text,
                &planned.locked,
                planned.replaces,
            )
            .await;
        match shown {
            Ok(id) => lock(&core.calls.center).popup_shown(&planned.call, id),
            Err(error) => {
                tracing::warn!(%error, "could not show an incoming call");
                lock(&core.calls.center).popup_failed(&planned.call);
                failed = true;
            }
        }
    }
    if failed {
        let calls = core.calls.clone();
        core.handle.spawn(async move {
            tokio::time::sleep(POPUP_RETRY).await;
            calls.nudge.notify_one();
        });
    }
}

fn pairing_event(core: &Core, port: &Arc<dyn CallsPort>, event: PairingEvent) {
    let mut center = lock(&core.calls.center);
    match event {
        PairingEvent::Confirm(prompt) => {
            let refused = match center.window_mut() {
                Some(window) => window.prompted(prompt, Instant::now()),
                None => Some(prompt.id),
            };
            if let Some(id) = refused {
                port.answer_pairing(id, false);
            }
        }
        PairingEvent::Cancelled { id } => {
            if let Some(window) = center.window_mut() {
                window.cancelled(id);
            }
        }
    }
}

fn refuse(port: &Arc<dyn CallsPort>, prompts: &[u64]) {
    for id in prompts {
        port.answer_pairing(*id, false);
    }
}

/// Closes the pairing window, if one is open: refuses the prompts that
/// still wait, and closes pairing at the adapter.
async fn close_pairing(core: &Core, port: &Arc<dyn CallsPort>) {
    let _pairing = core.calls.pairing.lock().await;
    close_window(core, port).await;
}

/// The runtime stops: pairing closes at the adapter whether a window is
/// open or the phone that just paired is still connecting, which would
/// close it only afterwards.
async fn stop_pairing(core: &Core, port: &Arc<dyn CallsPort>) {
    let _pairing = core.calls.pairing.lock().await;
    let waiting = lock(&core.calls.center).close_window();
    refuse(port, &waiting);
    if let Err(error) = port.close_pairing().await {
        tracing::debug!(%error, "could not close Bluetooth pairing");
    }
}

/// As [`close_pairing`], for a caller that holds the pairing lock.
async fn close_window(core: &Core, port: &Arc<dyn CallsPort>) {
    let waiting = {
        let mut center = lock(&core.calls.center);
        center.window().is_some().then(|| center.close_window())
    };
    if let Some(waiting) = waiting {
        refuse(port, &waiting);
        if let Err(error) = port.close_pairing().await {
            tracing::debug!(%error, "could not close Bluetooth pairing");
        }
    }
}

fn port(core: &Core) -> Result<Arc<dyn CallsPort>, BackendError> {
    core.calls
        .port
        .clone()
        .ok_or(BackendError::CallsUnavailable)
}

fn phone_of(core: &Core, device_id: &str) -> Result<(Phone, Vec<Phone>), BackendError> {
    core.calls.phone(device_id).ok_or(BackendError::NotFound)
}

/// Why the desktop does not take phone `device_id`'s calls now, if it
/// does not.
fn takes_calls(center: &Center, device_id: &str) -> Result<(), BackendError> {
    if center.availability() != Availability::Ready {
        return Err(BackendError::CallsUnavailable);
    }
    if center.phone_off(device_id) {
        return Err(BackendError::CallsOffOnPhone);
    }
    Ok(())
}

/// Turns calls on this desktop on or off. Turned off, the phones'
/// hands-free links are disconnected and pairing closes; turned on, the
/// links are connected.
pub(crate) fn set_enabled(core: &Core, enabled: bool) {
    let calls = &core.calls;
    // Not under the center's lock: the phones' lock comes first.
    let phones = lock(&calls.phones).clone();
    let closed = {
        let mut center = lock(&calls.center);
        if center.is_enabled() == enabled {
            return;
        }
        center.set_enabled(enabled);
        if enabled {
            for phone in &phones {
                center.want_connect(&phone.id);
            }
            None
        } else {
            // Pairing serves calls only: it closes with them.
            take_window(&mut center, |_| true)
        }
    };
    if let Some(closed) = closed {
        close_taken(core, closed);
    }
    calls.nudge.notify_one();
}

/// Phone `device_id` is no longer paired: what the desktop kept for it
/// goes, and pairing open for it closes.
pub(crate) fn forget(core: &Core, device_id: &str) {
    let calls = &core.calls;
    let (choices, closed) = {
        let mut center = lock(&calls.center);
        let closed = take_window(&mut center, |window| window.device_id() == device_id);
        center.forget(device_id);
        (center.choices().clone(), closed)
    };
    if let Some(closed) = closed {
        close_taken(core, closed);
    }
    lock(&calls.told).remove(device_id);
    calls.save(&choices);
    calls.nudge.notify_one();
}

/// A window taken from the center: the prompts that still waited, and
/// its number.
type TakenWindow = (Vec<u64>, u64);

/// Takes the pairing window from `center` if one is open and `which`
/// picks it.
fn take_window(
    center: &mut Center,
    which: impl FnOnce(&PairingWindow) -> bool,
) -> Option<TakenWindow> {
    center
        .window()
        .is_some_and(which)
        .then(|| (center.close_window(), center.windows_opened()))
}

/// Ends a window taken from the center without waiting: its prompts are
/// refused now, and pairing closes at the adapter soon after, unless a
/// newer window opens first.
fn close_taken(core: &Core, (waiting, opened): TakenWindow) {
    let Some(port) = core.calls.port.clone() else {
        return;
    };
    refuse(&port, &waiting);
    let calls = core.calls.clone();
    core.handle.spawn(async move {
        calls.close_adapter(&port, opened).await;
    });
}

/// Opens Bluetooth pairing for phone `device_id` for
/// [`PAIRING_WINDOW`], and asks the phone to pair once it can.
pub(crate) fn start_pairing(core: &Arc<Core>, device_id: &str) -> Result<(), BackendError> {
    let port = port(core)?;
    phone_of(core, device_id)?;
    takes_calls(&lock(&core.calls.center), device_id)?;
    let core = core.clone();
    let device_id = device_id.to_owned();
    core.clone().handle.spawn(async move {
        // The window opens at the adapter and here as one step: a close
        // for an earlier window then sees that a newer one opened.
        let pairing = core.calls.pairing.lock().await;
        close_window(&core, &port).await;
        match port.open_pairing(PAIRING_WINDOW).await {
            Ok(adapter) => {
                let window = PairingWindow::open(device_id, adapter, Instant::now());
                lock(&core.calls.center).open_window(window);
            }
            Err(error) => {
                tracing::warn!(%error, "could not open Bluetooth pairing");
                lock(&core.calls.center).set_problem(&device_id, Some(CallProblem::PairingFailed));
            }
        }
        drop(pairing);
        core.calls.nudge.notify_one();
    });
    Ok(())
}

/// Closes Bluetooth pairing.
pub(crate) fn cancel_pairing(core: &Arc<Core>) {
    let Some(port) = core.calls.port.clone() else {
        return;
    };
    let core = core.clone();
    core.clone().handle.spawn(async move {
        close_pairing(&core, &port).await;
        core.calls.nudge.notify_one();
    });
}

/// The user answered pairing prompt `id`: whether the codes match.
pub(crate) fn confirm_code(core: &Core, id: u32, matches: bool) -> Result<(), BackendError> {
    let port = port(core)?;
    let id = u64::from(id);
    let accept = lock(&core.calls.center)
        .window_mut()
        .and_then(|window| window.answer(id, matches, Instant::now()))
        .ok_or(BackendError::WrongState)?;
    port.answer_pairing(id, accept);
    core.calls.nudge.notify_one();
    Ok(())
}

/// The user picked the paired device at `address` as phone `device_id`'s.
pub(crate) fn choose_device(
    core: &Core,
    device_id: &str,
    address: &str,
) -> Result<(), BackendError> {
    phone_of(core, device_id)?;
    let address: BluetoothAddress = address.parse().map_err(|_| BackendError::NotFound)?;
    let choices = {
        let mut center = lock(&core.calls.center);
        if !center.choose(device_id, address) {
            return Err(BackendError::NotFound);
        }
        center.want_connect(device_id);
        center.choices().clone()
    };
    core.calls.save(&choices);
    core.calls.nudge.notify_one();
    Ok(())
}

/// Connects phone `device_id`'s hands-free link.
pub(crate) fn connect(core: &Arc<Core>, device_id: &str) -> Result<(), BackendError> {
    let port = port(core)?;
    let (phone, _) = phone_of(core, device_id)?;
    let address = {
        let center = lock(&core.calls.center);
        takes_calls(&center, device_id)?;
        center
            .found(&phone)
            .ok_or(BackendError::CallsNotConnected)?
    };
    connect_link(core, &port, device_id.to_owned(), address, Connect::User);
    core.calls.nudge.notify_one();
    Ok(())
}

/// Does `action` on call `call_id` of phone `device_id`.
pub(crate) fn act(
    core: &Arc<Core>,
    device_id: &str,
    call_id: &str,
    action: CallAction,
) -> Result<(), BackendError> {
    let (phone, phones) = phone_of(core, device_id)?;
    let call = lock(&core.calls.center)
        .calls_of(&phone, &phones)
        .into_iter()
        .find(|call| call.id == call_id)
        .ok_or(BackendError::NotFound)?;
    match operation(&call, action).ok_or(BackendError::WrongState)? {
        Operation::RunAction { key, action_id } => {
            let slots = lock(&core.slots);
            let slot = slots.get(device_id).ok_or(BackendError::NotFound)?;
            let _ = slot.commands.send(Command::Action {
                key,
                action_id,
                reply: None,
            });
        }
        Operation::Answer { call } => {
            spawn_command(core, device_id, move |port| async move {
                port.answer(&call).await
            })?;
        }
        Operation::HangUp { call } => {
            spawn_command(core, device_id, move |port| async move {
                port.hang_up(&call).await
            })?;
        }
    }
    Ok(())
}

/// The gateway of phone `device_id`'s hands-free link, while it is up.
fn gateway_id(core: &Core, device_id: &str) -> Result<(String, Option<bool>), BackendError> {
    let (phone, phones) = phone_of(core, device_id)?;
    let center = lock(&core.calls.center);
    if center.phone_off(device_id) {
        return Err(BackendError::CallsOffOnPhone);
    }
    if !center.shows_calls(&phone, &phones) {
        return Err(BackendError::CallsNotConnected);
    }
    let gateway = center
        .gateway_of(&phone, &phones)
        .ok_or(BackendError::CallsNotConnected)?;
    Ok((gateway.id.clone(), gateway.muted))
}

/// Calls `number` from phone `device_id`, over its hands-free link.
pub(crate) fn dial(core: &Arc<Core>, device_id: &str, number: &str) -> Result<(), BackendError> {
    let number = dialable(number).ok_or(BackendError::NotDialable)?;
    let (gateway, _) = gateway_id(core, device_id)?;
    spawn_command(core, device_id, move |port| async move {
        port.dial(&gateway, &number).await
    })
}

/// Sends touch tones in phone `device_id`'s active call.
pub(crate) fn send_tones(
    core: &Arc<Core>,
    device_id: &str,
    tones: &str,
) -> Result<(), BackendError> {
    if !are_tones(tones) {
        return Err(BackendError::WrongState);
    }
    let tones = tones.to_owned();
    let (gateway, _) = gateway_id(core, device_id)?;
    spawn_command(core, device_id, move |port| async move {
        port.send_tones(&gateway, &tones).await
    })
}

/// Mutes or unmutes this desktop's microphone in phone `device_id`'s
/// calls.
pub(crate) fn set_muted(
    core: &Arc<Core>,
    device_id: &str,
    muted: bool,
) -> Result<(), BackendError> {
    let (gateway, can) = gateway_id(core, device_id)?;
    if can.is_none() {
        return Err(BackendError::WrongState);
    }
    spawn_command(core, device_id, move |port| async move {
        port.set_muted(&gateway, muted).await
    })
}

/// Runs a telephony command without waiting, and reports a failure on the
/// phone's calls.
fn spawn_command<F, R>(core: &Arc<Core>, device_id: &str, command: F) -> Result<(), BackendError>
where
    F: FnOnce(Arc<dyn CallsPort>) -> R + Send + 'static,
    R: Future<Output = Result<(), PlatformError>> + Send,
{
    let port = port(core)?;
    let core = core.clone();
    let device_id = device_id.to_owned();
    core.clone().handle.spawn(async move {
        let problem = match command(port).await {
            Ok(()) => None,
            Err(error) => {
                tracing::info!(%error, "a call command failed");
                Some(CallProblem::CommandFailed)
            }
        };
        lock(&core.calls.center).set_problem(&device_id, problem);
        core.calls.nudge.notify_one();
    });
    Ok(())
}

/// The user acted on a banner: answers or declines the call of an
/// incoming-call pop-up.
pub(crate) fn popup_response(core: &Arc<Core>, response: &NotificationResponse) {
    let NotificationResponse::Action { id, key } = response else {
        return;
    };
    let action = match key.as_str() {
        ANSWER_KEY => CallAction::Answer,
        DECLINE_KEY => CallAction::Decline,
        _ => return,
    };
    let Some((device_id, call)) = lock(&core.calls.center).popup_call(*id) else {
        return;
    };
    if let Err(error) = act(core, &device_id, &call, action) {
        tracing::debug!(%error, "could not act on the call pop-up");
    }
}
