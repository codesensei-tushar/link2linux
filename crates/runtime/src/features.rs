//! The features of one phone, which its session loop drives.
//!
//! A feature joins the session loop here: a field for its driver in
//! [`Features`], a line in [`Features::session_started`] and in
//! [`Features::session_ended`], and two in [`Features::handle_event`]: one
//! for the `FeatureStatus` every feature gets, one in the chain of the
//! other events. Requests the
//! user makes of a feature, such as dismissing a notification, are
//! [`Command`](crate::link::Command)s, which the loop hands to its driver.
//! A feature with timers of its own reports them through
//! [`Features::next_deadline`], and the loop calls [`Features::tick`] then;
//! banner responses go through [`Features::handle_response`]. What the user
//! asks of the phone's status or ringing goes through
//! [`Features::phone_command`], and the desktop's calls tell each phone of a
//! change through [`Features::calls_changed`].

use link2linux_core::session::{SessionError, SessionEvent};
use link2linux_platform::NotificationResponse;
use tokio::time::Instant;

use crate::calls::CallsDriver;
use crate::clipboard::ClipboardDriver;
use crate::files::FilesDriver;
use crate::notify::{DriverError, NotificationsDriver, PhoneSession};
use crate::phone::{PhoneCommand, RingDriver, StatusDriver};
use crate::photos::{PhotoCommand, PhotosDriver};
use crate::screen::{ScreenCommand, ScreenDriver};

/// The features of one phone, which stay with it across sessions.
pub struct Features {
    /// Its notifications.
    pub notifications: Box<dyn NotificationsDriver>,
    /// Its part of the shared clipboard.
    pub clipboard: ClipboardDriver,
    /// Its file transfers.
    pub files: FilesDriver,
    /// Its status.
    pub status: StatusDriver,
    /// Ringing it.
    pub ring: RingDriver,
    /// Its calls.
    pub calls: CallsDriver,
    /// Its screen.
    pub screen: ScreenDriver,
    /// Its recent photos.
    pub photos: PhotosDriver,
}

/// The phone broke a feature's rules: the session ends as a protocol error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BrokeRules;

impl Features {
    /// The session is up: each feature starts. A feature that cannot start
    /// stays out of this session; the others go on.
    pub async fn session_started(&mut self, session: &mut PhoneSession) {
        if let Err(error) = self.notifications.session_started(session).await {
            tracing::warn!(%error, "notifications could not start");
        }
        if let Err(error) = self.clipboard.session_started(session).await {
            tracing::debug!(%error, "the clipboard could not start");
        }
        if let Err(error) = self.files.session_started(session).await {
            tracing::debug!(%error, "files could not start");
        }
        if let Err(error) = self.status.session_started(session).await {
            tracing::debug!(%error, "the phone's status could not start");
        }
        self.ring.session_started(session);
        self.screen.session_started(session);
        if let Err(error) = self.calls.session_started(session).await {
            tracing::debug!(%error, "calls could not start");
        }
        if let Err(error) = self.photos.session_started(session).await {
            tracing::debug!(%error, "photos could not start");
        }
    }

    /// Hands `event` to each feature in turn, and returns what none of them
    /// took, for the session loop. A `FeatureStatus` goes to every feature
    /// and comes back too, whatever a feature makes of it.
    ///
    /// # Errors
    ///
    /// Fails when the phone broke a feature's rules.
    pub async fn handle_event(
        &mut self,
        session: &mut PhoneSession,
        event: SessionEvent,
    ) -> Result<Option<SessionEvent>, BrokeRules> {
        if let SessionEvent::FeatureStatus(states) = &event {
            let states = states.clone();
            self.notifications_event(session, SessionEvent::FeatureStatus(states.clone()))
                .await?;
            self.clipboard_event(session, SessionEvent::FeatureStatus(states.clone()))
                .await;
            self.files_event(session, SessionEvent::FeatureStatus(states.clone()))
                .await;
            self.calls_event(session, SessionEvent::FeatureStatus(states.clone()))
                .await;
            self.status_event(session, SessionEvent::FeatureStatus(states.clone()))
                .await;
            self.screen_event(session, SessionEvent::FeatureStatus(states.clone()));
            self.photos_event(session, SessionEvent::FeatureStatus(states.clone()))
                .await;
            self.ring
                .handle_event(session, SessionEvent::FeatureStatus(states));
            return Ok(Some(event));
        }
        let Some(event) = self.notifications_event(session, event).await? else {
            return Ok(None);
        };
        let Some(event) = self.clipboard_event(session, event).await else {
            return Ok(None);
        };
        let Some(event) = self.files_event(session, event).await else {
            return Ok(None);
        };
        let Some(event) = self.status_event(session, event).await else {
            return Ok(None);
        };
        let Some(event) = self.ring.handle_event(session, event) else {
            return Ok(None);
        };
        let Some(event) = self.screen_event(session, event) else {
            return Ok(None);
        };
        let Some(event) = self.photos_event(session, event).await else {
            return Ok(None);
        };
        Ok(self.calls_event(session, event).await)
    }

    async fn notifications_event(
        &mut self,
        session: &mut PhoneSession,
        event: SessionEvent,
    ) -> Result<Option<SessionEvent>, BrokeRules> {
        match self.notifications.handle_event(session, event).await {
            Ok(rest) => Ok(rest),
            Err(DriverError::PhoneBrokeRules(error)) => {
                tracing::warn!(%error, "the phone broke the notification rules");
                Err(BrokeRules)
            }
            Err(error) => {
                tracing::warn!(%error, "notification event failed");
                Ok(None)
            }
        }
    }

    async fn clipboard_event(
        &mut self,
        session: &mut PhoneSession,
        event: SessionEvent,
    ) -> Option<SessionEvent> {
        self.clipboard
            .handle_event(session, event)
            .await
            .unwrap_or_else(|error| {
                tracing::debug!(%error, "clipboard event failed");
                None
            })
    }

    async fn files_event(
        &mut self,
        session: &mut PhoneSession,
        event: SessionEvent,
    ) -> Option<SessionEvent> {
        self.files
            .handle_event(session, event)
            .await
            .unwrap_or_else(|error| {
                tracing::debug!(%error, "files event failed");
                None
            })
    }

    async fn status_event(
        &mut self,
        session: &mut PhoneSession,
        event: SessionEvent,
    ) -> Option<SessionEvent> {
        self.status
            .handle_event(session, event)
            .await
            .unwrap_or_else(|error| {
                tracing::debug!(%error, "status event failed");
                None
            })
    }

    fn screen_event(
        &mut self,
        session: &mut PhoneSession,
        event: SessionEvent,
    ) -> Option<SessionEvent> {
        self.screen
            .handle_event(session, event)
            .unwrap_or_else(|error| {
                tracing::debug!(%error, "screen event failed");
                None
            })
    }

    /// Acts on what the user asked of the phone's screen. `session` is
    /// `None` between sessions.
    pub fn screen_command(&mut self, session: Option<&mut PhoneSession>, command: ScreenCommand) {
        if let Err(error) = self.screen.command(session, command) {
            tracing::debug!(%error, "could not act on the phone's screen");
        }
    }

    async fn photos_event(
        &mut self,
        session: &mut PhoneSession,
        event: SessionEvent,
    ) -> Option<SessionEvent> {
        self.photos
            .handle_event(session, event)
            .await
            .unwrap_or_else(|error| {
                tracing::debug!(%error, "photos event failed");
                None
            })
    }

    /// Hands what the shell asked of the phone's photos to its driver.
    /// `session` is `None` between sessions, when it is answered at once.
    pub fn photo_command(&mut self, session: Option<&mut PhoneSession>, command: PhotoCommand) {
        if let Err(error) = self.photos.command(session, command) {
            tracing::debug!(%error, "could not ask the phone for a photo");
        }
    }

    /// Acts on what the user asked of the phone's status or ringing.
    /// `session` is `None` between sessions.
    pub async fn phone_command(
        &mut self,
        session: Option<&mut PhoneSession>,
        command: PhoneCommand,
    ) {
        let done = match (command, session) {
            (PhoneCommand::StatusSwitch, Some(session)) => self.status.switched(session).await,
            // The next session starts with the new setting.
            (PhoneCommand::StatusSwitch, None) => Ok(()),
            (command, session) => self.ring.command(session, command).await,
        };
        if let Err(error) = done {
            tracing::debug!(%error, ?command, "could not act on the phone");
        }
    }

    async fn calls_event(
        &mut self,
        session: &mut PhoneSession,
        event: SessionEvent,
    ) -> Option<SessionEvent> {
        self.calls
            .handle_event(session, event)
            .await
            .unwrap_or_else(|error| {
                tracing::debug!(%error, "calls event failed");
                None
            })
    }

    /// The desktop's calls changed. The phone's call notifications get
    /// banners of their own only while the desktop does not show its calls
    /// itself. `session` is `None` between sessions.
    ///
    /// # Errors
    ///
    /// Fails when the session fails.
    pub async fn calls_changed(
        &mut self,
        session: Option<&mut PhoneSession>,
    ) -> Result<(), SessionError> {
        self.notifications
            .set_call_banners(!self.calls.shows_calls())
            .await;
        self.calls.changed(session).await
    }

    /// Acts on what the user did with a banner: each feature looks for its
    /// own. `session` is `None` between sessions.
    ///
    /// # Errors
    ///
    /// Fails when the phone broke a feature's rules.
    pub async fn handle_response(
        &mut self,
        session: Option<&mut PhoneSession>,
        response: NotificationResponse,
    ) -> Result<(), BrokeRules> {
        if self.files.handle_response(&response) {
            return Ok(());
        }
        match self.notifications.handle_response(session, response).await {
            Ok(Some(_)) => tracing::debug!("a banner asks to open the app"),
            Ok(None) => {}
            Err(DriverError::PhoneBrokeRules(error)) => {
                tracing::warn!(%error, "the phone broke the notification rules");
                return Err(BrokeRules);
            }
            Err(error) => tracing::warn!(%error, "notification response failed"),
        }
        Ok(())
    }

    /// When a feature next has something to do by itself, such as ending a
    /// transfer kept too long.
    #[must_use]
    pub fn next_deadline(&self) -> Option<Instant> {
        [
            self.files.next_deadline(),
            self.ring.next_deadline(),
            self.status.next_deadline(),
            self.screen.next_deadline(),
            self.photos.next_deadline(),
        ]
        .into_iter()
        .flatten()
        .min()
    }

    /// The time [`Self::next_deadline`] named has come. `session` is `None`
    /// between sessions.
    pub async fn tick(&mut self, session: Option<&mut PhoneSession>) {
        let mut session = session;
        if let Err(error) = self.files.tick(session.as_deref_mut()).await {
            tracing::debug!(%error, "files could not go on");
        }
        if let Some(session) = session.as_deref_mut()
            && let Err(error) = self.screen.tick(session)
        {
            tracing::debug!(%error, "could not ask for a key frame");
        }
        if let Some(session) = session {
            if let Err(error) = self.status.tick(session).await {
                tracing::debug!(%error, "the phone's status could not be asked for again");
            }
            if let Err(error) = self.photos.tick(session).await {
                tracing::debug!(%error, "photos could not go on");
            }
        }
        self.ring.tick(Instant::now());
    }

    /// Whether something the user sees happen is going on, such as a file
    /// transfer, so sessions ping often.
    #[must_use]
    pub fn is_busy(&self) -> bool {
        self.files.is_busy()
    }

    /// The session is over: each feature forgets what only held within it.
    pub fn session_ended(&mut self) {
        self.notifications.session_ended();
        self.clipboard.session_ended();
        self.files.session_ended();
        self.status.session_ended();
        self.ring.session_ended();
        self.calls.session_ended();
        self.screen.session_ended();
        self.photos.session_ended();
    }
}
