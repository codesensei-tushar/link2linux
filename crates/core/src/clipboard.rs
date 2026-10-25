//! A clipboard shared between the phone and the desktop.
//!
//! When the user copies something on one device, the other one can paste
//! it. The desktop side watches the local clipboard through
//! `link2linux-platform` and pushes changes with [`Clipboard::set`]; the
//! phone reports its changes as [`ClipboardEvent`]s.
//!
//! Stub: the types and the [`Clipboard`] trait are in place; the wire
//! messages come with this feature's schema.

use crate::device::FeatureError;

/// Clipboard content that can cross between the devices.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ClipboardContent {
    /// Plain text.
    Text(String),
}

/// What the phone reports about its clipboard.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ClipboardEvent {
    /// The user copied something on the phone.
    Changed(ClipboardContent),
}

/// Requests the desktop makes about the phone's clipboard.
pub trait Clipboard: Send + Sync {
    /// Puts `content` on the phone's clipboard.
    fn set(
        &self,
        content: &ClipboardContent,
    ) -> impl Future<Output = Result<(), FeatureError>> + Send;
}
