//! Desktop OS integration for link2linux, behind traits.
//!
//! `link2linux-core` never talks to the OS. Everything the desktop app needs
//! from it is a trait here, implemented once per OS:
//!
//! - [`NotificationSink`]: show phone notifications on the desktop;
//! - [`Clipboard`]: read, write and watch the desktop clipboard;
//! - [`FileStore`]: where received files go, and opening them;
//! - [`CallAudio`]: carry call audio between the phone and the desktop's
//!   microphone and speakers over Bluetooth.
//!
//! Linux comes first, in the `linux` module. Another OS adds a sibling
//! module behind its own `cfg(target_os = ...)`, and the app shell picks the
//! one for the target it is built for.

#![warn(missing_docs)]

use std::path::{Path, PathBuf};

#[cfg(target_os = "linux")]
pub mod linux;

/// Why a platform call failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PlatformError {
    /// This OS integration has not been written yet.
    #[error("{0} is not supported on this platform yet")]
    Unsupported(&'static str),
    /// The OS service exists but cannot be used right now, for example no
    /// D-Bus session or no Bluetooth adapter.
    #[error("{service} is unavailable: {reason}")]
    Unavailable {
        /// The service that failed.
        service: &'static str,
        /// What the OS reported.
        reason: String,
    },
    /// An I/O error from the OS.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// A notification to show on the desktop.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DesktopNotification {
    /// Name of the phone app that posted it.
    pub app_name: String,
    /// First line, shown in bold by most desktops.
    pub summary: String,
    /// Main text.
    pub body: String,
    /// Buttons, in display order.
    pub actions: Vec<DesktopAction>,
}

/// A button on a [`DesktopNotification`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopAction {
    /// Identifier reported back in [`NotificationResponse::Action`].
    pub key: String,
    /// Button label.
    pub label: String,
}

/// Handle to a notification shown by a [`NotificationSink`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NotificationId(pub u32);

/// What the user did with a desktop notification.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum NotificationResponse {
    /// The user clicked an action button.
    Action {
        /// The notification.
        id: NotificationId,
        /// [`DesktopAction::key`] of the clicked button.
        key: String,
    },
    /// The notification was dismissed or expired.
    Closed(NotificationId),
}

/// Shows notifications on the desktop.
pub trait NotificationSink: Send + Sync {
    /// Shows `notification`, or replaces the one shown as `replaces`.
    fn show(
        &self,
        notification: &DesktopNotification,
        replaces: Option<NotificationId>,
    ) -> impl Future<Output = Result<NotificationId, PlatformError>> + Send;

    /// Removes a notification from the screen.
    fn close(&self, id: NotificationId) -> impl Future<Output = Result<(), PlatformError>> + Send;

    /// Waits for the user to act on one of the shown notifications.
    fn next_response(
        &self,
    ) -> impl Future<Output = Result<NotificationResponse, PlatformError>> + Send;
}

/// The desktop clipboard.
pub trait Clipboard: Send + Sync {
    /// The current text content, or `None` when the clipboard holds no text.
    fn read_text(&self) -> impl Future<Output = Result<Option<String>, PlatformError>> + Send;

    /// Replaces the clipboard content with `text`.
    fn write_text(&self, text: &str) -> impl Future<Output = Result<(), PlatformError>> + Send;

    /// Resolves the next time another application changes the clipboard.
    fn changed(&self) -> impl Future<Output = Result<(), PlatformError>> + Send;
}

/// Where files from the phone are saved, and how they are opened.
pub trait FileStore: Send + Sync {
    /// The directory received files are saved in.
    ///
    /// # Errors
    ///
    /// Fails when the OS does not say where downloads go.
    fn downloads_dir(&self) -> Result<PathBuf, PlatformError>;

    /// Opens `path` with the desktop's default application.
    fn open(&self, path: &Path) -> impl Future<Output = Result<(), PlatformError>> + Send;
}

/// Call audio between the phone and the desktop, over Bluetooth.
///
/// During a call the phone acts as an audio gateway and the desktop as a
/// hands-free device, so the user talks through the desktop's microphone
/// and speakers.
pub trait CallAudio: Send + Sync {
    /// Routes call audio through the phone with Bluetooth address `phone`
    /// (`AA:BB:CC:DD:EE:FF`).
    fn connect(&self, phone: &str) -> impl Future<Output = Result<(), PlatformError>> + Send;

    /// Stops routing call audio.
    fn disconnect(&self) -> impl Future<Output = Result<(), PlatformError>> + Send;
}
