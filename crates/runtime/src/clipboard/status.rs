//! What the clipboard page shows: how this desktop follows its clipboard,
//! each phone's state, what went where lately, and the latest problem.
//! Never the text.

use std::collections::{HashMap, VecDeque};

use link2linux_core::clipboard::{ClipboardEvent, NotShared, PhoneClipboard, PushFailure};
use serde::{Deserialize, Serialize};
use specta::Type;

/// The most entries the activity list keeps.
pub const MAX_ACTIVITY: usize = 20;

/// The state of the shared clipboard, as a snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ClipboardStatus {
    pub mode: ClipboardMode,
    /// Every paired phone, in pairing order.
    pub phones: Vec<ClipboardPhone>,
    /// What went where lately, newest first.
    pub activity: Vec<ClipboardActivity>,
    /// The latest problem, until a later transfer it concerns works.
    pub problem: Option<ClipboardProblem>,
    /// Text from a phone that waits for the window to gain focus.
    pub held: Option<HeldClipboard>,
}

/// How this desktop follows its clipboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum ClipboardMode {
    /// Each copy is shared as it happens, and text from the phones goes on
    /// the clipboard as it arrives.
    Automatic,
    /// The desktop session lets the app use the clipboard only while its
    /// window has focus: the clipboard is shared when the window gains
    /// focus or the user clicks Send, and text from the phones waits until
    /// the window gains focus.
    WhenFocused,
    /// The app cannot reach the clipboard.
    Unavailable,
}

/// A paired phone and whether it takes the clipboard now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ClipboardPhone {
    pub device_id: String,
    pub name: String,
    pub state: ClipboardPhoneState,
}

/// Whether a phone takes the clipboard now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum ClipboardPhoneState {
    Ready,
    /// The user has clipboard sharing off on the phone.
    TurnedOff,
    /// The phone shares a clipboard, but cannot right now.
    Unavailable,
    /// The phone's app is too old to share a clipboard.
    Unsupported,
    Disconnected,
}

impl From<PhoneClipboard> for ClipboardPhoneState {
    fn from(state: PhoneClipboard) -> Self {
        match state {
            PhoneClipboard::Ready => Self::Ready,
            PhoneClipboard::TurnedOff => Self::TurnedOff,
            PhoneClipboard::Unavailable => Self::Unavailable,
            PhoneClipboard::Unsupported => Self::Unsupported,
            PhoneClipboard::Disconnected => Self::Disconnected,
        }
    }
}

/// Text that went to or came from a phone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ClipboardActivity {
    pub device_id: String,
    pub phone_name: String,
    pub direction: ClipboardDirection,
    /// The text's length in bytes.
    pub bytes: u32,
    /// A password manager marked it as secret.
    pub sensitive: bool,
    /// When it happened (Unix time, milliseconds).
    #[specta(type = specta_typescript::Number)]
    pub at_unix_ms: i64,
}

/// Which way text went.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum ClipboardDirection {
    /// From this desktop to the phone.
    Sent,
    /// From the phone to this desktop.
    Received,
}

/// Something the user may want to know about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ClipboardProblem {
    pub kind: ClipboardProblemKind,
    /// The phone it concerns, if it concerns one.
    pub device_id: Option<String>,
    pub phone_name: Option<String>,
    /// The text's length, for `tooLarge`.
    pub bytes: Option<u32>,
    /// When it happened (Unix time, milliseconds).
    #[specta(type = specta_typescript::Number)]
    pub at_unix_ms: i64,
}

/// What went wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum ClipboardProblemKind {
    /// The copied text is longer than 256 KiB, so it was not shared.
    TooLarge,
    /// A password manager marked the copy as secret, and "Share passwords"
    /// is off.
    PasswordNotShared,
    /// The user asked to send the clipboard, but it holds no text.
    NoText,
    /// The user asked to send the clipboard while sharing is off.
    SharingOff,
    /// The user asked to send the clipboard, but no phone takes it now.
    NoPhone,
    /// The phone has clipboard sharing turned off.
    TurnedOffOnPhone,
    /// The phone could not set its clipboard.
    PhoneUnavailable,
    /// The phone did not take the text.
    Failed,
}

/// Text from a phone that waits for the window to gain focus.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct HeldClipboard {
    pub device_id: String,
    pub phone_name: String,
    pub bytes: u32,
    pub sensitive: bool,
}

/// The lengths the page shows: texts this long are refused anyway.
pub(crate) fn bytes(len: usize) -> u32 {
    u32::try_from(len).unwrap_or(u32::MAX)
}

/// The clipboard page's state, kept from the hub's events.
#[derive(Debug, Default)]
pub(crate) struct Board {
    phones: HashMap<String, ClipboardPhoneState>,
    activity: VecDeque<ClipboardActivity>,
    problem: Option<ClipboardProblem>,
}

impl Board {
    /// Applies `event` at `now` (Unix time, milliseconds), naming phones
    /// with `name_of`.
    pub fn apply(&mut self, event: &ClipboardEvent, name_of: &dyn Fn(&str) -> String, now: i64) {
        match event {
            ClipboardEvent::Sent {
                device_id,
                bytes,
                sensitive,
            } => {
                self.record(
                    device_id,
                    name_of,
                    ClipboardDirection::Sent,
                    *bytes,
                    *sensitive,
                    now,
                );
                // A copy went out: the problems of earlier copies are over.
                if self.problem.as_ref().is_some_and(|problem| {
                    problem.device_id.is_none() || problem.device_id.as_ref() == Some(device_id)
                }) {
                    self.problem = None;
                }
            }
            ClipboardEvent::Received {
                device_id,
                bytes,
                sensitive,
                ..
            } => {
                self.record(
                    device_id,
                    name_of,
                    ClipboardDirection::Received,
                    *bytes,
                    *sensitive,
                    now,
                );
                if self
                    .problem
                    .as_ref()
                    .is_some_and(|problem| problem.device_id.as_ref() == Some(device_id))
                {
                    self.problem = None;
                }
            }
            ClipboardEvent::Failed { device_id, reason } => {
                let kind = match reason {
                    PushFailure::TurnedOff => ClipboardProblemKind::TurnedOffOnPhone,
                    PushFailure::Unavailable => ClipboardProblemKind::PhoneUnavailable,
                    PushFailure::Failed => ClipboardProblemKind::Failed,
                };
                self.problem = Some(ClipboardProblem {
                    kind,
                    device_id: Some(device_id.clone()),
                    phone_name: Some(name_of(device_id)),
                    bytes: None,
                    at_unix_ms: now,
                });
            }
            ClipboardEvent::NotShared(reason) => {
                let (kind, length) = match reason {
                    NotShared::TooLarge { bytes } => {
                        (ClipboardProblemKind::TooLarge, Some(self::bytes(*bytes)))
                    }
                    NotShared::Secret => (ClipboardProblemKind::PasswordNotShared, None),
                    NotShared::NoText => (ClipboardProblemKind::NoText, None),
                    NotShared::Off => (ClipboardProblemKind::SharingOff, None),
                };
                self.problem = Some(ClipboardProblem {
                    kind,
                    device_id: None,
                    phone_name: None,
                    bytes: length,
                    at_unix_ms: now,
                });
            }
            ClipboardEvent::Phone { device_id, state } => {
                self.phones.insert(device_id.clone(), (*state).into());
            }
            _ => {}
        }
    }

    fn record(
        &mut self,
        device_id: &str,
        name_of: &dyn Fn(&str) -> String,
        direction: ClipboardDirection,
        bytes: usize,
        sensitive: bool,
        now: i64,
    ) {
        if self.activity.len() == MAX_ACTIVITY {
            self.activity.pop_back();
        }
        self.activity.push_front(ClipboardActivity {
            device_id: device_id.to_owned(),
            phone_name: name_of(device_id),
            direction,
            bytes: self::bytes(bytes),
            sensitive,
            at_unix_ms: now,
        });
    }

    /// Notes that the user asked to send the clipboard while no phone takes
    /// it.
    pub fn no_phone(&mut self, now: i64) {
        self.problem = Some(ClipboardProblem {
            kind: ClipboardProblemKind::NoPhone,
            device_id: None,
            phone_name: None,
            bytes: None,
            at_unix_ms: now,
        });
    }

    /// Whether any phone takes the clipboard now.
    pub fn any_ready(&self) -> bool {
        self.phones
            .values()
            .any(|state| *state == ClipboardPhoneState::Ready)
    }

    /// A phone is no longer paired.
    pub fn forget(&mut self, device_id: &str) {
        self.phones.remove(device_id);
        if self
            .problem
            .as_ref()
            .is_some_and(|problem| problem.device_id.as_deref() == Some(device_id))
        {
            self.problem = None;
        }
    }

    /// The snapshot for the page, with `phones` the paired phones' ids and
    /// names in pairing order.
    pub fn snapshot(
        &self,
        mode: ClipboardMode,
        phones: Vec<(String, String)>,
        held: Option<HeldClipboard>,
    ) -> ClipboardStatus {
        ClipboardStatus {
            mode,
            phones: phones
                .into_iter()
                .map(|(device_id, name)| ClipboardPhone {
                    state: self
                        .phones
                        .get(&device_id)
                        .copied()
                        .unwrap_or(ClipboardPhoneState::Disconnected),
                    device_id,
                    name,
                })
                .collect(),
            activity: self.activity.iter().cloned().collect(),
            problem: self.problem.clone(),
            held,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name_of(id: &str) -> String {
        format!("Phone {id}")
    }

    fn sent(device_id: &str, bytes: usize) -> ClipboardEvent {
        ClipboardEvent::Sent {
            device_id: device_id.into(),
            bytes,
            sensitive: false,
        }
    }

    fn failed(device_id: &str, reason: PushFailure) -> ClipboardEvent {
        ClipboardEvent::Failed {
            device_id: device_id.into(),
            reason,
        }
    }

    #[test]
    fn transfers_are_listed_newest_first_and_only_the_latest_are_kept() {
        let mut board = Board::default();
        for i in 0..MAX_ACTIVITY + 5 {
            board.apply(&sent("a", i), &name_of, i64::try_from(i).unwrap());
        }
        board.apply(
            &ClipboardEvent::Received {
                device_id: "b".into(),
                bytes: 3,
                sensitive: true,
                held: true,
            },
            &name_of,
            100,
        );
        let status = board.snapshot(ClipboardMode::Automatic, Vec::new(), None);
        assert_eq!(status.activity.len(), MAX_ACTIVITY);
        let newest = &status.activity[0];
        assert_eq!(newest.direction, ClipboardDirection::Received);
        assert_eq!(
            (newest.phone_name.as_str(), newest.bytes, newest.sensitive),
            ("Phone b", 3, true)
        );
        assert_eq!(status.activity[1].bytes, 24);
        assert_eq!(status.activity[MAX_ACTIVITY - 1].bytes, 6);
    }

    #[test]
    fn a_problem_lasts_until_a_later_transfer_it_concerns_works() {
        let mut board = Board::default();
        board.apply(&failed("a", PushFailure::TurnedOff), &name_of, 1);
        let problem = board
            .snapshot(ClipboardMode::Automatic, Vec::new(), None)
            .problem;
        let problem = problem.unwrap();
        assert_eq!(problem.kind, ClipboardProblemKind::TurnedOffOnPhone);
        assert_eq!(problem.phone_name.as_deref(), Some("Phone a"));

        // Another phone took it: the first phone's problem stays.
        board.apply(&sent("b", 1), &name_of, 2);
        assert!(board.problem.is_some());
        board.apply(&sent("a", 1), &name_of, 3);
        assert!(board.problem.is_none());

        // A copy that stayed here is over once a copy goes out.
        board.apply(
            &ClipboardEvent::NotShared(NotShared::TooLarge { bytes: 300_000 }),
            &name_of,
            4,
        );
        let problem = board.problem.clone().unwrap();
        assert_eq!(
            (problem.kind, problem.bytes),
            (ClipboardProblemKind::TooLarge, Some(300_000))
        );
        board.apply(
            &ClipboardEvent::Received {
                device_id: "a".into(),
                bytes: 1,
                sensitive: false,
                held: false,
            },
            &name_of,
            5,
        );
        assert!(board.problem.is_some());
        board.apply(&sent("b", 1), &name_of, 6);
        assert!(board.problem.is_none());

        board.apply(&failed("a", PushFailure::Failed), &name_of, 7);
        board.forget("a");
        assert!(board.problem.is_none());
    }

    #[test]
    fn every_paired_phone_is_listed_with_its_state() {
        let mut board = Board::default();
        assert!(!board.any_ready());
        board.apply(
            &ClipboardEvent::Phone {
                device_id: "a".into(),
                state: PhoneClipboard::Ready,
            },
            &name_of,
            1,
        );
        board.apply(
            &ClipboardEvent::Phone {
                device_id: "c".into(),
                state: PhoneClipboard::Unsupported,
            },
            &name_of,
            1,
        );
        assert!(board.any_ready());
        let phones = vec![
            ("a".to_owned(), "Pixel".to_owned()),
            ("b".to_owned(), "Galaxy".to_owned()),
        ];
        let held = HeldClipboard {
            device_id: "a".into(),
            phone_name: "Pixel".into(),
            bytes: 4,
            sensitive: false,
        };
        let status = board.snapshot(ClipboardMode::WhenFocused, phones, Some(held.clone()));
        let states: Vec<_> = status
            .phones
            .iter()
            .map(|phone| (phone.name.as_str(), phone.state))
            .collect();
        assert_eq!(
            states,
            [
                ("Pixel", ClipboardPhoneState::Ready),
                ("Galaxy", ClipboardPhoneState::Disconnected)
            ]
        );
        assert_eq!(status.held, Some(held));
        board.forget("a");
        assert!(!board.any_ready());
    }

    #[test]
    fn the_status_reads_as_the_page_expects() {
        let mut board = Board::default();
        board.no_phone(5);
        let status = board.snapshot(ClipboardMode::WhenFocused, Vec::new(), None);
        let json = serde_json::to_value(&status).unwrap();
        assert_eq!(json["mode"], "whenFocused");
        assert_eq!(json["problem"]["kind"], "noPhone");
        assert_eq!(json["problem"]["atUnixMs"], 5);
    }
}
