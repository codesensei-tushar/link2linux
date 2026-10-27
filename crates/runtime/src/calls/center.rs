//! The desktop's part in calls as plain state: what Bluetooth and the
//! telephony service report, the phones' choices of device, the pairing
//! window, the phones' call notifications, and the pop-ups shown. The task
//! in [`super`] feeds it and carries out what it decides, so the rules are
//! tested here without an OS.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::Duration;

use link2linux_core::calls::{
    CallAction, CallNotification, Chosen, PairingWindow, PhoneAnswer, PhoneDevice, ShownCall,
    find_phone, merge, operation,
};
use link2linux_core::session::Availability;
use link2linux_platform::{
    BluetoothAdapter, BluetoothAddress, BluetoothDevice, CallState, DesktopAction,
    DesktopNotification, Gateway, NotificationId, Urgency,
};
use link2linux_protocol::v1::ErrorCode;
use tokio::time::Instant;

use super::dto::{
    BluetoothChoice, BluetoothPairing, CallLink, CallProblem, CallStateView, CallView,
    CallsDesktop, CallsStatus, PairingCode, PairingRequestState, PhoneCalls,
};

/// The action keys of the incoming-call pop-up.
pub(crate) const ANSWER_KEY: &str = "calls.answer";
pub(crate) const DECLINE_KEY: &str = "calls.decline";

/// A paired phone, in pairing order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Phone {
    pub id: String,
    pub name: String,
}

/// What the desktop's services report.
#[derive(Debug, Clone, Default)]
pub(crate) struct Seen {
    /// The adapter, or `None` without one.
    pub adapter: Option<BluetoothAdapter>,
    pub devices: Vec<BluetoothDevice>,
    /// The gateways, or `None` while the telephony service is unavailable.
    pub gateways: Option<Vec<Gateway>>,
}

/// An incoming-call pop-up on screen, or about to be.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Popup {
    device_id: String,
    /// What it says, to update it when the caller's name arrives.
    summary: String,
    id: Option<NotificationId>,
}

/// The pop-ups to show and take down.
#[derive(Debug, Default)]
pub(crate) struct PopupPlan {
    pub show: Vec<PlannedPopup>,
    pub close: Vec<NotificationId>,
}

/// A call's incoming-call pop-up to show.
#[derive(Debug)]
pub(crate) struct PlannedPopup {
    pub call: String,
    /// What it says with the caller.
    pub popup: DesktopNotification,
    /// What it says while the user keeps the text of the phone's
    /// notifications off pop-ups: the caller is the phone's to tell.
    pub without_text: DesktopNotification,
    /// What it says while the screen is locked, which names nobody.
    pub locked: DesktopNotification,
    /// The pop-up on screen that it replaces.
    pub replaces: Option<NotificationId>,
}

/// The desktop's part in calls.
pub(crate) struct Center {
    /// Whether this build has calls for this OS.
    supported: bool,
    enabled: bool,
    /// Whether the services were read once.
    read: bool,
    seen: Seen,
    /// The Bluetooth device of each phone, kept across runs.
    choices: BTreeMap<String, BluetoothAddress>,
    /// The kept devices the user picked, or paired with a confirmed code,
    /// since the desktop started. Each is its phone's even before the
    /// Bluetooth stack trusts it, which connecting it does.
    confirmed: HashSet<BluetoothAddress>,
    window: Option<PairingWindow>,
    /// Each phone's call notifications, by key.
    notes: HashMap<String, BTreeMap<String, CallNotification>>,
    /// The phones with a session: their call notifications count only
    /// while they have one.
    sessions: HashSet<String>,
    /// Whether calls are ready on each phone, by what its latest session
    /// reported. A phone that has not said yet is missing.
    phone_ready: HashMap<String, bool>,
    /// When each shown call became active, for its timer.
    active_since: HashMap<String, i64>,
    problems: HashMap<String, CallProblem>,
    /// Phones whose hands-free link the desktop is connecting.
    connecting: HashSet<String>,
    /// Phones whose hands-free link is to be connected once calls can be
    /// used.
    want_connect: HashSet<String>,
    popups: HashMap<String, Popup>,
}

impl std::fmt::Debug for Center {
    // Pop-ups and notes name callers: never logged.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Center")
            .field("supported", &self.supported)
            .field("enabled", &self.enabled)
            .field("choices", &self.choices)
            .finish_non_exhaustive()
    }
}

impl Center {
    pub fn new(
        supported: bool,
        enabled: bool,
        choices: BTreeMap<String, BluetoothAddress>,
    ) -> Self {
        Self {
            supported,
            enabled,
            read: false,
            seen: Seen::default(),
            choices,
            confirmed: HashSet::new(),
            window: None,
            notes: HashMap::new(),
            sessions: HashSet::new(),
            phone_ready: HashMap::new(),
            active_since: HashMap::new(),
            problems: HashMap::new(),
            connecting: HashSet::new(),
            want_connect: HashSet::new(),
            popups: HashMap::new(),
        }
    }

    /// Takes in what the services report now, at `now_ms`.
    pub fn update(&mut self, seen: Seen, now_ms: i64) {
        self.seen = seen;
        self.read = true;
        self.track_active(now_ms);
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// Whether this desktop can take calls, and why not.
    pub fn desktop(&self) -> CallsDesktop {
        if !self.supported {
            return CallsDesktop::Unsupported;
        }
        if !self.read {
            return CallsDesktop::Checking;
        }
        match (&self.seen.adapter, &self.seen.gateways) {
            (None, _) => CallsDesktop::NoAdapter,
            (Some(adapter), _) if !adapter.powered => CallsDesktop::BluetoothOff,
            (_, None) => CallsDesktop::NoCallControl,
            _ if !self.enabled => CallsDesktop::Disabled,
            _ => CallsDesktop::Ready,
        }
    }

    /// What this desktop reports for calls in `FeatureStatus`.
    pub fn availability(&self) -> Availability {
        match self.desktop() {
            CallsDesktop::Ready => Availability::Ready,
            CallsDesktop::Disabled => Availability::Disabled,
            _ => Availability::Unavailable,
        }
    }

    /// The Bluetooth device of `phone`, as far as the desktop can tell.
    pub fn device_of(&self, phone: &Phone) -> PhoneDevice {
        let taken: Vec<BluetoothAddress> = self
            .choices
            .iter()
            .filter(|(id, _)| **id != phone.id)
            .map(|(_, address)| *address)
            .collect();
        find_phone(&self.seen.devices, &phone.name, self.chosen(phone), &taken)
    }

    /// The device kept for `phone`, if any.
    fn chosen(&self, phone: &Phone) -> Option<Chosen> {
        let address = *self.choices.get(&phone.id)?;
        Some(Chosen {
            address,
            confirmed: self.confirmed.contains(&address),
        })
    }

    /// Keeps each phone's device once it is found, so it stays the same
    /// when more devices are paired later. Only a trusted device named
    /// like its phone is found without the user (see [`find_phone`]).
    /// Returns whether a choice changed.
    pub fn settle_choices(&mut self, phones: &[Phone]) -> bool {
        let mut changed = false;
        for by_name in [true, false] {
            for phone in phones {
                let chosen = self.chosen(phone);
                let device = if by_name {
                    let named: Vec<BluetoothDevice> = self
                        .seen
                        .devices
                        .iter()
                        .filter(|d| {
                            d.name.trim().to_lowercase() == phone.name.trim().to_lowercase()
                        })
                        .cloned()
                        .collect();
                    let taken: Vec<BluetoothAddress> = self
                        .choices
                        .iter()
                        .filter(|(id, _)| **id != phone.id)
                        .map(|(_, a)| *a)
                        .collect();
                    find_phone(&named, &phone.name, chosen, &taken)
                } else {
                    self.device_of(phone)
                };
                if let PhoneDevice::Found(address) = device
                    && chosen.map(|c| c.address) != Some(address)
                {
                    self.choices.insert(phone.id.clone(), address);
                    changed = true;
                }
            }
        }
        changed
    }

    /// The kept device of each phone.
    pub fn choices(&self) -> &BTreeMap<String, BluetoothAddress> {
        &self.choices
    }

    /// The user picked `address` as `device_id`'s device, in the list or
    /// by confirming its pairing code. Returns whether it is a paired
    /// device that may be the phone.
    pub fn choose(&mut self, device_id: &str, address: BluetoothAddress) -> bool {
        let known = self
            .seen
            .devices
            .iter()
            .any(|d| d.address == address && d.paired);
        if known {
            self.choices
                .retain(|id, a| id == device_id || *a != address);
            if let Some(earlier) = self.choices.insert(device_id.to_owned(), address) {
                self.confirmed.remove(&earlier);
            }
            self.confirmed.insert(address);
            self.problems.remove(device_id);
        }
        known
    }

    /// `device_id`'s device, when it is found.
    pub fn found(&self, phone: &Phone) -> Option<BluetoothAddress> {
        match self.device_of(phone) {
            PhoneDevice::Found(address) => Some(address),
            _ => None,
        }
    }

    /// The hands-free gateway of `phone`, while its link is up. A service
    /// too old to say whose a gateway is still has its only gateway taken
    /// for the only phone whose device is known.
    pub fn gateway_of(&self, phone: &Phone, phones: &[Phone]) -> Option<&Gateway> {
        let gateways = self.seen.gateways.as_ref()?;
        let address = self.found(phone)?;
        if let Some(gateway) = gateways.iter().find(|g| g.address == Some(address)) {
            return Some(gateway);
        }
        let [only] = gateways.as_slice() else {
            return None;
        };
        let known = phones.iter().filter(|p| self.found(p).is_some()).count();
        (only.address.is_none() && known == 1).then_some(only)
    }

    /// Whether the desktop shows `phone`'s calls itself, with answer and
    /// decline over Bluetooth: calls are on, here and on the phone, and
    /// its hands-free link is up. The phone's call notifications then get
    /// no banners of their own.
    pub fn shows_calls(&self, phone: &Phone, phones: &[Phone]) -> bool {
        self.takes_calls(&phone.id) && self.gateway_of(phone, phones).is_some()
    }

    /// Whether the desktop takes phone `device_id`'s calls over Bluetooth:
    /// it can and the user wants it here, and the phone did not say it
    /// has calls off.
    pub fn takes_calls(&self, device_id: &str) -> bool {
        self.desktop() == CallsDesktop::Ready && !self.phone_off(device_id)
    }

    /// Phone `device_id` reported whether calls are ready on it. Returns
    /// whether they just came on after it had them off.
    pub fn set_phone_ready(&mut self, device_id: &str, ready: bool) -> bool {
        let before = self.phone_ready.insert(device_id.to_owned(), ready);
        ready && before == Some(false)
    }

    /// Whether phone `device_id` has calls turned off, or cannot take
    /// them, by what it reported last. A phone that has not said yet
    /// counts as on: its hands-free link may be up before the app connects.
    pub fn phone_off(&self, device_id: &str) -> bool {
        self.phone_ready.get(device_id) == Some(&false)
    }

    /// The devices whose hands-free link is up while calls are off, here
    /// or on their phone: they are disconnected, so the phone keeps its
    /// calls to itself.
    pub fn to_disconnect(&self, phones: &[Phone]) -> Vec<BluetoothAddress> {
        phones
            .iter()
            .filter(|p| !self.enabled || self.phone_off(&p.id))
            .filter(|p| self.gateway_of(p, phones).is_some())
            .filter_map(|p| self.found(p))
            .collect()
    }

    /// `phone`'s calls, merged from its hands-free link while the desktop
    /// takes them and its call notifications.
    pub fn calls_of(&self, phone: &Phone, phones: &[Phone]) -> Vec<ShownCall> {
        let gateway = if self.takes_calls(&phone.id) {
            self.gateway_of(phone, phones)
        } else {
            None
        };
        let notes: Vec<CallNotification> = self
            .notes
            .get(&phone.id)
            .filter(|_| self.sessions.contains(&phone.id))
            .map(|notes| notes.values().cloned().collect())
            .unwrap_or_default();
        merge(gateway, &notes)
    }

    /// The phone's call notification `note` was posted or changed; `None`
    /// when notification `key` is gone or no longer a call.
    pub fn set_note(&mut self, device_id: &str, key: &str, note: Option<CallNotification>) {
        let notes = self.notes.entry(device_id.to_owned()).or_default();
        match note {
            Some(note) => {
                notes.insert(key.to_owned(), note);
            }
            None => {
                notes.remove(key);
            }
        }
    }

    /// A session with phone `device_id` started.
    pub fn session_started(&mut self, device_id: &str) {
        self.sessions.insert(device_id.to_owned());
    }

    /// A session with phone `device_id` ended.
    pub fn session_ended(&mut self, device_id: &str) {
        self.sessions.remove(device_id);
    }

    /// Whether notification `key` of phone `device_id` is a call
    /// notification.
    pub fn has_note(&self, device_id: &str, key: &str) -> bool {
        self.notes
            .get(device_id)
            .is_some_and(|notes| notes.contains_key(key))
    }

    /// Phone `device_id`'s hands-free link is to be connected.
    pub fn want_connect(&mut self, device_id: &str) {
        self.want_connect.insert(device_id.to_owned());
    }

    /// The phones whose hands-free link to connect now, with their device:
    /// those that want it while calls can be used, whose device is known
    /// and whose link is down. Wishes wait while the services were not
    /// read yet, and while Bluetooth or the telephony service is away,
    /// since the phone connected to the app meanwhile asks only once. They
    /// are dropped while calls are off here, and for a phone that has
    /// them off.
    pub fn take_connects(&mut self, phones: &[Phone]) -> Vec<(String, BluetoothAddress)> {
        match self.desktop() {
            CallsDesktop::Ready => {}
            CallsDesktop::Disabled | CallsDesktop::Unsupported => {
                self.want_connect.clear();
                return Vec::new();
            }
            CallsDesktop::Checking
            | CallsDesktop::NoAdapter
            | CallsDesktop::BluetoothOff
            | CallsDesktop::NoCallControl => return Vec::new(),
        }
        let wanted = std::mem::take(&mut self.want_connect);
        phones
            .iter()
            .filter(|phone| wanted.contains(&phone.id))
            .filter(|phone| !self.phone_off(&phone.id))
            .filter(|phone| !self.connecting.contains(&phone.id))
            .filter(|phone| self.gateway_of(phone, phones).is_none())
            .filter_map(|phone| Some((phone.id.clone(), self.found(phone)?)))
            .collect()
    }

    pub fn set_problem(&mut self, device_id: &str, problem: Option<CallProblem>) {
        match problem {
            Some(problem) => self.problems.insert(device_id.to_owned(), problem),
            None => self.problems.remove(device_id),
        };
    }

    pub fn set_connecting(&mut self, device_id: &str, connecting: bool) {
        if connecting {
            self.connecting.insert(device_id.to_owned());
        } else {
            self.connecting.remove(device_id);
        }
    }

    /// Phone `device_id` is no longer paired.
    pub fn forget(&mut self, device_id: &str) {
        if let Some(address) = self.choices.remove(device_id) {
            self.confirmed.remove(&address);
        }
        self.notes.remove(device_id);
        self.sessions.remove(device_id);
        self.phone_ready.remove(device_id);
        self.problems.remove(device_id);
        self.connecting.remove(device_id);
        self.want_connect.remove(device_id);
        if self
            .window
            .as_ref()
            .is_some_and(|w| w.device_id() == device_id)
        {
            self.window = None;
        }
    }

    pub fn window(&self) -> Option<&PairingWindow> {
        self.window.as_ref()
    }

    pub fn window_mut(&mut self) -> Option<&mut PairingWindow> {
        self.window.as_mut()
    }

    pub fn open_window(&mut self, window: PairingWindow) {
        self.problems.remove(window.device_id());
        self.window = Some(window);
    }

    /// Closes the window. Returns the prompts that still waited.
    pub fn close_window(&mut self) -> Vec<u64> {
        self.window.take().map(|w| w.waiting()).unwrap_or_default()
    }

    /// The window's phone paired: its device is kept and the window
    /// closes. Returns the phone and its device, to connect.
    pub fn pairing_done(&mut self) -> Option<(String, BluetoothAddress)> {
        let window = self.window.as_ref()?;
        let address = window.paired(&self.seen.devices)?;
        let device_id = window.device_id().to_owned();
        self.window = None;
        self.choose(&device_id, address);
        Some((device_id, address))
    }

    /// The window ran out at `now` before the phone paired. Returns the
    /// prompts that still waited.
    pub fn expire(&mut self, now: Instant) -> Option<Vec<u64>> {
        let window = self.window.as_ref()?;
        if window.is_open(now) {
            return None;
        }
        self.problems
            .insert(window.device_id().to_owned(), CallProblem::PairingTimedOut);
        Some(self.close_window())
    }

    /// The pairing request to send to phone `device_id`, while the window
    /// is open for it and the phone was not asked yet.
    pub fn request_for(
        &self,
        device_id: &str,
    ) -> Option<link2linux_protocol::v1::BluetoothPairingRequest> {
        let window = self.window.as_ref()?;
        (window.device_id() == device_id
            && matches!(
                window.phone_answer(),
                PhoneAnswer::NotAsked | PhoneAnswer::NoAnswer
            ))
        .then(|| window.request())
    }

    /// What the phone made of the pairing request.
    pub fn phone_answered(&mut self, device_id: &str, answer: PhoneAnswer) {
        if let Some(window) = self.window.as_mut()
            && window.device_id() == device_id
        {
            window.set_phone_answer(answer);
        }
    }

    /// The snapshot for the calls page, at `now` (and `now_ms` on the
    /// wall clock).
    pub fn status(&self, phones: &[Phone], now: Instant, now_ms: i64) -> CallsStatus {
        let desktop = self.desktop();
        CallsStatus {
            desktop,
            enabled: self.enabled,
            pairing: self.window.as_ref().map(|w| pairing_view(w, now, now_ms)),
            phones: phones
                .iter()
                .map(|phone| PhoneCalls {
                    device_id: phone.id.clone(),
                    phone_name: phone.name.clone(),
                    link: self.link(phone, phones, desktop),
                    calls: self
                        .calls_of(phone, phones)
                        .iter()
                        .filter_map(|call| self.call_view(call))
                        .collect(),
                    problem: self.problems.get(&phone.id).copied(),
                })
                .collect(),
        }
    }

    fn link(&self, phone: &Phone, phones: &[Phone], desktop: CallsDesktop) -> CallLink {
        if desktop != CallsDesktop::Ready {
            return CallLink::Off;
        }
        if self.phone_off(&phone.id) {
            return CallLink::PhoneOff;
        }
        let choice = |address: BluetoothAddress| BluetoothChoice {
            address: address.to_string(),
            name: self
                .seen
                .devices
                .iter()
                .find(|d| d.address == address)
                .map(|d| d.name.clone())
                .unwrap_or_default(),
        };
        match self.device_of(phone) {
            PhoneDevice::NotPaired => CallLink::NotPaired,
            PhoneDevice::Choose(candidates) => CallLink::Choose {
                candidates: candidates.into_iter().map(choice).collect(),
            },
            PhoneDevice::Found(address) => match self.gateway_of(phone, phones) {
                Some(gateway) => CallLink::Connected {
                    device: choice(address),
                    can_mute: gateway.muted.is_some(),
                    muted: gateway.muted.unwrap_or(false),
                },
                None if self.connecting.contains(&phone.id) => CallLink::Connecting {
                    device: choice(address),
                },
                None => CallLink::Disconnected {
                    device: choice(address),
                },
            },
        }
    }

    fn call_view(&self, call: &ShownCall) -> Option<CallView> {
        let state = match call.state {
            CallState::Incoming => CallStateView::Incoming,
            CallState::Waiting => CallStateView::Waiting,
            CallState::Dialing => CallStateView::Dialing,
            CallState::Alerting => CallStateView::Alerting,
            CallState::Active => CallStateView::Active,
            CallState::Held => CallStateView::Held,
            CallState::Disconnected => return None,
        };
        let ringing = call.state.is_ringing();
        let bluetooth = matches!(
            call.control,
            link2linux_core::calls::Control::Bluetooth { .. }
        );
        Some(CallView {
            id: call.id.clone(),
            number: call.number.clone(),
            name: call.name.clone(),
            state,
            app_name: call.app_name.clone(),
            audio_here: bluetooth,
            can_answer: operation(call, CallAction::Answer).is_some(),
            can_decline: ringing && operation(call, CallAction::Decline).is_some(),
            can_hang_up: !ringing && operation(call, CallAction::HangUp).is_some(),
            can_send_tones: bluetooth && call.state == CallState::Active,
            active_since_unix_ms: self.active_since.get(&call.id).copied(),
        })
    }

    /// Notes when each call became active, and forgets calls that ended.
    fn track_active(&mut self, now_ms: i64) {
        let mut active: HashSet<String> = HashSet::new();
        for gateway in self.seen.gateways.iter().flatten() {
            for call in &gateway.calls {
                if call.state == CallState::Active {
                    active.insert(format!("bt:{}", call.id));
                }
            }
        }
        for note in self.notes.values().flat_map(BTreeMap::values) {
            if !note.is_ringing() {
                active.insert(format!("app:{}", note.key));
            }
        }
        self.active_since.retain(|id, _| active.contains(id));
        for id in active {
            self.active_since.entry(id).or_insert(now_ms);
        }
    }

    /// Which incoming-call pop-ups to show, update and take down: one for
    /// each ringing call of a phone whose calls the desktop shows itself.
    pub fn plan_popups(&mut self, phones: &[Phone]) -> PopupPlan {
        let mut wanted: HashMap<String, (String, PlannedPopup)> = HashMap::new();
        for phone in phones {
            if !self.shows_calls(phone, phones) {
                continue;
            }
            for call in self.calls_of(phone, phones) {
                if call.state.is_ringing() {
                    let planned = incoming_popup(&call, &phone.name);
                    wanted.insert(call.id.clone(), (phone.id.clone(), planned));
                }
            }
        }
        let mut plan = PopupPlan::default();
        self.popups.retain(|call, popup| {
            let keep = wanted.contains_key(call);
            if !keep && let Some(id) = popup.id {
                plan.close.push(id);
            }
            keep
        });
        for (call, (device_id, mut planned)) in wanted {
            let shown = self.popups.get(&call);
            if shown.is_some_and(|shown| shown.summary == planned.popup.summary) {
                continue;
            }
            planned.replaces = shown.and_then(|shown| shown.id);
            self.popups.insert(
                call,
                Popup {
                    device_id,
                    summary: planned.popup.summary.clone(),
                    id: planned.replaces,
                },
            );
            plan.show.push(planned);
        }
        plan
    }

    /// Pop-up `id` is on screen for call `call`.
    pub fn popup_shown(&mut self, call: &str, id: NotificationId) {
        if let Some(popup) = self.popups.get_mut(call) {
            popup.id = Some(id);
        }
    }

    /// The pop-up of call `call` could not be shown: the next plan shows
    /// it again, replacing the one on screen, if any.
    pub fn popup_failed(&mut self, call: &str) {
        if let Some(popup) = self.popups.get_mut(call) {
            popup.summary.clear();
        }
    }

    /// The phone and call of pop-up `id`.
    pub fn popup_call(&self, id: NotificationId) -> Option<(String, String)> {
        self.popups
            .iter()
            .find(|(_, popup)| popup.id == Some(id))
            .map(|(call, popup)| (popup.device_id.clone(), call.clone()))
    }
}

fn pairing_view(window: &PairingWindow, now: Instant, now_ms: i64) -> BluetoothPairing {
    let left = window.deadline().saturating_duration_since(now);
    let request = window.request();
    BluetoothPairing {
        device_id: window.device_id().to_owned(),
        adapter_name: request.name,
        adapter_address: request.address,
        ends_unix_ms: now_ms.saturating_add(millis(left)),
        phone: match window.phone_answer() {
            PhoneAnswer::NotAsked => PairingRequestState::NotAsked,
            PhoneAnswer::Waiting => PairingRequestState::Waiting,
            PhoneAnswer::Asked => PairingRequestState::Asked,
            PhoneAnswer::Refused(
                ErrorCode::Unavailable | ErrorCode::Disabled | ErrorCode::Unsupported,
            ) => PairingRequestState::Unavailable,
            PhoneAnswer::Refused(ErrorCode::NotAllowed) => PairingRequestState::NotAllowed,
            PhoneAnswer::Refused(_) | PhoneAnswer::NoAnswer => PairingRequestState::Failed,
        },
        prompt: window.prompt().map(|prompt| PairingCode {
            id: u32::try_from(prompt.id).unwrap_or(u32::MAX),
            device_name: prompt.device_name.clone(),
            code: format!("{:06}", prompt.code % 1_000_000),
        }),
    }
}

fn millis(duration: Duration) -> i64 {
    i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
}

/// The pop-up for the ringing call `call` of phone `phone_name`, in its
/// forms with the caller, without, and for a locked screen. Its
/// replacement is left for the plan to fill in.
fn incoming_popup(call: &ShownCall, phone_name: &str) -> PlannedPopup {
    let caller = [&call.name, &call.number]
        .into_iter()
        .find(|s| !s.trim().is_empty())
        .map_or("Unknown caller", |s| s.as_str());
    let bluetooth = matches!(
        call.control,
        link2linux_core::calls::Control::Bluetooth { .. }
    );
    let (what, incoming) = match &call.app_name {
        Some(app) if !bluetooth => (format!("{app} call"), format!("Incoming {app} call")),
        _ => ("Call".to_owned(), "Incoming call".to_owned()),
    };
    let sound = if bluetooth {
        "Answer to talk on this computer."
    } else {
        "The call's sound stays on the phone."
    };
    let actions = vec![
        DesktopAction {
            key: ANSWER_KEY.into(),
            label: "Answer".into(),
        },
        DesktopAction {
            key: DECLINE_KEY.into(),
            label: "Decline".into(),
        },
    ];
    let locked = DesktopNotification {
        app_name: "link2linux".into(),
        summary: "Incoming call".into(),
        body: "Your phone is ringing.".into(),
        actions: actions.clone(),
        urgency: Urgency::Critical,
        category: Some("call.incoming".into()),
        ..DesktopNotification::default()
    };
    let popup = DesktopNotification {
        summary: format!("{what} from {caller}"),
        body: format!("On {phone_name}. {sound}"),
        ..locked.clone()
    };
    let without_text = DesktopNotification {
        summary: format!("{incoming} on {phone_name}"),
        body: sound.to_owned(),
        ..locked.clone()
    };
    PlannedPopup {
        call: call.id.clone(),
        popup,
        without_text,
        locked,
        replaces: None,
    }
}

#[cfg(test)]
mod tests;
