//! How mirrored notifications appear on the desktop, and what the user's
//! answers to them ask of the phone (`docs/protocol.md`, "Notifications").
//!
//! Everything here is a pure function of the notification, so the rules
//! are tested without a desktop or a session: [`wants_banner`] decides
//! whether a change alerts, [`to_desktop`] builds the banner, and
//! [`answer`] turns what the user did with a banner into a request for the
//! phone, or into opening the desktop app.

use std::path::PathBuf;

use link2linux_platform::{
    CloseReason, DesktopAction, DesktopNotification, InlineReply, NotificationCapabilities,
    NotificationResponse, Urgency, Visibility,
};
use link2linux_protocol::Payload;
use link2linux_protocol::v1::{
    NotificationActionRequest, NotificationCategory, NotificationContent,
    NotificationDismissRequest, NotificationImportance, NotificationVisibility, PhoneNotification,
};

/// Shown instead of content the phone's OS withheld, such as a one-time code.
pub const WITHHELD_BY_SYSTEM: &str = "Content hidden by Android. Check your phone.";
/// Shown instead of the content of an app locked on the phone.
pub const WITHHELD_APP_LOCKED: &str = "Hidden because the app is locked on your phone.";
/// Shown instead of the content of an app the user chose not to share.
pub const HIDDEN_BY_USER: &str = "You chose not to share this app's content.";
/// Shown instead of a notification the phone kept to itself because it
/// looks like it holds a one-time code.
pub const ONE_TIME_CODE: &str = "One-time code hidden. Check your phone.";
/// Shown instead of content a newer phone withheld for a reason this
/// desktop does not know.
pub const WITHHELD: &str = "Content hidden on your phone.";
/// The action key a desktop reports when the user clicks the banner itself.
pub const OPEN_KEY: &str = "default";
/// The longest reply sent to the phone, in bytes.
pub const MAX_REPLY_BYTES: usize = 8 * 1024;
/// How many of a conversation's latest messages the banner shows.
const BANNER_MESSAGES: usize = 3;

/// What the user's answer to a banner asks for.
#[derive(Debug, Clone, PartialEq)]
pub enum Answer {
    /// A request for the phone, to send through the outbox.
    Request(Box<Payload>),
    /// Open the desktop app on the notification, for example to type a
    /// reply where the desktop has no reply field in its banners.
    Open {
        /// The notification's key.
        key: String,
        /// The reply action to open, if any.
        action_id: Option<String>,
    },
}

/// The text shown instead of withheld content, if the content is withheld.
/// A value this desktop does not know counts as withheld, never as content.
#[must_use]
pub fn withheld_text(notification: &PhoneNotification) -> Option<&'static str> {
    // Not `content()`, which reads an unknown value as unspecified, and so
    // as full content.
    match NotificationContent::try_from(notification.content) {
        Ok(NotificationContent::Unspecified | NotificationContent::Full) => None,
        Ok(NotificationContent::RedactedBySystem) => Some(WITHHELD_BY_SYSTEM),
        Ok(NotificationContent::RedactedAppLocked) => Some(WITHHELD_APP_LOCKED),
        Ok(NotificationContent::HiddenByUser) => Some(HIDDEN_BY_USER),
        Ok(NotificationContent::OneTimeCode) => Some(ONE_TIME_CODE),
        Err(_) => Some(WITHHELD),
    }
}

/// Whether only the app, the time and the category of `notification` may
/// be shown: content the user hid, a one-time code, or content withheld
/// for a reason this desktop does not know. Android's own redaction keeps
/// the title, so it is not one of these.
#[must_use]
pub fn app_only(notification: &PhoneNotification) -> bool {
    !matches!(
        NotificationContent::try_from(notification.content),
        Ok(NotificationContent::Unspecified
            | NotificationContent::Full
            | NotificationContent::RedactedBySystem
            | NotificationContent::RedactedAppLocked)
    )
}

/// Whether a posted notification gets a banner. Notifications from a sync
/// never alert, and `MIN` and `LOW` ones only go to the desktop's list.
#[must_use]
pub fn wants_banner(notification: &PhoneNotification, alert: bool) -> bool {
    alert
        && !matches!(
            notification.importance(),
            NotificationImportance::Min | NotificationImportance::Low
        )
}

/// The desktop banner for `notification`. `quiet` updates one already on
/// screen without a sound; `image` is a local copy of its picture or large
/// icon, when fetched; `capabilities` are the desktop's, which decide
/// between a reply field and a reply button.
#[must_use]
pub fn to_desktop(
    notification: &PhoneNotification,
    quiet: bool,
    capabilities: NotificationCapabilities,
    image: Option<PathBuf>,
) -> DesktopNotification {
    let app_name = if notification.app_name.is_empty() {
        notification.app_id.clone()
    } else {
        notification.app_name.clone()
    };
    let app_only = app_only(notification);
    // The pop-up goes out under this app's name (a phone app picks its own
    // label, which must not pass for a desktop app's), so the summary names
    // the phone app.
    let summary = [&notification.conversation_title, &notification.title]
        .into_iter()
        .find(|s| !s.is_empty() && !app_only)
        .map_or_else(|| app_name.clone(), |title| format!("{app_name} · {title}"));
    let withheld = withheld_text(notification);
    let body = match withheld {
        Some(text) => text.to_owned(),
        None if !notification.messages.is_empty() => conversation_body(notification),
        None => notification.text.clone(),
    };

    let mut actions = Vec::new();
    let mut inline_reply = None;
    if withheld.is_none() {
        for action in &notification.actions {
            if action.accepts_reply && capabilities.inline_reply && inline_reply.is_none() {
                inline_reply = Some(InlineReply {
                    action_key: action.id.clone(),
                    placeholder: "Reply".to_owned(),
                });
            } else {
                actions.push(DesktopAction {
                    key: action.id.clone(),
                    label: action.label.clone(),
                });
            }
        }
    }

    let category = notification.category();
    DesktopNotification {
        app_name,
        summary,
        body,
        actions,
        urgency: match (category, notification.importance()) {
            (NotificationCategory::Call, _) => Urgency::Critical,
            (_, NotificationImportance::Min | NotificationImportance::Low) => Urgency::Low,
            _ => Urgency::Normal,
        },
        category: match category {
            NotificationCategory::Message => Some("im.received"),
            NotificationCategory::Email => Some("email.arrived"),
            NotificationCategory::Call => Some("call.incoming"),
            NotificationCategory::MissedCall => Some("call.unanswered"),
            _ => None,
        }
        .map(str::to_owned),
        image_path: image.filter(|_| !app_only),
        inline_reply,
        quiet,
        visibility: visibility(notification.visibility()),
    }
}

/// The latest messages of a conversation, oldest first, one per line.
fn conversation_body(notification: &PhoneNotification) -> String {
    let start = notification.messages.len().saturating_sub(BANNER_MESSAGES);
    notification.messages[start..]
        .iter()
        .map(|message| {
            if message.outgoing {
                format!("You: {}", message.text)
            } else if message.sender.is_empty() {
                message.text.clone()
            } else {
                format!("{}: {}", message.sender, message.text)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// What the user's `response` to the banner of `notification` asks for, if
/// anything. A dismissal reaches the phone only when the user dismissed
/// the banner, the phone can dismiss the notification, and dismiss sync is
/// on; a banner the desktop closed itself, or that expired, asks nothing.
#[must_use]
pub fn answer(
    notification: &PhoneNotification,
    response: &NotificationResponse,
    dismiss_sync: bool,
) -> Option<Answer> {
    let key = notification.key.clone();
    match response {
        NotificationResponse::Action {
            key: action_key, ..
        } if action_key == OPEN_KEY => Some(Answer::Open {
            key,
            action_id: None,
        }),
        NotificationResponse::Action {
            key: action_key, ..
        } => {
            let action = notification.actions.iter().find(|a| &a.id == action_key)?;
            if action.accepts_reply {
                // There was no reply field: the text has to be typed elsewhere.
                return Some(Answer::Open {
                    key,
                    action_id: Some(action.id.clone()),
                });
            }
            Some(action_request(key, action.id.clone(), String::new()))
        }
        NotificationResponse::Replied {
            action_key, text, ..
        } => {
            let action = notification
                .actions
                .iter()
                .find(|a| &a.id == action_key && a.accepts_reply)?;
            Some(action_request(
                key,
                action.id.clone(),
                truncate(text, MAX_REPLY_BYTES),
            ))
        }
        NotificationResponse::Closed {
            reason: CloseReason::Dismissed,
            ..
        } if dismiss_sync && notification.dismissible => Some(Answer::Request(Box::new(
            Payload::NotificationDismissRequest(NotificationDismissRequest {
                key,
                ..NotificationDismissRequest::default()
            }),
        ))),
        _ => None,
    }
}

fn action_request(key: String, action_id: String, reply_text: String) -> Answer {
    Answer::Request(Box::new(Payload::NotificationActionRequest(
        NotificationActionRequest {
            key,
            action_id,
            reply_text,
            ..NotificationActionRequest::default()
        },
    )))
}

/// `text` cut to at most `max` bytes at a character boundary.
pub(crate) fn truncate(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

/// The desktop's form of the visibility the app gave, which counts as
/// private when it gave none.
fn visibility(value: NotificationVisibility) -> Visibility {
    match value {
        NotificationVisibility::Public => Visibility::Public,
        NotificationVisibility::Secret => Visibility::Secret,
        NotificationVisibility::Private | NotificationVisibility::Unspecified => {
            Visibility::Private
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use link2linux_platform::NotificationId;
    use link2linux_protocol::v1::{ConversationMessage, NotificationAction};

    const ID: NotificationId = NotificationId(4);

    fn chat() -> PhoneNotification {
        let message = |sender: &str, text: &str, outgoing| ConversationMessage {
            sender: sender.into(),
            text: text.into(),
            outgoing,
            ..ConversationMessage::default()
        };
        PhoneNotification {
            key: "0|chat|7".into(),
            app_id: "org.example.chat".into(),
            app_name: "Chat".into(),
            title: "Ana".into(),
            text: "Running late".into(),
            conversation_title: "Book club".into(),
            category: NotificationCategory::Message.into(),
            importance: NotificationImportance::High.into(),
            dismissible: true,
            messages: vec![
                message("Ana", "first", false),
                message("Bo", "Is it at 8?", false),
                message("", "Yes", true),
                message("Ana", "Running late", false),
            ],
            actions: vec![
                NotificationAction {
                    id: "reply".into(),
                    label: "Reply".into(),
                    accepts_reply: true,
                    ..NotificationAction::default()
                },
                NotificationAction {
                    id: "read".into(),
                    label: "Mark as read".into(),
                    ..NotificationAction::default()
                },
            ],
            ..PhoneNotification::default()
        }
    }

    fn caps(inline_reply: bool) -> NotificationCapabilities {
        NotificationCapabilities {
            actions: true,
            inline_reply,
            ..NotificationCapabilities::default()
        }
    }

    fn request(answer: Option<Answer>) -> Payload {
        match answer {
            Some(Answer::Request(payload)) => *payload,
            other => panic!("expected a request, got {other:?}"),
        }
    }

    #[test]
    fn a_conversation_shows_its_latest_messages_and_a_reply_field() {
        let banner = to_desktop(&chat(), false, caps(true), Some("/c/ana.png".into()));
        assert_eq!(banner.app_name, "Chat");
        assert_eq!(banner.summary, "Chat · Book club");
        assert_eq!(banner.body, "Bo: Is it at 8?\nYou: Yes\nAna: Running late");
        assert_eq!(
            banner.inline_reply,
            Some(InlineReply {
                action_key: "reply".into(),
                placeholder: "Reply".into()
            })
        );
        assert_eq!(
            banner.actions,
            [DesktopAction {
                key: "read".into(),
                label: "Mark as read".into()
            }]
        );
        assert_eq!(banner.category.as_deref(), Some("im.received"));
        assert_eq!(banner.urgency, Urgency::Normal);
        assert_eq!(banner.image_path, Some("/c/ana.png".into()));
        assert!(!banner.quiet);
        assert_eq!(banner.visibility, Visibility::Private);
    }

    #[test]
    fn without_a_reply_field_the_reply_is_a_button_that_opens_the_app() {
        let banner = to_desktop(&chat(), true, caps(false), None);
        assert_eq!(banner.inline_reply, None);
        assert_eq!(banner.actions[0].key, "reply");
        assert!(banner.quiet);
        let response = NotificationResponse::Action {
            id: ID,
            key: "reply".into(),
        };
        assert_eq!(
            answer(&chat(), &response, true),
            Some(Answer::Open {
                key: "0|chat|7".into(),
                action_id: Some("reply".into())
            })
        );
    }

    #[test]
    fn withheld_content_is_explained_and_offers_nothing_to_run() {
        for (content, text) in [
            (NotificationContent::RedactedBySystem, WITHHELD_BY_SYSTEM),
            (NotificationContent::RedactedAppLocked, WITHHELD_APP_LOCKED),
            (NotificationContent::HiddenByUser, HIDDEN_BY_USER),
            (NotificationContent::OneTimeCode, ONE_TIME_CODE),
        ] {
            let mut notification = chat();
            notification.content = content.into();
            let banner = to_desktop(&notification, false, caps(true), None);
            assert_eq!(banner.body, text);
            assert!(banner.actions.is_empty() && banner.inline_reply.is_none());
        }
    }

    #[test]
    fn a_one_time_code_shows_only_the_app() {
        let mut notification = chat();
        notification.content = NotificationContent::OneTimeCode.into();
        let banner = to_desktop(&notification, false, caps(true), Some("/c/ana.png".into()));
        assert_eq!(
            (banner.summary.as_str(), banner.body.as_str()),
            ("Chat", ONE_TIME_CODE)
        );
        assert_eq!(banner.image_path, None);
        assert_eq!(banner.category.as_deref(), Some("im.received"));
    }

    #[test]
    fn content_of_a_kind_this_desktop_does_not_know_stays_hidden() {
        let mut notification = chat();
        notification.content = 99;
        assert!(app_only(&notification));
        let banner = to_desktop(&notification, false, caps(true), Some("/c/ana.png".into()));
        assert_eq!(banner.body, WITHHELD);
        assert_eq!(banner.summary, "Chat");
        assert!(banner.actions.is_empty() && banner.inline_reply.is_none());
        assert_eq!(banner.image_path, None);

        // Android's own redaction keeps the title.
        notification.content = NotificationContent::RedactedBySystem.into();
        assert!(!app_only(&notification));
        notification.content = NotificationContent::Unspecified.into();
        assert_eq!(withheld_text(&notification), None);
    }

    #[test]
    fn importance_and_category_set_the_urgency() {
        let mut quiet = chat();
        quiet.importance = NotificationImportance::Low.into();
        assert_eq!(
            to_desktop(&quiet, false, caps(false), None).urgency,
            Urgency::Low
        );
        assert!(!wants_banner(&quiet, true));
        assert!(wants_banner(&chat(), true));
        assert!(
            !wants_banner(&chat(), false),
            "synced notifications never alert"
        );

        let mut call = chat();
        call.category = NotificationCategory::Call.into();
        let banner = to_desktop(&call, false, caps(false), None);
        assert_eq!(banner.urgency, Urgency::Critical);
        assert_eq!(banner.category.as_deref(), Some("call.incoming"));

        let mut bare = PhoneNotification {
            app_id: "org.example.app".into(),
            ..PhoneNotification::default()
        };
        bare.text = "hi".into();
        let banner = to_desktop(&bare, false, caps(false), None);
        assert_eq!(
            (banner.app_name.as_str(), banner.summary.as_str()),
            ("org.example.app", "org.example.app")
        );
    }

    #[test]
    fn answers_become_requests_for_the_phone() {
        let open = NotificationResponse::Action {
            id: ID,
            key: OPEN_KEY.into(),
        };
        assert_eq!(
            answer(&chat(), &open, true),
            Some(Answer::Open {
                key: "0|chat|7".into(),
                action_id: None
            })
        );

        let read = NotificationResponse::Action {
            id: ID,
            key: "read".into(),
        };
        let Payload::NotificationActionRequest(run) = request(answer(&chat(), &read, true)) else {
            panic!("expected an action request");
        };
        assert_eq!(
            (
                run.key.as_str(),
                run.action_id.as_str(),
                run.reply_text.as_str()
            ),
            ("0|chat|7", "read", "")
        );

        let long = "é".repeat(MAX_REPLY_BYTES);
        let replied = NotificationResponse::Replied {
            id: ID,
            action_key: "reply".into(),
            text: long,
        };
        let Payload::NotificationActionRequest(reply) = request(answer(&chat(), &replied, true))
        else {
            panic!("expected an action request");
        };
        assert_eq!(reply.action_id, "reply");
        assert_eq!(reply.reply_text.len(), MAX_REPLY_BYTES);

        let gone = NotificationResponse::Action {
            id: ID,
            key: "no-longer-there".into(),
        };
        assert_eq!(answer(&chat(), &gone, true), None);
    }

    #[test]
    fn only_the_users_dismissal_reaches_the_phone() {
        let closed = |reason| NotificationResponse::Closed { id: ID, reason };
        let Payload::NotificationDismissRequest(dismiss) =
            request(answer(&chat(), &closed(CloseReason::Dismissed), true))
        else {
            panic!("expected a dismiss request");
        };
        assert_eq!(dismiss.key, "0|chat|7");

        assert_eq!(
            answer(&chat(), &closed(CloseReason::Dismissed), false),
            None,
            "dismiss sync off"
        );
        for reason in [
            CloseReason::ClosedByApp,
            CloseReason::Expired,
            CloseReason::Unknown,
        ] {
            assert_eq!(answer(&chat(), &closed(reason), true), None, "{reason:?}");
        }
        let mut ongoing = chat();
        ongoing.dismissible = false;
        assert_eq!(
            answer(&ongoing, &closed(CloseReason::Dismissed), true),
            None
        );
    }
}
