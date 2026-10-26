//! The notifications feature on the desktop, driven by a [`Session`].
//!
//! [`NotificationService`] joins the other parts of the module: it keeps the
//! [`Mirror`] in step with the phone, shows banners through a
//! [`NotificationSink`], fetches their images into the [`ImageCache`], and
//! sends the user's dismissals and actions through the [`Outbox`]. It does
//! not own the session, so it can share one with other features:
//!
//! 1. [`NotificationService::session_started`] once the session is up;
//! 2. every [`SessionEvent`] through [`NotificationService::handle_event`],
//!    which keeps the notification events and hands back the rest;
//! 3. every banner response from [`NotificationSink::next_response`] through
//!    [`NotificationService::handle_response`];
//! 4. [`NotificationService::session_ended`] when the session is gone.
//!
//! The notifications page follows [`NotificationService::subscribe`] and
//! reads [`NotificationService::notifications`].

use std::collections::HashMap;
use std::time::SystemTime;

use link2linux_platform::{
    NotificationCapabilities, NotificationId, NotificationResponse, NotificationSink,
};
use link2linux_protocol::Payload;
use link2linux_protocol::v1::{
    NotificationActionRequest, NotificationDismissRequest, PhoneNotification,
};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::broadcast;

use super::images::ImageCache;
use super::mirror::{Change, Mirror, MirrorError};
use super::present::{self, Answer};
use crate::Feature;
use crate::outbox::{IdempotencyKey, Outbox, OutboxEntry, OutboxError, OutboxStore};
use crate::session::{Session, SessionError, SessionEvent};
use crate::sync::SyncCursor;

/// The user's settings for the feature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotificationSettings {
    /// Dismissing a banner on the desktop dismisses the notification on the
    /// phone too. On by default.
    pub dismiss_sync: bool,
}

impl Default for NotificationSettings {
    fn default() -> Self {
        Self { dismiss_sync: true }
    }
}

/// Why the service could not go on with the session.
#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    /// The session failed.
    #[error(transparent)]
    Session(#[from] SessionError),
    /// The phone broke the sync rules; end the session as a protocol error.
    #[error(transparent)]
    Mirror(#[from] MirrorError),
    /// The phone answered a request with a message it does not take.
    #[error("unexpected answer to a notifications request")]
    UnexpectedReply,
    /// The outbox could not be stored.
    #[error(transparent)]
    Outbox(#[from] OutboxError),
}

/// What a request this service sent is waiting for.
enum Pending {
    Sync,
    Image(Vec<u8>),
    Outbox(IdempotencyKey),
}

/// The notifications feature, between a session, the desktop and the user.
pub struct NotificationService<K, O> {
    sink: K,
    outbox: Outbox<O>,
    images: ImageCache,
    settings: NotificationSettings,
    mirror: Mirror,
    capabilities: Option<NotificationCapabilities>,
    /// The banner of each notification on screen, both ways.
    banners: HashMap<String, NotificationId>,
    keys: HashMap<NotificationId, String>,
    pending: HashMap<u64, Pending>,
    changes: broadcast::Sender<Change>,
}

impl<K: NotificationSink, O: OutboxStore> NotificationService<K, O> {
    /// A service that resumes `mirror`, which the caller restored from the
    /// cursor and notifications it stored (or [`Mirror::default`]).
    pub fn new(
        sink: K,
        outbox: Outbox<O>,
        images: ImageCache,
        settings: NotificationSettings,
        mirror: Mirror,
    ) -> Self {
        Self {
            sink,
            outbox,
            images,
            settings,
            mirror,
            capabilities: None,
            banners: HashMap::new(),
            keys: HashMap::new(),
            pending: HashMap::new(),
            changes: broadcast::channel(64).0,
        }
    }

    /// The mirrored notifications, for the notifications page.
    pub fn notifications(&self) -> impl Iterator<Item = &PhoneNotification> {
        self.mirror.items()
    }

    /// The cursor to store with [`Self::notifications`].
    pub fn cursor(&self) -> SyncCursor {
        self.mirror.cursor()
    }

    /// Each change to the mirrored notifications, as it is applied.
    pub fn subscribe(&self) -> broadcast::Receiver<Change> {
        self.changes.subscribe()
    }

    /// The user's dismissals and actions not yet answered by the phone,
    /// oldest first, for showing them as pending.
    pub fn pending(&self) -> Vec<OutboxEntry> {
        self.outbox.pending()
    }

    /// Changes the user's settings.
    pub fn set_settings(&mut self, settings: NotificationSettings) {
        self.settings = settings;
    }

    /// Starts the sync, if notifications are ready on both devices, and
    /// sends what the outbox holds. Call it once the session is up.
    pub async fn session_started<S>(&mut self, session: &mut Session<S>) -> Result<(), ServiceError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        self.start_sync(session).await?;
        self.flush(session).await
    }

    /// Forgets what was in flight; the next session syncs again from the
    /// stored cursor and resends the outbox.
    pub fn session_ended(&mut self) {
        self.mirror.session_ended();
        self.outbox.session_ended();
        self.pending.clear();
    }

    /// Takes the events of the notifications feature and returns every
    /// other event, for the caller to handle. A `FeatureStatus` is used and
    /// returned too.
    pub async fn handle_event<S>(
        &mut self,
        session: &mut Session<S>,
        event: SessionEvent,
    ) -> Result<Option<SessionEvent>, ServiceError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        match event {
            SessionEvent::Reply(reply) => {
                let Some(pending) = self.pending.remove(&reply.request) else {
                    return Ok(Some(SessionEvent::Reply(reply)));
                };
                let (request, last) = (reply.request, reply.last);
                match (pending, reply.into_result()) {
                    (Pending::Sync, Ok(Payload::NotificationSyncBatch(batch))) => {
                        // More batches answer the same request.
                        if !last {
                            self.pending.insert(request, Pending::Sync);
                        }
                        let changes = self.mirror.apply_batch(batch)?;
                        self.apply(session, changes).await?;
                    }
                    (Pending::Sync, Err(refused)) => {
                        tracing::info!(
                            code = refused.code,
                            "the phone refused the notifications sync"
                        );
                        self.mirror.session_ended();
                    }
                    (Pending::Image(sha256), Ok(Payload::ImageData(image))) => {
                        if image.sha256 == sha256 {
                            match self.images.store(&image) {
                                Ok(_) => self.refresh_banners_with(session, &sha256).await?,
                                Err(e) => tracing::warn!(%e, "dropped a notification image"),
                            }
                        }
                    }
                    (Pending::Image(_), Err(_)) => {}
                    (Pending::Outbox(key), Ok(_)) => {
                        self.outbox.acked(&key)?;
                    }
                    (Pending::Outbox(key), Err(refused)) => {
                        self.outbox.refused(&key, &refused, SystemTime::now())?;
                    }
                    _ => return Err(ServiceError::UnexpectedReply),
                }
                Ok(None)
            }
            SessionEvent::RequestTimedOut { request } => match self.pending.remove(&request) {
                Some(Pending::Outbox(key)) => {
                    self.outbox.unanswered(&key, SystemTime::now())?;
                    Ok(None)
                }
                Some(Pending::Sync) => {
                    // Live events stay refused until a new session syncs again.
                    self.mirror.session_ended();
                    Ok(None)
                }
                Some(Pending::Image(_)) => Ok(None),
                None => Ok(Some(SessionEvent::RequestTimedOut { request })),
            },
            SessionEvent::Message {
                payload: Payload::NotificationPosted(posted),
                ..
            } => {
                let change = self.mirror.apply_posted(posted)?;
                self.apply(session, change.into_iter().collect()).await?;
                Ok(None)
            }
            SessionEvent::Message {
                payload: Payload::NotificationRemoved(removed),
                ..
            } => {
                let change = self.mirror.apply_removed(removed)?;
                self.apply(session, change.into_iter().collect()).await?;
                Ok(None)
            }
            SessionEvent::FeatureStatus(status) => {
                if session.is_ready(Feature::Notifications) {
                    self.outbox.feature_ready(Feature::Notifications)?;
                    self.start_sync(session).await?;
                    self.flush(session).await?;
                }
                Ok(Some(SessionEvent::FeatureStatus(status)))
            }
            other => Ok(Some(other)),
        }
    }

    /// Acts on what the user did with a banner. Requests for the phone go
    /// through the outbox, and out at once when `session` is up; an
    /// [`Answer::Open`] is returned for the caller to open the app window.
    pub async fn handle_response<S>(
        &mut self,
        session: Option<&mut Session<S>>,
        response: NotificationResponse,
    ) -> Result<Option<Answer>, ServiceError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let id = match &response {
            NotificationResponse::Action { id, .. }
            | NotificationResponse::Replied { id, .. }
            | NotificationResponse::Closed { id, .. } => *id,
            _ => return Ok(None),
        };
        let Some(key) = self.keys.get(&id).cloned() else {
            return Ok(None);
        };
        if matches!(response, NotificationResponse::Closed { .. }) {
            self.keys.remove(&id);
            self.banners.remove(&key);
        }
        let Some(notification) = self.mirror.get(&key) else {
            return Ok(None);
        };
        match present::answer(notification, &response, self.settings.dismiss_sync) {
            Some(Answer::Request(request)) => {
                self.queue(session, *request).await?;
                Ok(None)
            }
            other => Ok(other),
        }
    }

    /// Dismisses a notification on the phone, from the notifications page.
    pub async fn dismiss<S>(
        &mut self,
        session: Option<&mut Session<S>>,
        key: &str,
    ) -> Result<(), ServiceError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let request = Payload::NotificationDismissRequest(NotificationDismissRequest {
            key: key.to_owned(),
            ..NotificationDismissRequest::default()
        });
        self.queue(session, request).await
    }

    /// Runs one of a notification's actions on the phone, with `reply` for
    /// actions that take text, from the notifications page.
    pub async fn run_action<S>(
        &mut self,
        session: Option<&mut Session<S>>,
        key: &str,
        action_id: &str,
        reply: Option<&str>,
    ) -> Result<(), ServiceError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let request = Payload::NotificationActionRequest(NotificationActionRequest {
            key: key.to_owned(),
            action_id: action_id.to_owned(),
            reply_text: reply.unwrap_or_default().to_owned(),
            ..NotificationActionRequest::default()
        });
        self.queue(session, request).await
    }

    async fn queue<S>(
        &mut self,
        session: Option<&mut Session<S>>,
        request: Payload,
    ) -> Result<(), ServiceError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        self.outbox.push(request, SystemTime::now())?;
        match session {
            Some(session) => self.flush(session).await,
            None => Ok(()),
        }
    }

    async fn start_sync<S>(&mut self, session: &mut Session<S>) -> Result<(), ServiceError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let syncing = self.pending.values().any(|p| matches!(p, Pending::Sync));
        if syncing || !session.is_ready(Feature::Notifications) {
            return Ok(());
        }
        let request = self.mirror.sync_request();
        let seq = session
            .start_request(Payload::NotificationSyncRequest(request))
            .await?;
        self.pending.insert(seq, Pending::Sync);
        Ok(())
    }

    async fn flush<S>(&mut self, session: &mut Session<S>) -> Result<(), ServiceError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let due = self
            .outbox
            .take_due(SystemTime::now(), |feature| session.is_ready(feature));
        for entry in due {
            let seq = session.start_request(entry.request).await?;
            self.pending.insert(seq, Pending::Outbox(entry.key));
        }
        Ok(())
    }

    /// Shows the changes: banners for new alerting notifications, quiet
    /// updates of banners on screen, and closing banners of removed ones.
    async fn apply<S>(
        &mut self,
        session: &mut Session<S>,
        changes: Vec<Change>,
    ) -> Result<(), ServiceError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        for change in changes {
            match &change {
                Change::Posted {
                    notification,
                    alert,
                } => {
                    let on_screen = self.banners.get(&notification.key).copied();
                    if on_screen.is_some() || present::wants_banner(notification, *alert) {
                        let image = self.image_for(session, notification).await?;
                        self.show(notification, on_screen, !*alert, image).await;
                    }
                }
                Change::Removed { key, .. } => {
                    if let Some(id) = self.banners.remove(key) {
                        self.keys.remove(&id);
                        if let Err(e) = self.sink.close(id).await {
                            tracing::warn!(%e, "closing a desktop notification failed");
                        }
                    }
                }
            }
            // No subscriber is fine: the page may not be open.
            let _ = self.changes.send(change);
        }
        Ok(())
    }

    async fn show(
        &mut self,
        notification: &PhoneNotification,
        replaces: Option<NotificationId>,
        quiet: bool,
        image: Option<std::path::PathBuf>,
    ) {
        let capabilities = match self.capabilities {
            Some(capabilities) => capabilities,
            None => {
                let capabilities = self.sink.capabilities().await.unwrap_or_default();
                self.capabilities = Some(capabilities);
                capabilities
            }
        };
        let banner = present::to_desktop(notification, quiet, capabilities, image);
        match self.sink.show(&banner, replaces).await {
            Ok(id) => {
                if let Some(old) = replaces.filter(|old| *old != id) {
                    self.keys.remove(&old);
                }
                self.banners.insert(notification.key.clone(), id);
                self.keys.insert(id, notification.key.clone());
            }
            Err(e) => tracing::warn!(%e, "showing a desktop notification failed"),
        }
    }

    /// The local file of the notification's image, when cached; otherwise
    /// asks the phone for it, and the banner is updated when it arrives.
    async fn image_for<S>(
        &mut self,
        session: &mut Session<S>,
        notification: &PhoneNotification,
    ) -> Result<Option<std::path::PathBuf>, ServiceError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let Some(sha256) = banner_image(notification) else {
            return Ok(None);
        };
        if let Some(path) = self.images.get(sha256) {
            return Ok(Some(path));
        }
        let asked = self
            .pending
            .values()
            .any(|p| matches!(p, Pending::Image(s) if s.as_slice() == sha256));
        if !asked {
            let request = Payload::ImageRequest(ImageCache::request(sha256));
            let seq = session.start_request(request).await?;
            self.pending.insert(seq, Pending::Image(sha256.to_vec()));
        }
        Ok(None)
    }

    /// Shows the image that just arrived in the banners that wait for it.
    async fn refresh_banners_with<S>(
        &mut self,
        session: &mut Session<S>,
        sha256: &[u8],
    ) -> Result<(), ServiceError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let waiting: Vec<(PhoneNotification, NotificationId)> = self
            .banners
            .iter()
            .filter_map(|(key, id)| {
                let notification = self.mirror.get(key)?;
                (banner_image(notification) == Some(sha256)).then(|| (notification.clone(), *id))
            })
            .collect();
        for (notification, id) in waiting {
            let image = self.image_for(session, &notification).await?;
            self.show(&notification, Some(id), true, image).await;
        }
        Ok(())
    }
}

/// The image a banner shows: the sender's photo, else the app's icon.
fn banner_image(notification: &PhoneNotification) -> Option<&[u8]> {
    [&notification.large_icon, &notification.app_icon]
        .into_iter()
        .flatten()
        .map(|image| image.sha256.as_slice())
        .find(|sha256| sha256.len() == 32)
}
