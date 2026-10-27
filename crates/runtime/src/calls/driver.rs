//! The calls feature of one phone, in its session loop.

use std::sync::Arc;

use link2linux_core::calls::{CallsService, Handled};
use link2linux_core::session::{SessionError, SessionEvent};

use super::Calls;
use crate::notify::PhoneSession;

/// Carries the calls feature in one phone's sessions: the desktop's
/// availability, and the request to pair while the user pairs this phone
/// over Bluetooth. The calls themselves go through [`Calls`], which every
/// phone's driver shares.
pub struct CallsDriver {
    device_id: String,
    calls: Arc<Calls>,
    service: CallsService,
}

impl CallsDriver {
    /// The driver for phone `device_id`.
    #[must_use]
    pub fn new(device_id: impl Into<String>, calls: Arc<Calls>) -> Self {
        Self {
            device_id: device_id.into(),
            calls,
            service: CallsService::new(),
        }
    }

    /// Whether the desktop shows this phone's calls itself, so its call
    /// notifications get no banners of their own.
    #[must_use]
    pub fn shows_calls(&self) -> bool {
        self.calls.shows_calls(&self.device_id)
    }

    /// The session is up: reports the desktop's availability, and asks
    /// the phone to pair if the user is pairing it.
    ///
    /// # Errors
    ///
    /// Fails when the session fails.
    pub async fn session_started(
        &mut self,
        session: &mut PhoneSession,
    ) -> Result<(), SessionError> {
        self.service
            .session_started(session, self.calls.availability())
            .await?;
        self.calls.session_started(&self.device_id);
        // The phone is around: its hands-free link may come up too.
        self.calls.phone_connected(&self.device_id);
        self.ask_to_pair(session).await
    }

    /// Takes the events of the calls feature and returns the rest.
    ///
    /// # Errors
    ///
    /// Fails when the session fails.
    pub async fn handle_event(
        &mut self,
        session: &mut PhoneSession,
        event: SessionEvent,
    ) -> Result<Option<SessionEvent>, SessionError> {
        match self.service.handle_event(session, event).await? {
            Handled::NotMine(event @ SessionEvent::FeatureStatus(_)) => {
                // Calls may have become ready on the phone.
                self.ask_to_pair(session).await?;
                Ok(Some(event))
            }
            Handled::NotMine(event) => Ok(Some(event)),
            Handled::Taken => Ok(None),
            Handled::Answered(answer) => {
                self.calls.phone_answered(&self.device_id, answer);
                Ok(None)
            }
        }
    }

    /// The desktop's calls changed: its availability, or the pairing.
    /// `session` is `None` between sessions.
    ///
    /// # Errors
    ///
    /// Fails when the session fails.
    pub async fn changed(
        &mut self,
        session: Option<&mut PhoneSession>,
    ) -> Result<(), SessionError> {
        let Some(session) = session else {
            return Ok(());
        };
        self.service
            .availability_changed(session, self.calls.availability())
            .await?;
        self.ask_to_pair(session).await
    }

    /// The session is over.
    pub fn session_ended(&mut self) {
        self.calls.session_ended(&self.device_id);
        if let Some(answer) = self.service.session_ended() {
            self.calls.phone_answered(&self.device_id, answer);
        }
    }

    async fn ask_to_pair(&mut self, session: &mut PhoneSession) -> Result<(), SessionError> {
        let Some(request) = self.calls.request_for(&self.device_id) else {
            return Ok(());
        };
        if self.service.ask_to_pair(session, request).await? {
            self.calls.phone_answered(
                &self.device_id,
                link2linux_core::calls::PhoneAnswer::Waiting,
            );
        }
        Ok(())
    }
}
