//! Desktop OS integration for link2linux, behind traits.
//!
//! `link2linux-core` never talks to the OS. Everything the desktop app needs
//! from it is a trait here, implemented once per OS:
//!
//! - [`NotificationSink`]: show phone notifications on the desktop;
//! - [`Clipboard`]: read, write and watch the desktop clipboard, as far as
//!   the desktop session allows ([`ClipboardAccess`]);
//! - [`FileStore`]: where received files go, and opening them or showing
//!   them in their folder;
//! - [`Bluetooth`] and [`Telephony`]: the phone's calls over Bluetooth,
//!   with the desktop as the hands-free unit of the phone;
//! - [`SecretStore`]: keep small secrets, such as the desktop's private
//!   key, in the desktop's keyring.
//!
//! Linux comes first, in the `linux` module. Another OS adds a sibling
//! module behind its own `cfg(target_os = ...)`, and the app shell picks the
//! one for the target it is built for.

#![warn(missing_docs)]

use std::path::{Path, PathBuf};

mod calls;
#[cfg(target_os = "linux")]
pub mod linux;
mod presence;
mod secrets;

pub use calls::{
    Bluetooth, BluetoothAdapter, BluetoothDevice, CallState, Gateway, GatewayCall,
    HANDS_FREE_GATEWAY_UUID, PROMPT_TIMEOUT, PairingEvent, PairingPrompt, Telephony,
};
pub use presence::{
    AddressKind, BluetoothAddress, MAX_ADVERTISED_NAME_BYTES, PRESENCE_SERVICE_UUID,
    ParseAddressError, PresenceAdapter, PresenceAdvertiser, truncate_device_name,
};
pub use secrets::SecretStore;

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
    /// How urgent the desktop should treat it.
    pub urgency: Urgency,
    /// A freedesktop notification category such as `im.received`, so the
    /// desktop can group it and pick an icon.
    pub category: Option<String>,
    /// A local image file shown with the notification, such as the
    /// sender's photo.
    pub image_path: Option<std::path::PathBuf>,
    /// A reply field for desktops that offer one in the notification itself.
    /// Desktops without it show only [`Self::actions`].
    pub inline_reply: Option<InlineReply>,
    /// Update the notification without a sound, for quiet changes such as
    /// progress.
    pub quiet: bool,
    /// How private the phone's app marked it.
    pub visibility: Visibility,
}

/// How private the app that posted a notification marked it, for the
/// desktop to honor while its screen is locked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Visibility {
    /// It may be shown in full.
    Public,
    /// Show that it exists, but not its content, on a locked screen. Also
    /// what an app that said nothing gets.
    #[default]
    Private,
    /// Do not show it at all on a locked screen.
    Secret,
}

/// How urgent a [`DesktopNotification`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Urgency {
    /// Background information.
    Low,
    /// The usual case.
    #[default]
    Normal,
    /// Needs attention now, such as an incoming call. Desktops keep it on
    /// screen until the user acts.
    Critical,
}

/// A reply field shown in a [`DesktopNotification`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineReply {
    /// The key reported in [`NotificationResponse::Replied`]: the phone
    /// action that takes the reply.
    pub action_key: String,
    /// Hint text shown in the empty field.
    pub placeholder: String,
}

/// What the desktop's notification service can do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct NotificationCapabilities {
    /// It shows action buttons.
    pub actions: bool,
    /// It offers a reply field inside the notification.
    pub inline_reply: bool,
    /// It shows images.
    pub images: bool,
    /// It keeps notifications in a list after they leave the screen.
    pub persistence: bool,
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
    /// The user sent a reply from the notification's own reply field.
    Replied {
        /// The notification.
        id: NotificationId,
        /// [`InlineReply::action_key`] of its reply field.
        action_key: String,
        /// What the user typed.
        text: String,
    },
    /// The notification left the screen.
    Closed {
        /// The notification.
        id: NotificationId,
        /// Why it closed. Only [`CloseReason::Dismissed`] is the user's doing.
        reason: CloseReason,
    },
}

/// Why a desktop notification closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseReason {
    /// It timed out.
    Expired,
    /// The user dismissed it.
    Dismissed,
    /// [`NotificationSink::close`] removed it.
    ClosedByApp,
    /// The desktop did not say.
    Unknown,
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

    /// What the desktop's notification service supports. Ask before
    /// offering an inline reply: without one, show a reply action instead.
    fn capabilities(
        &self,
    ) -> impl Future<Output = Result<NotificationCapabilities, PlatformError>> + Send;

    /// Waits for the user to act on one of the shown notifications.
    fn next_response(
        &self,
    ) -> impl Future<Output = Result<NotificationResponse, PlatformError>> + Send;
}

/// Text on the desktop clipboard.
#[derive(Clone, PartialEq, Eq)]
pub struct ClipboardText {
    /// The text, as it was copied.
    pub text: String,
    /// The app it was copied from marked it as a secret, as password
    /// managers do with the `x-kde-passwordManagerHint` type set to
    /// `secret`. Written back with the same mark, so clipboard managers
    /// leave it out of their history.
    pub sensitive: bool,
}

impl std::fmt::Debug for ClipboardText {
    // Clipboard text is the user's, and may be a password: never logged.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClipboardText")
            .field("bytes", &self.text.len())
            .field("sensitive", &self.sensitive)
            .finish()
    }
}

/// How much of the clipboard the desktop session lets the app use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardAccess {
    /// Reading, writing and change reports all work while the app's window
    /// is in the background.
    Background,
    /// The clipboard can be used only while the app's own window has focus,
    /// as on a Wayland session whose compositor offers no clipboard access
    /// to background apps. [`Clipboard::changed`] never resolves.
    FocusOnly,
}

/// The desktop clipboard.
pub trait Clipboard: Send + Sync {
    /// What the desktop session allows.
    fn access(&self) -> ClipboardAccess;

    /// The current text, or `None` when the clipboard holds no text, or
    /// holds copied files rather than text.
    fn read(&self) -> impl Future<Output = Result<Option<ClipboardText>, PlatformError>> + Send;

    /// Replaces the clipboard content with `text`, and keeps serving it to
    /// other apps until something else is copied.
    fn write(&self, text: &ClipboardText)
    -> impl Future<Output = Result<(), PlatformError>> + Send;

    /// Resolves the next time the clipboard changes, also after this app's
    /// own [`write`](Self::write): callers compare the content to tell the
    /// two apart. Never resolves with [`ClipboardAccess::FocusOnly`].
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

    /// The directory the user keeps pictures in, where saved photos go.
    /// [`Self::downloads_dir`] where the OS names none.
    ///
    /// # Errors
    ///
    /// Fails when the OS says neither.
    fn pictures_dir(&self) -> Result<PathBuf, PlatformError> {
        self.downloads_dir()
    }

    /// Opens the file at the absolute `path` with the desktop's default
    /// application. Only ever on the user's request: received files are
    /// never opened by themselves.
    fn open(&self, path: &Path) -> impl Future<Output = Result<(), PlatformError>> + Send;

    /// Shows the file at the absolute `path` in the desktop's file
    /// manager, selected in its folder where the file manager can do that.
    fn show_in_folder(&self, path: &Path)
    -> impl Future<Output = Result<(), PlatformError>> + Send;
}
