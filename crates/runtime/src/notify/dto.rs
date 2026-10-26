//! The mirrored notifications as the UI receives them.
//!
//! These types follow `src/features/notifications/types.ts`, which the
//! notifications page was written against; the generated bindings must stay
//! assignable to it.

use link2linux_core::notifications::Change;
use link2linux_protocol::v1::{
    ConversationMessage, NotificationAction, NotificationActionKind, NotificationCategory,
    NotificationContent, NotificationImportance, NotificationVisibility, PhoneNotification,
};
use serde::{Deserialize, Serialize};
use specta::Type;

/// A reference to an image the phone shared, by the SHA-256 of its bytes
/// in lowercase hexadecimal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ImageRef {
    pub sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum NotificationCategoryView {
    Unspecified,
    Message,
    Email,
    Call,
    MissedCall,
    Event,
    Reminder,
    Alarm,
    Progress,
    Media,
    Social,
    Promotion,
    Navigation,
    System,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum NotificationImportanceView {
    Unspecified,
    Min,
    Low,
    Default,
    High,
}

/// Whether the text fields hold the real content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum NotificationContentView {
    Full,
    RedactedBySystem,
    RedactedAppLocked,
    HiddenByUser,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum NotificationVisibilityView {
    Unspecified,
    Public,
    Private,
    Secret,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum NotificationActionKindView {
    Unspecified,
    Reply,
    MarkRead,
    Delete,
    Archive,
    Mute,
    AnswerCall,
    DeclineCall,
    HangUp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct NotificationActionInfo {
    /// Identifies the action within its notification. Opaque.
    pub id: String,
    pub label: String,
    pub kind: NotificationActionKindView,
    /// The action takes free-form reply text.
    pub accepts_reply: bool,
    /// Ready-made replies the app offers.
    pub reply_choices: Vec<String>,
    /// Android runs this action only on an unlocked phone.
    pub requires_unlock: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ConversationMessageInfo {
    /// Empty for messages the phone's user sent.
    pub sender: String,
    pub text: String,
    #[specta(type = specta_typescript::Number)]
    pub sent_unix_ms: i64,
    pub outgoing: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct NotificationProgressInfo {
    pub current: u32,
    pub max: u32,
    pub indeterminate: bool,
}

/// A notification as the phone shares it and the desktop mirrors it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MirroredNotification {
    /// Opaque and stable for the notification's lifetime.
    pub key: String,
    pub app_name: String,
    pub app_icon: Option<ImageRef>,
    #[specta(type = specta_typescript::Number)]
    pub when_unix_ms: i64,
    pub title: String,
    pub text: String,
    pub sub_text: String,
    pub category: NotificationCategoryView,
    pub importance: NotificationImportanceView,
    pub ongoing: bool,
    /// Dismissal on the phone is possible.
    pub dismissible: bool,
    /// Notifications with the same group are shown stacked. Empty for none.
    pub group: String,
    pub actions: Vec<NotificationActionInfo>,
    /// Conversations only: latest messages, oldest first.
    pub messages: Vec<ConversationMessageInfo>,
    pub conversation_title: String,
    pub is_group_conversation: bool,
    pub progress: Option<NotificationProgressInfo>,
    pub large_icon: Option<ImageRef>,
    pub picture: Option<ImageRef>,
    pub content: NotificationContentView,
    pub visibility: NotificationVisibilityView,
}

/// A change to a phone's mirrored list, as pushed to the UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "type", rename_all = "camelCase")]
#[allow(
    clippy::large_enum_variant,
    reason = "posted events are the common case; boxing would only add an allocation"
)]
pub enum NotificationEvent {
    /// New, or an update that replaces the earlier version in place.
    #[serde(rename_all = "camelCase")]
    Posted {
        notification: MirroredNotification,
        /// The phone asks to alert the user. False for sync and quiet
        /// updates.
        alert: bool,
    },
    Removed {
        key: String,
    },
}

fn image(reference: &Option<link2linux_protocol::v1::ImageRef>) -> Option<ImageRef> {
    let reference = reference.as_ref()?;
    if reference.sha256.len() != 32 {
        return None;
    }
    Some(ImageRef {
        sha256: reference
            .sha256
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
    })
}

fn category(value: NotificationCategory) -> NotificationCategoryView {
    use NotificationCategory as C;
    use NotificationCategoryView as V;
    match value {
        C::Unspecified => V::Unspecified,
        C::Message => V::Message,
        C::Email => V::Email,
        C::Call => V::Call,
        C::MissedCall => V::MissedCall,
        C::Event => V::Event,
        C::Reminder => V::Reminder,
        C::Alarm => V::Alarm,
        C::Progress => V::Progress,
        C::Media => V::Media,
        C::Social => V::Social,
        C::Promotion => V::Promotion,
        C::Navigation => V::Navigation,
        C::System => V::System,
    }
}

fn importance(value: NotificationImportance) -> NotificationImportanceView {
    match value {
        NotificationImportance::Unspecified => NotificationImportanceView::Unspecified,
        NotificationImportance::Min => NotificationImportanceView::Min,
        NotificationImportance::Low => NotificationImportanceView::Low,
        NotificationImportance::Default => NotificationImportanceView::Default,
        NotificationImportance::High => NotificationImportanceView::High,
    }
}

fn content(value: NotificationContent) -> NotificationContentView {
    match value {
        // A phone that says nothing shares the real content.
        NotificationContent::Unspecified | NotificationContent::Full => {
            NotificationContentView::Full
        }
        NotificationContent::RedactedBySystem => NotificationContentView::RedactedBySystem,
        NotificationContent::RedactedAppLocked => NotificationContentView::RedactedAppLocked,
        NotificationContent::HiddenByUser | NotificationContent::OneTimeCode => {
            NotificationContentView::HiddenByUser
        }
    }
}

fn visibility(value: NotificationVisibility) -> NotificationVisibilityView {
    match value {
        NotificationVisibility::Unspecified => NotificationVisibilityView::Unspecified,
        NotificationVisibility::Public => NotificationVisibilityView::Public,
        NotificationVisibility::Private => NotificationVisibilityView::Private,
        NotificationVisibility::Secret => NotificationVisibilityView::Secret,
    }
}

fn kind(value: NotificationActionKind) -> NotificationActionKindView {
    use NotificationActionKind as K;
    use NotificationActionKindView as V;
    match value {
        K::Unspecified => V::Unspecified,
        K::Reply => V::Reply,
        K::MarkRead => V::MarkRead,
        K::Delete => V::Delete,
        K::Archive => V::Archive,
        K::Mute => V::Mute,
        K::AnswerCall => V::AnswerCall,
        K::DeclineCall => V::DeclineCall,
        K::HangUp => V::HangUp,
    }
}

fn action(action: &NotificationAction) -> NotificationActionInfo {
    NotificationActionInfo {
        id: action.id.clone(),
        label: action.label.clone(),
        kind: kind(action.kind()),
        accepts_reply: action.accepts_reply,
        reply_choices: action.reply_choices.clone(),
        requires_unlock: action.requires_unlock,
    }
}

fn message(message: &ConversationMessage) -> ConversationMessageInfo {
    ConversationMessageInfo {
        sender: message.sender.clone(),
        text: message.text.clone(),
        sent_unix_ms: message.sent_unix_ms,
        outgoing: message.outgoing,
    }
}

/// The UI's form of a notification the phone shared.
#[must_use]
pub fn mirrored(notification: &PhoneNotification) -> MirroredNotification {
    MirroredNotification {
        key: notification.key.clone(),
        app_name: notification.app_name.clone(),
        app_icon: image(&notification.app_icon),
        when_unix_ms: notification.when_unix_ms,
        title: notification.title.clone(),
        text: notification.text.clone(),
        sub_text: notification.sub_text.clone(),
        category: category(notification.category()),
        importance: importance(notification.importance()),
        ongoing: notification.ongoing,
        dismissible: notification.dismissible,
        group: notification.group.clone(),
        actions: notification.actions.iter().map(action).collect(),
        messages: notification.messages.iter().map(message).collect(),
        conversation_title: notification.conversation_title.clone(),
        is_group_conversation: notification.is_group_conversation,
        progress: notification
            .progress
            .as_ref()
            .map(|progress| NotificationProgressInfo {
                current: progress.current,
                max: progress.max,
                indeterminate: progress.indeterminate,
            }),
        large_icon: image(&notification.large_icon),
        picture: image(&notification.picture),
        content: content(notification.content()),
        visibility: visibility(notification.visibility()),
    }
}

/// The UI's form of a change to the mirrored list.
#[must_use]
pub fn event(change: &Change) -> NotificationEvent {
    match change {
        Change::Posted {
            notification,
            alert,
        } => NotificationEvent::Posted {
            notification: mirrored(notification),
            alert: *alert,
        },
        Change::Removed { key, .. } => NotificationEvent::Removed { key: key.clone() },
    }
}

#[cfg(test)]
mod tests {
    use link2linux_protocol::v1::NotificationProgress;

    use super::*;

    fn rich() -> PhoneNotification {
        PhoneNotification {
            key: "k".into(),
            app_name: "Chat".into(),
            title: "Ana".into(),
            category: NotificationCategory::MissedCall.into(),
            importance: NotificationImportance::High.into(),
            content: NotificationContent::HiddenByUser.into(),
            dismissible: true,
            actions: vec![NotificationAction {
                id: "a".into(),
                label: "Reply".into(),
                kind: NotificationActionKind::Reply.into(),
                accepts_reply: true,
                reply_choices: vec!["ok".into()],
                requires_unlock: true,
            }],
            progress: Some(NotificationProgress {
                current: 1,
                max: 4,
                indeterminate: false,
            }),
            app_icon: Some(link2linux_protocol::v1::ImageRef {
                sha256: vec![0xab; 32],
                ..Default::default()
            }),
            large_icon: Some(link2linux_protocol::v1::ImageRef {
                sha256: vec![1; 3],
                ..Default::default()
            }),
            ..PhoneNotification::default()
        }
    }

    #[test]
    fn a_notification_keeps_what_the_page_shows() {
        let shown = mirrored(&rich());
        assert_eq!(shown.category, NotificationCategoryView::MissedCall);
        assert_eq!(shown.importance, NotificationImportanceView::High);
        assert_eq!(shown.content, NotificationContentView::HiddenByUser);
        assert_eq!(shown.actions[0].kind, NotificationActionKindView::Reply);
        assert!(shown.actions[0].accepts_reply && shown.actions[0].requires_unlock);
        assert_eq!(shown.progress.unwrap().max, 4);
        assert_eq!(shown.app_icon.unwrap().sha256, "ab".repeat(32));
        assert_eq!(
            shown.large_icon, None,
            "a hash of the wrong length is no image"
        );
    }

    #[test]
    fn unspecified_content_counts_as_full() {
        let shown = mirrored(&PhoneNotification::default());
        assert_eq!(shown.content, NotificationContentView::Full);
    }

    #[test]
    fn changes_become_events() {
        let posted = event(&Change::Posted {
            notification: Box::new(rich()),
            alert: true,
        });
        assert!(matches!(
            posted,
            NotificationEvent::Posted { alert: true, .. }
        ));
        let removed = event(&Change::Removed {
            key: "k".into(),
            reason: link2linux_protocol::v1::NotificationRemovalReason::Dismissed,
        });
        assert_eq!(removed, NotificationEvent::Removed { key: "k".into() });
    }
}
