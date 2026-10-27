//! What the UI shows of a phone's screen, and what it sends of the user's
//! input. They are IPC types, so they derive the serde and specta traits
//! the generated bindings need.

use link2linux_core::screen::{self, Codec, Input, InputProblem, Key, Point, ScreenState};
use serde::{Deserialize, Serialize};
use specta::Type;

/// What the desktop shows of a phone's screen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ScreenStatus {
    pub phase: ScreenPhase,
    /// Why the phone cannot show its screen now, or `null` when it can.
    pub blocked: Option<ScreenProblem>,
    /// Why the last start did not work.
    pub problem: Option<ScreenProblem>,
    /// Why the last stream ended without this computer stopping it.
    pub ended: Option<ScreenEnd>,
    /// The size and encoding of the frames, while the phone shows its
    /// screen.
    pub format: Option<ScreenFormat>,
    /// Why the phone did not carry out the user's last input.
    pub input_problem: Option<ScreenInputProblem>,
}

impl Default for ScreenStatus {
    fn default() -> Self {
        Self {
            phase: ScreenPhase::Idle,
            blocked: Some(ScreenProblem::NotConnected),
            problem: None,
            ended: None,
            format: None,
            input_problem: None,
        }
    }
}

/// Where the stream stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum ScreenPhase {
    Idle,
    /// Asked; the phone has not answered yet.
    Starting,
    /// The phone asks its user whether to share the screen.
    Asking,
    /// The phone shows its screen.
    Showing,
    /// Asked to stop; the phone has not answered yet.
    Stopping,
}

/// Why the phone cannot show its screen, or did not start.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum ScreenProblem {
    /// The phone is not connected.
    NotConnected,
    /// The phone has not yet said whether it can share its screen: shown
    /// as nothing.
    Waiting,
    /// The phone's app is too old to share its screen.
    Unsupported,
    /// Screen sharing is turned off on the phone.
    TurnedOff,
    /// The phone can't share its screen right now.
    Unavailable,
    /// The phone shares its screen with another computer.
    Busy,
    /// The phone can't ask its user, for example because the app's
    /// notifications are blocked there.
    CannotAsk,
    /// The phone can encode none of the formats this computer decodes.
    NoCodec,
    /// The phone refused, or did not answer in time.
    Failed,
}

/// Why a stream ended without this computer stopping it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum ScreenEnd {
    /// The user said no on the phone.
    Declined,
    /// Nobody answered on the phone in time.
    TimedOut,
    /// The user stopped sharing on the phone.
    StoppedOnPhone,
    /// The phone locked.
    Locked,
    /// Screen sharing was turned off on the phone.
    TurnedOff,
    /// The phone could not go on.
    Interrupted,
    /// The connection with the phone ended.
    Disconnected,
}

/// Why the phone did not carry out the user's input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum ScreenInputProblem {
    /// Control is off on the phone.
    ControlOff,
    /// The screen changed, for example it rotated, before the input
    /// arrived.
    ScreenChanged,
    /// No text field has the focus on the phone.
    NoTextField,
    /// The focused field doesn't take it: a password field the phone can't
    /// edit, or Enter where the field has no action.
    CannotType,
    /// The phone refused, or did not answer in time.
    Failed,
}

/// How the phone encodes its screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum ScreenCodec {
    H264,
    Jpeg,
}

impl ScreenCodec {
    /// The core's codec.
    #[must_use]
    pub fn to_core(self) -> Codec {
        match self {
            Self::H264 => Codec::H264,
            Self::Jpeg => Codec::Jpeg,
        }
    }

    fn from_core(codec: Codec) -> Self {
        match codec {
            Codec::H264 => Self::H264,
            Codec::Jpeg => Self::Jpeg,
        }
    }
}

/// The size and encoding of the frames being shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ScreenFormat {
    /// Names this size and encoding; gestures carry it.
    pub id: u32,
    pub codec: ScreenCodec,
    pub width: u32,
    pub height: u32,
    /// Whether the phone takes input from this computer now.
    pub input: bool,
}

/// Something the user did in the window that shows the phone's screen.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ScreenInput {
    /// One finger following `points`, in the pixels of the frames of
    /// format `formatId`.
    #[serde(rename_all = "camelCase")]
    Gesture {
        format_id: u32,
        points: Vec<ScreenPoint>,
    },
    Key {
        key: ScreenKey,
    },
    /// Text to type into the phone's focused field.
    Text {
        text: String,
    },
}

impl std::fmt::Debug for ScreenInput {
    // Typed text is the user's content: only its length goes into logs.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.clone().into_core())
    }
}

impl ScreenInput {
    /// The core's input.
    #[must_use]
    pub fn into_core(self) -> Input {
        match self {
            Self::Gesture { format_id, points } => Input::Gesture {
                format_id,
                points: points
                    .into_iter()
                    .map(|point| Point {
                        x: point.x,
                        y: point.y,
                        time_ms: point.time_ms,
                    })
                    .collect(),
            },
            Self::Key { key } => Input::Key(match key {
                ScreenKey::Back => Key::Back,
                ScreenKey::Home => Key::Home,
                ScreenKey::Recents => Key::Recents,
                ScreenKey::Notifications => Key::Notifications,
                ScreenKey::Backspace => Key::Backspace,
                ScreenKey::Enter => Key::Enter,
            }),
            Self::Text { text } => Input::Text(text),
        }
    }
}

/// One point of a gesture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ScreenPoint {
    pub x: u32,
    pub y: u32,
    /// Milliseconds after the gesture's first point.
    pub time_ms: u32,
}

/// The phone's keys the window offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum ScreenKey {
    Back,
    Home,
    Recents,
    Notifications,
    Backspace,
    Enter,
}

/// The snapshot of `state`.
#[must_use]
pub fn status(state: ScreenState) -> ScreenStatus {
    ScreenStatus {
        phase: match state.phase {
            screen::ScreenPhase::Idle => ScreenPhase::Idle,
            screen::ScreenPhase::Starting => ScreenPhase::Starting,
            screen::ScreenPhase::Asking => ScreenPhase::Asking,
            screen::ScreenPhase::Showing => ScreenPhase::Showing,
            screen::ScreenPhase::Stopping => ScreenPhase::Stopping,
        },
        blocked: state.blocked.map(problem),
        problem: state.failure.map(problem),
        ended: state.ended.map(|ended| match ended {
            screen::ScreenEnd::Declined => ScreenEnd::Declined,
            screen::ScreenEnd::TimedOut => ScreenEnd::TimedOut,
            screen::ScreenEnd::StoppedOnPhone => ScreenEnd::StoppedOnPhone,
            screen::ScreenEnd::Locked => ScreenEnd::Locked,
            screen::ScreenEnd::TurnedOff => ScreenEnd::TurnedOff,
            screen::ScreenEnd::Interrupted => ScreenEnd::Interrupted,
            screen::ScreenEnd::Disconnected => ScreenEnd::Disconnected,
        }),
        format: state.format.map(|format| ScreenFormat {
            id: format.id,
            codec: ScreenCodec::from_core(format.codec),
            width: format.width,
            height: format.height,
            input: format.input,
        }),
        input_problem: state.input_problem.map(|problem| match problem {
            InputProblem::ControlOff => ScreenInputProblem::ControlOff,
            InputProblem::ScreenChanged => ScreenInputProblem::ScreenChanged,
            InputProblem::NoTextField => ScreenInputProblem::NoTextField,
            InputProblem::CannotType => ScreenInputProblem::CannotType,
            InputProblem::Failed => ScreenInputProblem::Failed,
        }),
    }
}

fn problem(problem: screen::ScreenProblem) -> ScreenProblem {
    match problem {
        screen::ScreenProblem::NotConnected => ScreenProblem::NotConnected,
        screen::ScreenProblem::Waiting => ScreenProblem::Waiting,
        screen::ScreenProblem::Unsupported => ScreenProblem::Unsupported,
        screen::ScreenProblem::TurnedOff => ScreenProblem::TurnedOff,
        screen::ScreenProblem::Unavailable => ScreenProblem::Unavailable,
        screen::ScreenProblem::Busy => ScreenProblem::Busy,
        screen::ScreenProblem::CannotAsk => ScreenProblem::CannotAsk,
        screen::ScreenProblem::NoCodec => ScreenProblem::NoCodec,
        screen::ScreenProblem::Failed => ScreenProblem::Failed,
    }
}

#[cfg(test)]
mod tests {
    use link2linux_core::screen::ScreenService;

    use super::*;

    #[test]
    fn a_new_service_shows_as_the_default_status() {
        assert_eq!(
            status(ScreenService::new().state()),
            ScreenStatus::default()
        );
        let json = serde_json::to_value(ScreenStatus::default()).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "phase": "idle",
                "blocked": "notConnected",
                "problem": null,
                "ended": null,
                "format": null,
                "inputProblem": null,
            })
        );
    }

    #[test]
    fn input_reads_as_the_ui_sends_it_and_keeps_text_out_of_logs() {
        let gesture: ScreenInput = serde_json::from_value(serde_json::json!({
            "kind": "gesture",
            "formatId": 3,
            "points": [{ "x": 1, "y": 2, "timeMs": 0 }, { "x": 5, "y": 6, "timeMs": 40 }],
        }))
        .unwrap();
        assert_eq!(
            gesture.into_core(),
            Input::Gesture {
                format_id: 3,
                points: vec![
                    Point {
                        x: 1,
                        y: 2,
                        time_ms: 0
                    },
                    Point {
                        x: 5,
                        y: 6,
                        time_ms: 40
                    },
                ],
            }
        );
        let key: ScreenInput =
            serde_json::from_value(serde_json::json!({ "kind": "key", "key": "backspace" }))
                .unwrap();
        assert_eq!(key.into_core(), Input::Key(Key::Backspace));
        let text: ScreenInput =
            serde_json::from_value(serde_json::json!({ "kind": "text", "text": "hunter2" }))
                .unwrap();
        assert!(!format!("{text:?}").contains("hunter2"));
        assert_eq!(text.into_core(), Input::Text("hunter2".into()));
    }
}
