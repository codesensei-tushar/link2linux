//! Phone notifications in a session.
//!
//! [`NotificationsDriver`] is the seam between a session and the
//! notifications feature: the session loop feeds it every event, it keeps
//! the mirrored list and the outbox, and it shows banners through a
//! [`Presenter`]. [`ServiceDriver`] runs the core's notifications service
//! behind it.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use link2linux_core::device::DeviceInfo;
pub use link2linux_core::notifications::CachedImage;
use link2linux_core::notifications::{Answer, Change};
use link2linux_core::session::{Session, SessionEvent};
use link2linux_core::transport::TlsConnection;
use link2linux_platform::{
    DesktopNotification, NotificationCapabilities, NotificationId, NotificationResponse,
    NotificationSink, PlatformError,
};

/// A boxed future, so the traits here can be used as `dyn`.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub mod dto;
mod popups;
mod service;

pub use popups::{NEUTRAL_TEXT, PrivatePopups};
pub use service::{ServiceDriver, ServiceFactory};

/// The session type the runtime runs.
pub type PhoneSession = Session<TlsConnection>;

/// Why the notifications feature could not handle something.
#[derive(Debug, thiserror::Error)]
pub enum DriverError {
    /// The phone broke the notification rules: the session ends as a
    /// protocol error.
    #[error("the phone broke the notification rules: {0}")]
    PhoneBrokeRules(String),
    /// Anything else. The session goes on: this is logged, not fatal.
    #[error("{0}")]
    Failed(String),
}

/// A [`NotificationSink`] usable as a trait object.
pub trait Presenter: Send + Sync {
    /// See [`NotificationSink::show`].
    fn show<'a>(
        &'a self,
        notification: &'a DesktopNotification,
        replaces: Option<NotificationId>,
    ) -> BoxFuture<'a, Result<NotificationId, PlatformError>>;

    /// See [`NotificationSink::close`].
    fn close(&self, id: NotificationId) -> BoxFuture<'_, Result<(), PlatformError>>;

    /// Shows a pop-up of this app's own, whose words are the app's, such as
    /// "Clipboard from Pixel", and `when_locked` instead while the screen
    /// is locked, since names such as the phone's come from the phone.
    /// Clicking it opens the app window. See [`NotificationSink::show`].
    fn show_own<'a>(
        &'a self,
        notification: &'a DesktopNotification,
        when_locked: &'a DesktopNotification,
        replaces: Option<NotificationId>,
    ) -> BoxFuture<'a, Result<NotificationId, PlatformError>> {
        let _ = when_locked;
        self.show(notification, replaces)
    }

    /// See [`NotificationSink::capabilities`].
    fn capabilities(&self) -> BoxFuture<'_, Result<NotificationCapabilities, PlatformError>>;

    /// See [`NotificationSink::next_response`].
    fn next_response(&self) -> BoxFuture<'_, Result<NotificationResponse, PlatformError>>;
}

impl<N: NotificationSink> Presenter for N {
    fn show<'a>(
        &'a self,
        notification: &'a DesktopNotification,
        replaces: Option<NotificationId>,
    ) -> BoxFuture<'a, Result<NotificationId, PlatformError>> {
        Box::pin(NotificationSink::show(self, notification, replaces))
    }

    fn close(&self, id: NotificationId) -> BoxFuture<'_, Result<(), PlatformError>> {
        Box::pin(NotificationSink::close(self, id))
    }

    fn capabilities(&self) -> BoxFuture<'_, Result<NotificationCapabilities, PlatformError>> {
        Box::pin(NotificationSink::capabilities(self))
    }

    fn next_response(&self) -> BoxFuture<'_, Result<NotificationResponse, PlatformError>> {
        Box::pin(NotificationSink::next_response(self))
    }
}

/// A [`Presenter`] shared by every phone's notifications service, used as
/// the [`NotificationSink`] each service wants to own.
#[derive(Clone)]
pub struct SharedSink(pub Arc<dyn Presenter>);

impl NotificationSink for SharedSink {
    async fn show(
        &self,
        notification: &DesktopNotification,
        replaces: Option<NotificationId>,
    ) -> Result<NotificationId, PlatformError> {
        self.0.show(notification, replaces).await
    }

    async fn close(&self, id: NotificationId) -> Result<(), PlatformError> {
        self.0.close(id).await
    }

    async fn capabilities(&self) -> Result<NotificationCapabilities, PlatformError> {
        self.0.capabilities().await
    }

    async fn next_response(&self) -> Result<NotificationResponse, PlatformError> {
        self.0.next_response().await
    }
}

/// The notifications feature of one phone, driven by its session loop.
///
/// The loop calls [`session_started`](Self::session_started) once the
/// session is up, feeds every [`SessionEvent`] through
/// [`handle_event`](Self::handle_event) first and handles what comes back
/// itself, passes banner responses to
/// [`handle_response`](Self::handle_response), and calls
/// [`session_ended`](Self::session_ended) when the session is over.
pub trait NotificationsDriver: Send {
    /// The session is up, or the feature became ready: starts the sync and
    /// sends what waits in the outbox.
    fn session_started<'a>(
        &'a mut self,
        session: &'a mut PhoneSession,
    ) -> BoxFuture<'a, Result<(), DriverError>>;

    /// Takes the events of the notifications feature and returns the rest.
    fn handle_event<'a>(
        &'a mut self,
        session: &'a mut PhoneSession,
        event: SessionEvent,
    ) -> BoxFuture<'a, Result<Option<SessionEvent>, DriverError>>;

    /// The user did something with a banner. `session` is `None` between
    /// sessions: actions then wait in the outbox.
    fn handle_response<'a>(
        &'a mut self,
        session: Option<&'a mut PhoneSession>,
        response: NotificationResponse,
    ) -> BoxFuture<'a, Result<Option<Answer>, DriverError>>;

    /// The session is over: forgets what only holds within it.
    fn session_ended(&mut self);

    /// Each change to the mirrored notifications, as it is applied.
    fn subscribe(&self) -> tokio::sync::broadcast::Receiver<Change>;

    /// The user dismissed a notification on the notifications page. It goes
    /// to the phone through the outbox, at once when `session` is up.
    fn dismiss<'a>(
        &'a mut self,
        session: Option<&'a mut PhoneSession>,
        key: &'a str,
    ) -> BoxFuture<'a, Result<(), DriverError>>;

    /// Whether the phone's call notifications get banners of their own:
    /// not while the desktop shows the phone's calls itself.
    fn set_call_banners(&mut self, show: bool) -> BoxFuture<'_, ()> {
        let _ = show;
        Box::pin(async {})
    }

    /// The user ran an action of a notification on the notifications page,
    /// with `reply` for actions that take text.
    fn run_action<'a>(
        &'a mut self,
        session: Option<&'a mut PhoneSession>,
        key: &'a str,
        action_id: &'a str,
        reply: Option<&'a str>,
    ) -> BoxFuture<'a, Result<(), DriverError>>;
}

/// Makes the driver for a phone.
pub trait DriverFactory: Send + Sync {
    /// A driver for the phone `device`, which stays with it across
    /// sessions.
    fn create(&self, device: &DeviceInfo) -> Box<dyn NotificationsDriver>;

    /// The picture with this SHA-256 that phone `device_id` shared, if it
    /// has arrived.
    fn image(&self, _device_id: &str, _sha256: &[u8]) -> Option<CachedImage> {
        None
    }

    /// Phone `device_id` is no longer paired: drops its pictures.
    fn forget(&self, _device_id: &str) {}
}
