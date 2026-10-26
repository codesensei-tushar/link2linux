//! The features of one phone, which its session loop drives.
//!
//! A feature joins the session loop here: a field for its driver in
//! [`Features`], a line in [`Features::session_started`] and in
//! [`Features::session_ended`], and two in [`Features::handle_event`]: one
//! for the `FeatureStatus` every feature gets, one in the chain of the
//! other events. Requests the
//! user makes of a feature, such as dismissing a notification, are
//! [`Command`](crate::link::Command)s, which the loop hands to its driver.

use link2linux_core::session::SessionEvent;

use crate::clipboard::ClipboardDriver;
use crate::notify::{DriverError, NotificationsDriver, PhoneSession};

/// The features of one phone, which stay with it across sessions.
pub struct Features {
    /// Its notifications.
    pub notifications: Box<dyn NotificationsDriver>,
    /// Its part of the shared clipboard.
    pub clipboard: ClipboardDriver,
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
            self.clipboard_event(session, SessionEvent::FeatureStatus(states))
                .await;
            return Ok(Some(event));
        }
        let Some(event) = self.notifications_event(session, event).await? else {
            return Ok(None);
        };
        Ok(self.clipboard_event(session, event).await)
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

    /// The session is over: each feature forgets what only held within it.
    pub fn session_ended(&mut self) {
        self.notifications.session_ended();
        self.clipboard.session_ended();
    }
}
