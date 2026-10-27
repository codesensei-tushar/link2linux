//! The phone's status: its battery, signal and sound mode, which the
//! desktop shows (`docs/protocol.md`, "Phone status").
//!
//! The status lives on the phone. [`StatusService`] asks for it whenever the
//! feature becomes ready on both devices, which also subscribes the desktop
//! to its changes, and holds the newest one until the session ends or the
//! feature leaves `READY`. Each status the phone sends is complete and
//! replaces the one before, so a lost change is repaired by the next.
//!
//! The status shows nothing that gives away where the phone is, but it is
//! still the user's: it never reaches a log.

use link2linux_protocol::Payload;
use link2linux_protocol::v1::{self as wire, ErrorCode, ErrorReply, PhoneStatusRequest};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::time::{Duration, Instant};

use crate::Feature;
use crate::files::shown_name;
use crate::session::{Availability, Session, SessionError, SessionEvent};

/// The longest operator name the phone sends, in bytes. A longer one is
/// shown as unknown.
pub const MAX_OPERATOR_NAME_BYTES: usize = 64;

/// The highest signal level: full bars.
pub const MAX_LEVEL: u8 = 4;

/// How long the desktop waits before it asks again after a request that
/// timed out or failed in a way that may pass: for the first retry, the
/// second, and every one after.
const RETRY_AFTER: [Duration; 3] = [
    Duration::from_secs(5),
    Duration::from_secs(30),
    Duration::from_secs(300),
];

/// The phone's status, checked against the protocol's ranges. A part the
/// phone left unset or sent out of range is `None`: unknown, and shown as
/// nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PhoneStatus {
    /// The battery.
    pub battery: Option<Battery>,
    /// The cellular connection the phone's status bar shows first. `None`
    /// on a phone without telephony.
    pub cellular: Option<Cellular>,
    /// Set while the phone is connected to Wi-Fi.
    pub wifi: Option<Wifi>,
    /// The ringer mode.
    pub ringer: Option<RingerMode>,
    /// Do Not Disturb is on.
    pub do_not_disturb: bool,
}

/// The phone's battery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Battery {
    /// The charge, 0 to 100.
    pub percent: Option<u8>,
    /// The phone is connected to power, also once the battery is full.
    pub plugged_in: bool,
    /// Battery saver is on.
    pub power_saver: bool,
}

/// The phone's cellular connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cellular {
    /// The signal level, 0 (none) to [`MAX_LEVEL`] (full).
    pub level: Option<u8>,
    /// The generation the phone's status bar shows.
    pub network: Option<CellularNetwork>,
    /// The carrier name the phone shows, fit to be shown here.
    pub operator_name: Option<String>,
}

/// The phone's Wi-Fi connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Wifi {
    /// The signal level, 0 (none) to [`MAX_LEVEL`] (full).
    pub level: Option<u8>,
}

/// The cellular generation the phone's status bar shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellularNetwork {
    /// No SIM, no service, or emergency calls only.
    NoService,
    /// 2G.
    TwoG,
    /// 3G.
    ThreeG,
    /// 4G.
    FourG,
    /// 5G, also when anchored on 4G.
    FiveG,
}

/// Whether calls and notifications make sound on the phone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RingerMode {
    /// They make sound.
    Normal,
    /// They vibrate.
    Vibrate,
    /// They are silent.
    Silent,
}

impl PhoneStatus {
    /// The phone's status as it arrived, with each value out of range read
    /// as unknown.
    #[must_use]
    pub fn from_wire(status: &wire::PhoneStatus) -> Self {
        Self {
            battery: status.battery.as_ref().map(|battery| Battery {
                percent: u8::try_from(battery.percent)
                    .ok()
                    .filter(|percent| *percent <= 100),
                plugged_in: battery.plugged_in,
                power_saver: battery.power_saver,
            }),
            cellular: status.cellular.as_ref().map(|cellular| Cellular {
                level: level(cellular.level),
                network: CellularNetwork::from_wire(cellular.network()),
                operator_name: operator_name(&cellular.operator_name),
            }),
            wifi: status.wifi.as_ref().map(|wifi| Wifi {
                level: level(wifi.level),
            }),
            ringer: RingerMode::from_wire(status.ringer_mode()),
            do_not_disturb: status.do_not_disturb,
        }
    }
}

impl CellularNetwork {
    fn from_wire(network: wire::CellularNetwork) -> Option<Self> {
        match network {
            wire::CellularNetwork::Unspecified => None,
            wire::CellularNetwork::NoService => Some(Self::NoService),
            wire::CellularNetwork::CellularNetwork2g => Some(Self::TwoG),
            wire::CellularNetwork::CellularNetwork3g => Some(Self::ThreeG),
            wire::CellularNetwork::CellularNetwork4g => Some(Self::FourG),
            wire::CellularNetwork::CellularNetwork5g => Some(Self::FiveG),
        }
    }
}

impl RingerMode {
    fn from_wire(mode: wire::RingerMode) -> Option<Self> {
        match mode {
            wire::RingerMode::Unspecified => None,
            wire::RingerMode::Normal => Some(Self::Normal),
            wire::RingerMode::Vibrate => Some(Self::Vibrate),
            wire::RingerMode::Silent => Some(Self::Silent),
        }
    }
}

/// A signal level, or `None` above [`MAX_LEVEL`].
fn level(level: u32) -> Option<u8> {
    u8::try_from(level).ok().filter(|level| *level <= MAX_LEVEL)
}

/// The operator name as it can be shown, or `None` when it is empty or
/// longer than the protocol allows. Marks that reorder text are replaced,
/// so a name cannot turn the text around it.
fn operator_name(name: &str) -> Option<String> {
    if name.is_empty() || name.len() > MAX_OPERATOR_NAME_BYTES {
        return None;
    }
    Some(shown_name(name))
}

/// What the desktop can show of the phone's status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatusState {
    /// The phone's current status.
    Current(PhoneStatus),
    /// The desktop asked for the status, or waits for the phone to say
    /// whether it shares it.
    Waiting,
    /// The user hid the phone's status on this desktop.
    Hidden,
    /// The user turned the status off on the phone.
    TurnedOff,
    /// The phone does not share its status right now, or did not send it.
    Unavailable,
    /// The phone's app does not share its status: it is older than this
    /// desktop's.
    Unsupported,
    /// No session with the phone is up.
    Disconnected,
}

/// The status feature with one phone, driven by its session. It does not
/// own the session, so it can share one with other features:
///
/// 1. [`StatusService::session_started`] once the session is up;
/// 2. every [`SessionEvent`] through [`StatusService::handle_event`], which
///    keeps the status events and hands back the rest;
/// 3. [`StatusService::set_shown`] when the user shows or hides the phone's
///    status on this desktop;
/// 4. [`StatusService::session_ended`] when the session is gone.
///
/// [`StatusService::state`] says what to show after each of them.
#[derive(Debug)]
pub struct StatusService {
    state: StatusState,
    /// Whether the feature was ready on both devices at the last look.
    ready: bool,
    /// The `PhoneStatusRequest` that waits for its answer.
    asked: Option<u64>,
    /// A request was sent while the feature has been ready, answered or
    /// not: the phone counts the desktop as subscribed, so its changes
    /// apply until the feature leaves `READY` or the session ends.
    following: bool,
    /// When to ask again after a request that got no usable answer.
    retry_at: Option<Instant>,
    /// The requests in a row that got no usable answer.
    failures: usize,
}

impl Default for StatusService {
    fn default() -> Self {
        Self::new()
    }
}

impl StatusService {
    /// A service with no session yet.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: StatusState::Disconnected,
            ready: false,
            asked: None,
            following: false,
            retry_at: None,
            failures: 0,
        }
    }

    /// What to show of the phone's status now.
    #[must_use]
    pub fn state(&self) -> &StatusState {
        &self.state
    }

    /// Asks for the status if the feature is ready on both devices. Call it
    /// once the session is up.
    ///
    /// # Errors
    ///
    /// Fails when the session fails.
    pub async fn session_started<S>(&mut self, session: &mut Session<S>) -> Result<(), SessionError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        self.ready = false;
        self.look(session).await
    }

    /// Forgets the status: it is no longer current once the session is
    /// gone.
    pub fn session_ended(&mut self) {
        self.ready = false;
        self.asked = None;
        self.following = false;
        self.retry_at = None;
        self.failures = 0;
        self.state = StatusState::Disconnected;
    }

    /// Takes the events of the status feature and returns every other
    /// event, for the caller to handle. A `FeatureStatus` is used and
    /// returned too.
    ///
    /// # Errors
    ///
    /// Fails when the session fails.
    pub async fn handle_event<S>(
        &mut self,
        session: &mut Session<S>,
        event: SessionEvent,
    ) -> Result<Option<SessionEvent>, SessionError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        match event {
            SessionEvent::Reply(reply) if self.asked == Some(reply.request) => {
                self.asked = None;
                self.answered(reply.into_result());
                Ok(None)
            }
            SessionEvent::RequestTimedOut { request } if self.asked == Some(request) => {
                self.asked = None;
                tracing::info!("the phone did not send its status in time");
                self.state = StatusState::Unavailable;
                self.retry_later(0);
                Ok(None)
            }
            SessionEvent::Message {
                payload: Payload::PhoneStatusChanged(changed),
                ..
            } => {
                if self.following {
                    let status = changed.status.unwrap_or_default();
                    self.state = StatusState::Current(PhoneStatus::from_wire(&status));
                    // The subscription works after all.
                    self.retry_at = None;
                    self.failures = 0;
                    tracing::trace!("the phone's status changed");
                } else {
                    tracing::debug!("ignoring a status change the desktop did not ask for");
                }
                Ok(None)
            }
            SessionEvent::FeatureStatus(states) => {
                self.look(session).await?;
                Ok(Some(SessionEvent::FeatureStatus(states)))
            }
            other => Ok(Some(other)),
        }
    }

    /// The user showed or hid the phone's status on this desktop: tells the
    /// phone with a new `FeatureStatus`, and asks for the status again once
    /// it is shown.
    ///
    /// # Errors
    ///
    /// Fails when the session fails.
    pub async fn set_shown<S>(
        &mut self,
        session: &mut Session<S>,
        shown: bool,
    ) -> Result<(), SessionError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let availability = if shown {
            Availability::Ready
        } else {
            Availability::Disabled
        };
        session
            .set_availability(Feature::Status, availability)
            .await?;
        self.look(session).await
    }

    /// Asks for the status when the feature just became ready on both
    /// devices, and stops following it when it no longer is.
    async fn look<S>(&mut self, session: &mut Session<S>) -> Result<(), SessionError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let ready = session.is_ready(Feature::Status);
        if !ready {
            if self.ready {
                tracing::debug!("the phone's status is no longer followed");
            }
            self.ready = false;
            // A late answer to the request that waits is not taken.
            self.asked = None;
            self.following = false;
            self.retry_at = None;
            self.failures = 0;
            self.state = not_ready(session);
            return Ok(());
        }
        if !self.ready {
            self.failures = 0;
            self.retry_at = None;
            self.ask(session).await?;
            self.ready = true;
            self.state = StatusState::Waiting;
        }
        Ok(())
    }

    async fn ask<S>(&mut self, session: &mut Session<S>) -> Result<(), SessionError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let request = Payload::PhoneStatusRequest(PhoneStatusRequest {});
        let seq = session.start_request(request).await?;
        tracing::debug!(seq, "asked for the phone's status");
        self.asked = Some(seq);
        self.following = true;
        Ok(())
    }

    /// Plans the next request after one that got no usable answer: in
    /// `retry_after_ms` when the phone said, else after a backoff.
    fn retry_later(&mut self, retry_after_ms: u32) {
        let backoff = RETRY_AFTER[self.failures.min(RETRY_AFTER.len() - 1)];
        let wait = match retry_after_ms {
            0 => backoff,
            ms => Duration::from_millis(ms.into()),
        };
        self.failures += 1;
        self.retry_at = Some(Instant::now() + wait);
    }

    /// When the desktop asks for the status again by itself.
    #[must_use]
    pub fn next_deadline(&self) -> Option<Instant> {
        self.retry_at
    }

    /// The time [`StatusService::next_deadline`] named may have come: asks
    /// again, unless the feature is no longer ready.
    ///
    /// # Errors
    ///
    /// Fails when the session fails.
    pub async fn tick<S>(
        &mut self,
        session: &mut Session<S>,
        now: Instant,
    ) -> Result<(), SessionError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        if !self.retry_at.is_some_and(|at| at <= now) {
            return Ok(());
        }
        self.retry_at = None;
        if self.ready && self.asked.is_none() && session.is_ready(Feature::Status) {
            self.ask(session).await?;
        }
        Ok(())
    }

    /// Notes the answer to the request.
    fn answered(&mut self, answer: Result<Payload, ErrorReply>) {
        self.state = match answer {
            Ok(Payload::PhoneStatusReply(reply)) => {
                self.following = true;
                self.retry_at = None;
                self.failures = 0;
                let status = reply.status.unwrap_or_default();
                StatusState::Current(PhoneStatus::from_wire(&status))
            }
            Ok(_) => {
                tracing::debug!("the phone answered the status request with something else");
                self.retry_later(0);
                StatusState::Unavailable
            }
            Err(error) => {
                tracing::info!(code = error.code, "the phone did not send its status");
                match error.code() {
                    ErrorCode::Disabled => StatusState::TurnedOff,
                    ErrorCode::Unsupported => StatusState::Unsupported,
                    ErrorCode::Unavailable | ErrorCode::Busy | ErrorCode::Internal => {
                        self.retry_later(error.retry_after_ms);
                        StatusState::Unavailable
                    }
                    _ => StatusState::Unavailable,
                }
            }
        };
    }
}

/// Why the status cannot be followed in `session`, where the feature is not
/// ready on both devices.
fn not_ready<S>(session: &Session<S>) -> StatusState
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    if session.availability().get(&Feature::Status) == Some(&Availability::Disabled) {
        return StatusState::Hidden;
    }
    if !session.shared_features().contains(&Feature::Status) {
        return StatusState::Unsupported;
    }
    let Some(states) = session.peer_availability() else {
        return StatusState::Waiting;
    };
    match states.get(&Feature::Status) {
        Some(Availability::Disabled) => StatusState::TurnedOff,
        _ => StatusState::Unavailable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wire_status() -> wire::PhoneStatus {
        wire::PhoneStatus {
            battery: Some(wire::Battery {
                percent: 81,
                plugged_in: true,
                power_saver: false,
            }),
            cellular: Some(wire::Cellular {
                level: 3,
                network: wire::CellularNetwork::CellularNetwork5g.into(),
                operator_name: "Carrier".into(),
            }),
            wifi: Some(wire::Wifi { level: 4 }),
            ringer_mode: wire::RingerMode::Vibrate.into(),
            do_not_disturb: true,
        }
    }

    #[test]
    fn a_status_in_range_reads_as_sent() {
        let status = PhoneStatus::from_wire(&wire_status());
        assert_eq!(
            status,
            PhoneStatus {
                battery: Some(Battery {
                    percent: Some(81),
                    plugged_in: true,
                    power_saver: false,
                }),
                cellular: Some(Cellular {
                    level: Some(3),
                    network: Some(CellularNetwork::FiveG),
                    operator_name: Some("Carrier".into()),
                }),
                wifi: Some(Wifi { level: Some(4) }),
                ringer: Some(RingerMode::Vibrate),
                do_not_disturb: true,
            }
        );
    }

    #[test]
    fn values_out_of_range_read_as_unknown() {
        let mut sent = wire_status();
        sent.battery.as_mut().unwrap().percent = 101;
        let cellular = sent.cellular.as_mut().unwrap();
        cellular.level = 5;
        cellular.network = 99;
        cellular.operator_name = "a".repeat(MAX_OPERATOR_NAME_BYTES + 1);
        sent.wifi.as_mut().unwrap().level = u32::MAX;
        sent.ringer_mode = 42;
        let status = PhoneStatus::from_wire(&sent);

        let battery = status.battery.unwrap();
        assert_eq!(battery.percent, None);
        assert!(battery.plugged_in);
        assert_eq!(
            status.cellular,
            Some(Cellular {
                level: None,
                network: None,
                operator_name: None,
            })
        );
        // Connected to Wi-Fi, at a level nobody knows.
        assert_eq!(status.wifi, Some(Wifi { level: None }));
        assert_eq!(status.ringer, None);
    }

    #[test]
    fn the_edges_of_each_range_are_in_range() {
        let mut sent = wire_status();
        sent.battery.as_mut().unwrap().percent = 100;
        let cellular = sent.cellular.as_mut().unwrap();
        cellular.level = 0;
        cellular.operator_name = "\u{e9}".repeat(MAX_OPERATOR_NAME_BYTES / 2);
        let status = PhoneStatus::from_wire(&sent);
        assert_eq!(status.battery.unwrap().percent, Some(100));
        let cellular = status.cellular.unwrap();
        assert_eq!(cellular.level, Some(0));
        assert_eq!(
            cellular.operator_name.map(|name| name.len()),
            Some(MAX_OPERATOR_NAME_BYTES)
        );
    }

    #[test]
    fn unset_parts_stay_unknown() {
        let status = PhoneStatus::from_wire(&wire::PhoneStatus::default());
        assert_eq!(status, PhoneStatus::default());
        let mut sent = wire_status();
        sent.cellular.as_mut().unwrap().operator_name.clear();
        let cellular = PhoneStatus::from_wire(&sent).cellular.unwrap();
        assert_eq!(cellular.operator_name, None);
    }

    #[test]
    fn an_operator_name_cannot_reorder_the_text_around_it() {
        let mut sent = wire_status();
        sent.cellular.as_mut().unwrap().operator_name = "Tele\u{202e}moc\n".into();
        let cellular = PhoneStatus::from_wire(&sent).cellular.unwrap();
        assert_eq!(cellular.operator_name.as_deref(), Some("Tele_moc_"));
    }
}
