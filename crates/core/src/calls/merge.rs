//! One list of the phone's calls, from the hands-free link and from the
//! phone's call notifications (`docs/protocol.md`, "Calls on the
//! desktop").
//!
//! The phone's dialer posts a notification for each call, which arrives
//! through the notifications feature with the caller's name and answer
//! and decline actions; the same call also comes over Bluetooth while the
//! hands-free link is up. A call seen both ways is shown once. It is
//! answered and ended over Bluetooth, which brings its audio to the
//! desktop, and through the notification's actions only when the
//! hands-free link does not have it, which leaves the audio on the phone.
//! Calls of other apps, such as a messenger's, only come as notifications.

use link2linux_platform::{CallState, Gateway, GatewayCall};
use link2linux_protocol::v1::{NotificationActionKind, NotificationCategory, PhoneNotification};

use crate::notifications::present::withheld_text;

/// A call notification of the phone's, with its call actions.
#[derive(Clone, PartialEq, Eq)]
pub struct CallNotification {
    /// The notification's key.
    pub key: String,
    /// The app that posted it, such as the phone's dialer.
    pub app_name: String,
    /// The caller as the notification names them: a contact's name or a
    /// number. Empty when the phone withheld the content.
    pub caller: String,
    /// The action that answers the call, while it rings.
    pub answer: Option<String>,
    /// The action that declines the call, while it rings.
    pub decline: Option<String>,
    /// The action that ends the call.
    pub hang_up: Option<String>,
}

impl std::fmt::Debug for CallNotification {
    // The caller is one of the user's contacts: never logged.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CallNotification")
            .field("key", &self.key)
            .field("caller_bytes", &self.caller.len())
            .field("answer", &self.answer.is_some())
            .field("decline", &self.decline.is_some())
            .field("hang_up", &self.hang_up.is_some())
            .finish_non_exhaustive()
    }
}

impl CallNotification {
    /// The call `notification` is about, if it is a call notification
    /// with at least one call action.
    #[must_use]
    pub fn of(notification: &PhoneNotification) -> Option<Self> {
        if notification.category() != NotificationCategory::Call {
            return None;
        }
        let action = |kind: NotificationActionKind| {
            notification
                .actions
                .iter()
                .find(|a| a.kind() == kind)
                .map(|a| a.id.clone())
        };
        let call = Self {
            key: notification.key.clone(),
            app_name: if notification.app_name.is_empty() {
                notification.app_id.clone()
            } else {
                notification.app_name.clone()
            },
            caller: if withheld_text(notification).is_some() {
                String::new()
            } else {
                [&notification.title, &notification.conversation_title]
                    .into_iter()
                    .find(|s| !s.trim().is_empty())
                    .map(|s| s.trim().to_owned())
                    .unwrap_or_default()
            },
            answer: action(NotificationActionKind::AnswerCall),
            decline: action(NotificationActionKind::DeclineCall),
            hang_up: action(NotificationActionKind::HangUp),
        };
        (call.answer.is_some() || call.decline.is_some() || call.hang_up.is_some()).then_some(call)
    }

    /// Whether the call rings: it can be answered.
    #[must_use]
    pub fn is_ringing(&self) -> bool {
        self.answer.is_some()
    }
}

/// How a shown call is answered and ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Control {
    /// Over the hands-free link: the audio comes to the desktop.
    Bluetooth {
        /// The gateway's id.
        gateway: String,
        /// The call's id.
        call: String,
    },
    /// Through the notification's actions: the audio stays on the phone.
    Notification {
        /// The notification's key.
        key: String,
        /// Its answer action.
        answer: Option<String>,
        /// Its decline action.
        decline: Option<String>,
        /// Its hang-up action.
        hang_up: Option<String>,
    },
}

/// A call as the desktop shows it.
#[derive(Clone, PartialEq, Eq)]
pub struct ShownCall {
    /// Identifies the call while it lasts: `bt:` and the gateway call's
    /// id, or `app:` and the notification's key.
    pub id: String,
    /// The other party's number, when known.
    pub number: String,
    /// The other party's name, when known.
    pub name: String,
    /// Where the call is.
    pub state: CallState,
    /// The app whose call it is, for calls that only come as a
    /// notification.
    pub app_name: Option<String>,
    /// The call notification shown for it, if any.
    pub notification: Option<String>,
    /// How it is answered and ended.
    pub control: Control,
}

impl std::fmt::Debug for ShownCall {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShownCall")
            .field("id", &self.id)
            .field("number_bytes", &self.number.len())
            .field("name_bytes", &self.name.len())
            .field("state", &self.state)
            .field("notification", &self.notification)
            .finish_non_exhaustive()
    }
}

/// The calls to show for one phone: those of its hands-free `gateway`
/// while the link is up, each with the call notification that is about
/// it, then the call notifications no gateway call matched.
#[must_use]
pub fn merge(gateway: Option<&Gateway>, notifications: &[CallNotification]) -> Vec<ShownCall> {
    let mut left: Vec<&CallNotification> = notifications.iter().collect();
    let mut shown = Vec::new();
    if let Some(gateway) = gateway {
        let calls: Vec<&GatewayCall> = gateway
            .calls
            .iter()
            .filter(|call| call.state != CallState::Disconnected)
            .collect();
        let mut matched: Vec<Option<&CallNotification>> = vec![None; calls.len()];
        // By the caller first.
        for (call, slot) in calls.iter().zip(&mut matched) {
            if let Some(index) = left.iter().position(|n| same_caller(call, n)) {
                *slot = Some(left.remove(index));
            }
        }
        // Then by state, where one ringing (or one ongoing) call of each
        // side is left: the caller may be a contact the link names only by
        // number.
        for ringing in [true, false] {
            let calls_left: Vec<usize> = (0..calls.len())
                .filter(|&i| matched[i].is_none() && calls[i].state.is_ringing() == ringing)
                .collect();
            let notes_left: Vec<usize> = (0..left.len())
                .filter(|&i| left[i].is_ringing() == ringing)
                .collect();
            if let ([call], [note]) = (calls_left.as_slice(), notes_left.as_slice()) {
                matched[*call] = Some(left.remove(*note));
            }
        }
        for (call, note) in calls.into_iter().zip(matched) {
            let name = if call.name.trim().is_empty() {
                note.map(|n| n.caller.as_str())
                    .filter(|caller| !is_number(caller))
                    .unwrap_or_default()
                    .to_owned()
            } else {
                call.name.clone()
            };
            shown.push(ShownCall {
                id: format!("bt:{}", call.id),
                number: call.number.clone(),
                name,
                state: call.state,
                app_name: None,
                notification: note.map(|n| n.key.clone()),
                control: Control::Bluetooth {
                    gateway: gateway.id.clone(),
                    call: call.id.clone(),
                },
            });
        }
    }
    shown.extend(left.into_iter().map(|note| {
        let (number, name) = if is_number(&note.caller) {
            (note.caller.clone(), String::new())
        } else {
            (String::new(), note.caller.clone())
        };
        ShownCall {
            id: format!("app:{}", note.key),
            number,
            name,
            state: if note.is_ringing() {
                CallState::Incoming
            } else {
                CallState::Active
            },
            app_name: Some(note.app_name.clone()),
            notification: Some(note.key.clone()),
            control: Control::Notification {
                key: note.key.clone(),
                answer: note.answer.clone(),
                decline: note.decline.clone(),
                hang_up: note.hang_up.clone(),
            },
        }
    }));
    shown
}

/// How many final digits two long numbers share when they are the same
/// number written in national and international form, as "0151 1234567"
/// and "+49 151 1234567".
const SAME_NUMBER_DIGITS: usize = 7;

/// Whether `call` and `note` name the same caller: the same number, maybe
/// written with and without a country code, or the same name.
fn same_caller(call: &GatewayCall, note: &CallNotification) -> bool {
    let (a, b) = (digits(&call.number), digits(&note.caller));
    let tail = |n: &str| n[n.len().saturating_sub(SAME_NUMBER_DIGITS)..].to_owned();
    let numbers = !a.is_empty()
        && !b.is_empty()
        && (a == b
            || (a.len() >= SAME_NUMBER_DIGITS
                && b.len() >= SAME_NUMBER_DIGITS
                && tail(&a) == tail(&b)));
    let names = !call.name.trim().is_empty()
        && call.name.trim().to_lowercase() == note.caller.to_lowercase();
    numbers || names
}

fn digits(text: &str) -> String {
    text.chars().filter(char::is_ascii_digit).collect()
}

/// Whether `text` is a phone number rather than a name.
fn is_number(text: &str) -> bool {
    !digits(text).is_empty()
        && text
            .chars()
            .all(|c| c.is_ascii_digit() || " +-().*#/".contains(c))
}

/// What the user does with a shown call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallAction {
    /// Answer it while it rings.
    Answer,
    /// Refuse it while it rings.
    Decline,
    /// End it.
    HangUp,
}

/// What a [`CallAction`] takes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Operation {
    /// Answer the gateway call `call`.
    Answer {
        /// The call's id.
        call: String,
    },
    /// End or decline the gateway call `call`.
    HangUp {
        /// The call's id.
        call: String,
    },
    /// Run the action `action_id` of the phone's notification `key`.
    RunAction {
        /// The notification's key.
        key: String,
        /// The action's id.
        action_id: String,
    },
}

/// What doing `action` on `call` takes, or `None` when it cannot be done,
/// such as answering a call that does not ring.
#[must_use]
pub fn operation(call: &ShownCall, action: CallAction) -> Option<Operation> {
    match &call.control {
        Control::Bluetooth { call: id, .. } => match action {
            CallAction::Answer if call.state.is_ringing() => {
                Some(Operation::Answer { call: id.clone() })
            }
            CallAction::Answer => None,
            CallAction::Decline | CallAction::HangUp => {
                Some(Operation::HangUp { call: id.clone() })
            }
        },
        Control::Notification {
            key,
            answer,
            decline,
            hang_up,
        } => {
            let action_id = match action {
                CallAction::Answer => answer.as_ref(),
                CallAction::Decline => decline.as_ref().or(hang_up.as_ref()),
                CallAction::HangUp => hang_up.as_ref().or(decline.as_ref()),
            }?;
            Some(Operation::RunAction {
                key: key.clone(),
                action_id: action_id.clone(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use link2linux_protocol::v1::{NotificationAction, NotificationContent};

    use super::*;

    fn action(id: &str, kind: NotificationActionKind) -> NotificationAction {
        NotificationAction {
            id: id.into(),
            label: id.into(),
            kind: kind.into(),
            ..NotificationAction::default()
        }
    }

    fn ringing(key: &str, caller: &str) -> PhoneNotification {
        PhoneNotification {
            key: key.into(),
            app_id: "com.android.dialer".into(),
            app_name: "Phone".into(),
            title: caller.into(),
            text: "Incoming call".into(),
            category: NotificationCategory::Call.into(),
            actions: vec![
                action("answer", NotificationActionKind::AnswerCall),
                action("decline", NotificationActionKind::DeclineCall),
            ],
            ..PhoneNotification::default()
        }
    }

    fn ongoing(key: &str, caller: &str) -> PhoneNotification {
        PhoneNotification {
            actions: vec![action("hang", NotificationActionKind::HangUp)],
            text: "Ongoing call".into(),
            ..ringing(key, caller)
        }
    }

    fn note(notification: &PhoneNotification) -> CallNotification {
        CallNotification::of(notification).unwrap()
    }

    fn call(id: &str, number: &str, name: &str, state: CallState) -> GatewayCall {
        GatewayCall {
            id: id.into(),
            number: number.into(),
            name: name.into(),
            state,
            multiparty: false,
        }
    }

    fn gateway(calls: Vec<GatewayCall>) -> Gateway {
        Gateway {
            id: "/ag0".into(),
            address: None,
            calls,
            muted: None,
        }
    }

    #[test]
    fn call_notifications_are_read_from_their_actions() {
        let ring = note(&ringing("k1", " Anna "));
        assert_eq!(ring.caller, "Anna");
        assert_eq!(ring.app_name, "Phone");
        assert_eq!(ring.answer.as_deref(), Some("answer"));
        assert_eq!(ring.decline.as_deref(), Some("decline"));
        assert!(ring.is_ringing());
        assert!(!note(&ongoing("k2", "Anna")).is_ringing());

        // Not a call, or a call without call actions: not a call notification.
        let mut chat = ringing("k3", "Anna");
        chat.category = NotificationCategory::Message.into();
        assert!(CallNotification::of(&chat).is_none());
        let mut missed = ringing("k4", "Anna");
        missed.actions.clear();
        assert!(CallNotification::of(&missed).is_none());

        // Withheld content names nobody.
        let mut hidden = ringing("k5", "Anna");
        hidden.content = NotificationContent::RedactedBySystem.into();
        assert_eq!(note(&hidden).caller, "");
        assert!(!format!("{ring:?}").contains("Anna"));
    }

    #[test]
    fn a_call_seen_both_ways_is_shown_once_and_controlled_over_bluetooth() {
        let link = gateway(vec![call(
            "/ag0/call0",
            "+1 555 0100",
            "",
            CallState::Incoming,
        )]);
        let notes = [note(&ringing("k1", "Anna"))];
        let shown = merge(Some(&link), &notes);
        assert_eq!(shown.len(), 1);
        let only = &shown[0];
        assert_eq!(only.id, "bt:/ag0/call0");
        assert_eq!(only.number, "+1 555 0100");
        // The link names the caller only by number; the notification by name.
        assert_eq!(only.name, "Anna");
        assert_eq!(only.notification.as_deref(), Some("k1"));
        assert_eq!(
            operation(only, CallAction::Answer),
            Some(Operation::Answer {
                call: "/ag0/call0".into()
            })
        );
        assert_eq!(
            operation(only, CallAction::Decline),
            Some(Operation::HangUp {
                call: "/ag0/call0".into()
            })
        );
    }

    #[test]
    fn calls_match_by_number_with_or_without_country_code_and_by_name() {
        let link = gateway(vec![
            call("/ag0/call0", "+4915112345678", "", CallState::Active),
            call("/ag0/call1", "", "Bob", CallState::Waiting),
        ]);
        let notes = [
            note(&ringing("waiting", "bob")),
            note(&ongoing("active", "0151 12345678")),
        ];
        let shown = merge(Some(&link), &notes);
        let pairs: Vec<(&str, Option<&str>, &str)> = shown
            .iter()
            .map(|s| (s.id.as_str(), s.notification.as_deref(), s.name.as_str()))
            .collect();
        // A number in the notification is not taken as a name.
        assert_eq!(
            pairs,
            [
                ("bt:/ag0/call0", Some("active"), ""),
                ("bt:/ag0/call1", Some("waiting"), "Bob"),
            ]
        );
    }

    #[test]
    fn other_apps_calls_are_shown_with_their_notification_actions() {
        let link = gateway(vec![call(
            "/ag0/call0",
            "+15550100",
            "Anna",
            CallState::Active,
        )]);
        let mut messenger = ringing("m1", "Carol");
        messenger.app_name = "Messenger".into();
        let notes = [note(&ongoing("k1", "Anna")), note(&messenger)];
        let shown = merge(Some(&link), &notes);
        assert_eq!(shown.len(), 2);
        let other = &shown[1];
        assert_eq!(other.id, "app:m1");
        assert_eq!(other.name, "Carol");
        assert_eq!(other.app_name.as_deref(), Some("Messenger"));
        assert_eq!(other.state, CallState::Incoming);
        assert_eq!(
            operation(other, CallAction::Answer),
            Some(Operation::RunAction {
                key: "m1".into(),
                action_id: "answer".into()
            })
        );
    }

    #[test]
    fn without_the_link_notifications_are_the_calls() {
        let notes = [
            note(&ringing("k1", "+1 555 0100")),
            note(&ongoing("k2", "Anna")),
        ];
        let shown = merge(None, &notes);
        assert_eq!(shown[0].number, "+1 555 0100");
        assert_eq!(shown[0].name, "");
        assert_eq!(shown[0].state, CallState::Incoming);
        assert_eq!(shown[1].state, CallState::Active);
        // An ongoing call ends with its hang-up action; declining it does
        // the same, and it cannot be answered.
        for action in [CallAction::HangUp, CallAction::Decline] {
            assert_eq!(
                operation(&shown[1], action),
                Some(Operation::RunAction {
                    key: "k2".into(),
                    action_id: "hang".into()
                })
            );
        }
        assert_eq!(operation(&shown[1], CallAction::Answer), None);
        // A ringing call without a hang-up action hangs up by declining.
        assert_eq!(
            operation(&shown[0], CallAction::HangUp),
            Some(Operation::RunAction {
                key: "k1".into(),
                action_id: "decline".into()
            })
        );
    }

    #[test]
    fn ended_calls_are_left_out_and_ongoing_ones_cannot_be_answered() {
        let link = gateway(vec![
            call("/ag0/call0", "1", "", CallState::Disconnected),
            call("/ag0/call1", "2", "", CallState::Dialing),
        ]);
        let shown = merge(Some(&link), &[]);
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].id, "bt:/ag0/call1");
        assert_eq!(operation(&shown[0], CallAction::Answer), None);
        assert!(!format!("{:?}", shown[0]).contains("\"2\""));
    }

    #[test]
    fn two_ringing_calls_of_each_side_are_not_matched_by_state() {
        let link = gateway(vec![
            call("/ag0/call0", "", "", CallState::Incoming),
            call("/ag0/call1", "", "", CallState::Waiting),
        ]);
        let notes = [note(&ringing("a", "Anna")), note(&ringing("b", "Bob"))];
        let shown = merge(Some(&link), &notes);
        // Nothing tells which is which, so all four are shown.
        assert_eq!(shown.len(), 4);
        assert!(shown[..2].iter().all(|s| s.notification.is_none()));
    }
}
