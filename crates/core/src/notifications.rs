//! Phone notifications, mirrored on the desktop.
//!
//! The phone reports notifications as they are posted and removed. The
//! desktop shows them through `link2linux-platform` and can dismiss them,
//! run their actions or send an inline reply.
//!
//! Stub: the types and the [`Notifications`] trait are in place, and the
//! wire messages are in `notifications.proto`; the service that maps
//! between the two comes next.

use std::time::SystemTime;

use crate::device::FeatureError;

/// A notification shown on the phone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhoneNotification {
    /// The phone's key for it, stable while it is shown.
    pub key: String,
    /// Package name of the app that posted it.
    pub app_package: String,
    /// Display name of that app.
    pub app_name: String,
    /// Title line.
    pub title: String,
    /// Body text.
    pub text: String,
    /// When the app posted it.
    pub posted_at: SystemTime,
    /// Buttons the app attached, in display order.
    pub actions: Vec<NotificationAction>,
}

/// A button on a [`PhoneNotification`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationAction {
    /// Identifier passed back to [`Notifications::invoke_action`].
    pub id: String,
    /// Button label.
    pub label: String,
    /// Whether the action takes a typed reply, like "Reply" in a chat app.
    pub accepts_reply: bool,
}

/// What the phone reports about its notifications.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum NotificationEvent {
    /// A notification was posted, or updated in place.
    Posted(PhoneNotification),
    /// A notification was removed on the phone.
    Removed {
        /// Its key.
        key: String,
    },
}

/// Requests the desktop makes about the phone's notifications.
pub trait Notifications: Send + Sync {
    /// Dismisses a notification on the phone.
    fn dismiss(&self, key: &str) -> impl Future<Output = Result<(), FeatureError>> + Send;

    /// Runs one of a notification's actions. `reply` carries the typed text
    /// for actions that [accept one](NotificationAction::accepts_reply).
    fn invoke_action(
        &self,
        key: &str,
        action_id: &str,
        reply: Option<&str>,
    ) -> impl Future<Output = Result<(), FeatureError>> + Send;
}
