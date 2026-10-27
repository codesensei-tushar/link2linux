//! What the UI shows of a phone's status and of ringing it, as snapshots.
//! They are IPC types, so they derive the serde and specta traits the
//! generated bindings need.

use link2linux_core::ring::{self, RingFailure, RingState};
use link2linux_core::status::{self, StatusState};
use serde::{Deserialize, Serialize};
use specta::Type;

/// What the desktop shows of a phone's status.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum PhoneStatus {
    /// The phone's current status. A part that is `null` is unknown, and
    /// shows as nothing.
    #[serde(rename_all = "camelCase")]
    Current {
        battery: Option<PhoneBattery>,
        /// The cellular connection the phone's status bar shows first;
        /// `null` on a phone without telephony.
        cellular: Option<PhoneCellular>,
        /// Set while the phone is connected to Wi-Fi.
        wifi: Option<PhoneWifi>,
        ringer: Option<RingerMode>,
        do_not_disturb: bool,
    },
    /// Connected, and the status is on its way.
    Waiting,
    /// The user hid the phone's status in this computer's settings.
    Hidden,
    /// The user turned the status off on the phone.
    TurnedOff,
    /// The phone does not share its status right now.
    Unavailable,
    /// The phone's app is too old to share its status.
    Unsupported,
    /// The phone is not connected, so its status is not current.
    #[default]
    Disconnected,
}

/// The phone's battery.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PhoneBattery {
    /// The charge, 0 to 100.
    pub percent: Option<u8>,
    /// Connected to power, also once the battery is full.
    pub plugged_in: bool,
    /// Battery saver is on.
    pub power_saver: bool,
}

/// The phone's cellular connection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PhoneCellular {
    /// Signal bars, 0 to 4.
    pub level: Option<u8>,
    pub network: Option<CellularNetwork>,
    /// The carrier's name.
    pub operator_name: Option<String>,
}

/// The phone's Wi-Fi connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PhoneWifi {
    /// Signal bars, 0 to 4.
    pub level: Option<u8>,
}

/// The cellular generation the phone's status bar shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
pub enum CellularNetwork {
    /// No SIM, no service, or emergency calls only.
    #[serde(rename = "noService")]
    NoService,
    #[serde(rename = "2g")]
    TwoG,
    #[serde(rename = "3g")]
    ThreeG,
    #[serde(rename = "4g")]
    FourG,
    #[serde(rename = "5g")]
    FiveG,
}

/// Whether calls and notifications make sound on the phone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum RingerMode {
    Normal,
    Vibrate,
    Silent,
}

/// What the desktop shows of ringing a phone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RingStatus {
    pub phase: RingPhase,
    /// Why the phone cannot ring now, or `null` when it can.
    pub blocked: Option<RingProblem>,
    /// Why the last ring or stop the user asked for did not work.
    pub problem: Option<RingProblem>,
    /// Why the phone stopped ringing without this computer asking.
    pub ended: Option<RingEnd>,
}

impl Default for RingStatus {
    fn default() -> Self {
        Self {
            phase: RingPhase::Idle,
            blocked: Some(RingProblem::NotConnected),
            problem: None,
            ended: None,
        }
    }
}

/// Where ringing stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum RingPhase {
    Idle,
    /// Asked to ring; the phone has not answered yet.
    Starting,
    Ringing,
    /// Asked to stop; the phone has not answered yet.
    Stopping,
}

/// Why the phone cannot ring, or did not do what the user asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum RingProblem {
    /// Ringing is turned off on the phone.
    TurnedOff,
    /// The phone can't ring right now, for example during a call.
    Unavailable,
    /// The phone's app is too old to ring.
    Unsupported,
    /// The phone is not connected.
    NotConnected,
    /// The phone has not yet said whether it can ring: shown as nothing.
    Waiting,
    /// The phone refused, or did not answer in time.
    Failed,
}

/// Why the phone stopped ringing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum RingEnd {
    /// Someone stopped it on the phone.
    StoppedOnPhone,
    /// A computer stopped it: this one or another one.
    StoppedFromDesktop,
    /// It rang for its full minute.
    TimedOut,
    /// It could not keep ringing, for example because a call started.
    Interrupted,
}

/// The snapshot of `state`.
#[must_use]
pub fn status(state: &StatusState) -> PhoneStatus {
    match state {
        StatusState::Current(status) => PhoneStatus::Current {
            battery: status.battery.map(|battery| PhoneBattery {
                percent: battery.percent,
                plugged_in: battery.plugged_in,
                power_saver: battery.power_saver,
            }),
            cellular: status.cellular.as_ref().map(|cellular| PhoneCellular {
                level: cellular.level,
                network: cellular.network.map(network),
                operator_name: cellular.operator_name.clone(),
            }),
            wifi: status.wifi.map(|wifi| PhoneWifi { level: wifi.level }),
            ringer: status.ringer.map(ringer),
            do_not_disturb: status.do_not_disturb,
        },
        StatusState::Waiting => PhoneStatus::Waiting,
        StatusState::Hidden => PhoneStatus::Hidden,
        StatusState::TurnedOff => PhoneStatus::TurnedOff,
        StatusState::Unavailable => PhoneStatus::Unavailable,
        StatusState::Unsupported => PhoneStatus::Unsupported,
        StatusState::Disconnected => PhoneStatus::Disconnected,
    }
}

fn network(network: status::CellularNetwork) -> CellularNetwork {
    match network {
        status::CellularNetwork::NoService => CellularNetwork::NoService,
        status::CellularNetwork::TwoG => CellularNetwork::TwoG,
        status::CellularNetwork::ThreeG => CellularNetwork::ThreeG,
        status::CellularNetwork::FourG => CellularNetwork::FourG,
        status::CellularNetwork::FiveG => CellularNetwork::FiveG,
    }
}

fn ringer(mode: status::RingerMode) -> RingerMode {
    match mode {
        status::RingerMode::Normal => RingerMode::Normal,
        status::RingerMode::Vibrate => RingerMode::Vibrate,
        status::RingerMode::Silent => RingerMode::Silent,
    }
}

/// The snapshot of `state`.
#[must_use]
pub fn ring(state: RingState) -> RingStatus {
    RingStatus {
        phase: match state.phase {
            ring::RingPhase::Idle => RingPhase::Idle,
            ring::RingPhase::Starting => RingPhase::Starting,
            ring::RingPhase::Ringing => RingPhase::Ringing,
            ring::RingPhase::Stopping => RingPhase::Stopping,
        },
        blocked: state.blocked.map(problem),
        problem: state.failure.map(problem),
        ended: state.ended.map(|ended| match ended {
            ring::RingEnd::StoppedOnPhone => RingEnd::StoppedOnPhone,
            ring::RingEnd::StoppedFromDesktop => RingEnd::StoppedFromDesktop,
            ring::RingEnd::TimedOut => RingEnd::TimedOut,
            ring::RingEnd::Interrupted => RingEnd::Interrupted,
        }),
    }
}

fn problem(failure: RingFailure) -> RingProblem {
    match failure {
        RingFailure::TurnedOff => RingProblem::TurnedOff,
        RingFailure::Unavailable => RingProblem::Unavailable,
        RingFailure::Unsupported => RingProblem::Unsupported,
        RingFailure::NotConnected => RingProblem::NotConnected,
        RingFailure::Waiting => RingProblem::Waiting,
        RingFailure::Failed => RingProblem::Failed,
    }
}

#[cfg(test)]
mod tests {
    use link2linux_core::ring::RingService;
    use link2linux_core::status::PhoneStatus as Status;

    use super::*;

    #[test]
    fn a_status_serializes_with_its_state_and_camel_case_fields() {
        let current = StatusState::Current(Status {
            battery: Some(status::Battery {
                percent: Some(42),
                plugged_in: true,
                power_saver: false,
            }),
            cellular: Some(status::Cellular {
                level: Some(3),
                network: Some(status::CellularNetwork::FiveG),
                operator_name: Some("Carrier".into()),
            }),
            wifi: None,
            ringer: Some(status::RingerMode::Vibrate),
            do_not_disturb: true,
        });
        let json = serde_json::to_value(status(&current)).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "state": "current",
                "battery": { "percent": 42, "pluggedIn": true, "powerSaver": false },
                "cellular": { "level": 3, "network": "5g", "operatorName": "Carrier" },
                "wifi": null,
                "ringer": "vibrate",
                "doNotDisturb": true,
            })
        );
        let json = serde_json::to_value(status(&StatusState::TurnedOff)).unwrap();
        assert_eq!(json, serde_json::json!({ "state": "turnedOff" }));
    }

    #[test]
    fn a_phone_without_a_session_cannot_ring() {
        assert_eq!(ring(RingService::new().state()), RingStatus::default());
        let json = serde_json::to_value(RingStatus::default()).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "phase": "idle",
                "blocked": "notConnected",
                "problem": null,
                "ended": null,
            })
        );
    }
}
