//! The features of one phone, which its session loop drives.
//!
//! A feature joins the session loop here: a field for its driver in
//! [`Features`], and a line in each of [`Features::session_started`],
//! [`Features::handle_event`] and [`Features::session_ended`]. Requests the
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
    /// and comes back too.
    ///
    /// # Errors
    ///
    /// Fails when the phone broke a feature's rules.
    pub async fn handle_event(
        &mut self,
        session: &mut PhoneSession,
        event: SessionEvent,
    ) -> Result<Option<SessionEvent>, BrokeRules> {
        let rest = match self.notifications.handle_event(session, event).await {
            Ok(rest) => rest,
            Err(DriverError::PhoneBrokeRules(error)) => {
                tracing::warn!(%error, "the phone broke the notification rules");
                return Err(BrokeRules);
            }
            Err(error) => {
                tracing::warn!(%error, "notification event failed");
                None
            }
        };
        let Some(event) = rest else {
            return Ok(None);
        };
        Ok(self
            .clipboard
            .handle_event(session, event)
            .await
            .unwrap_or_else(|error| {
                tracing::debug!(%error, "clipboard event failed");
                None
            }))
    }

    /// The session is over: each feature forgets what only held within it.
    pub fn session_ended(&mut self) {
        self.notifications.session_ended();
        self.clipboard.session_ended();
    }
}
