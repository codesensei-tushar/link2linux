//! SMS and MMS conversations from the phone.
//!
//! The desktop lists conversations, reads their messages and sends replies
//! through the phone, which stays the device that actually sends them.
//!
//! Stub: the types and the [`Messages`] trait are in place; the wire
//! messages come with this feature's schema.

use std::time::SystemTime;

use crate::device::FeatureError;

/// A conversation thread on the phone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conversation {
    /// The phone's thread identifier.
    pub id: String,
    /// Phone numbers or addresses of the other participants.
    pub participants: Vec<String>,
    /// The latest message, for list views.
    pub snippet: String,
    /// When the latest message was sent or received.
    pub updated_at: SystemTime,
    /// Number of unread messages.
    pub unread: u32,
}

/// One message in a [`Conversation`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    /// The phone's message identifier.
    pub id: String,
    /// The conversation it belongs to.
    pub conversation_id: String,
    /// Who sent it: an address, or `None` for messages sent from the phone.
    pub sender: Option<String>,
    /// Text content.
    pub body: String,
    /// When it was sent or received.
    pub timestamp: SystemTime,
}

/// What the phone reports about its messages.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum MessageEvent {
    /// A message arrived or was sent.
    Received(Message),
    /// A conversation's summary changed (new message, read state).
    ConversationUpdated(Conversation),
}

/// Requests the desktop makes about the phone's messages.
pub trait Messages: Send + Sync {
    /// The most recent conversations, newest first.
    fn conversations(
        &self,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<Conversation>, FeatureError>> + Send;

    /// Messages of one conversation sent before `before` (or the newest
    /// ones when `None`), newest first.
    fn messages(
        &self,
        conversation_id: &str,
        before: Option<SystemTime>,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<Message>, FeatureError>> + Send;

    /// Sends `body` to the participants of a conversation, through the
    /// phone.
    fn send(
        &self,
        conversation_id: &str,
        body: &str,
    ) -> impl Future<Output = Result<(), FeatureError>> + Send;

    /// Marks every message in a conversation as read.
    fn mark_read(
        &self,
        conversation_id: &str,
    ) -> impl Future<Output = Result<(), FeatureError>> + Send;
}
