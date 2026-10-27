//! The calls feature of one phone, in its session loop.

use std::sync::Arc;

use link2linux_core::calls::{CallsService, Handled, PhoneAnswer};
use link2linux_core::device::Feature;
use link2linux_core::session::{Availability, SessionError, SessionEvent};

use super::Calls;
use crate::notify::PhoneSession;
use crate::phone::PhoneBoard;

/// Carries the calls feature in one phone's sessions: the desktop's
/// availability, and the request to pair while the user pairs this phone
/// over Bluetooth. The calls themselves go through [`Calls`], which every
/// phone's driver shares.
///
/// What it tells [`Calls`] about the phone counts only while its task is
/// the phone's current one: a replaced task ends its session after its
/// successor started, and must not undo what the successor reported.
pub struct CallsDriver {
    device_id: String,
    calls: Arc<Calls>,
    service: CallsService,
    /// The owner token of this driver's task.
    owner: PhoneBoard,
    /// Whether this session found calls ready on the phone yet.
    phone_ready: bool,
}

impl CallsDriver {
    /// The driver for phone `device_id`, in the task that `owner` belongs
    /// to.
    #[must_use]
    pub fn new(device_id: impl Into<String>, calls: Arc<Calls>, owner: PhoneBoard) -> Self {
        Self {
            device_id: device_id.into(),
            calls,
            service: CallsService::new(),
            owner,
            phone_ready: false,
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
        if self.owner.is_current() {
            self.calls.session_started(&self.device_id);
        }
        self.phone_status(session);
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
                // Calls may have come on or gone off on the phone.
                self.phone_status(session);
                self.ask_to_pair(session).await?;
                Ok(Some(event))
            }
            Handled::NotMine(event) => Ok(Some(event)),
            Handled::Taken => Ok(None),
            Handled::Answered(answer) => {
                self.answered(answer);
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
        let answer = self.service.session_ended();
        // The next session finds the phone's calls afresh. What this one
        // reported still counts meanwhile: the hands-free link may stay up
        // while the app is away.
        self.phone_ready = false;
        if !self.owner.is_current() {
            return;
        }
        self.calls.session_ended(&self.device_id);
        if let Some(answer) = answer {
            self.calls.phone_answered(&self.device_id, answer);
        }
    }

    /// Reports whether calls are ready on the phone, once its status says.
    /// The first time in a session that they are, the phone counts as
    /// connected with calls on, and its hands-free link may come up too.
    fn phone_status(&mut self, session: &PhoneSession) {
        let Some(ready) = calls_ready(session) else {
            return;
        };
        if !self.owner.is_current() {
            return;
        }
        self.calls.phone_ready(&self.device_id, ready);
        if ready && !self.phone_ready {
            self.phone_ready = true;
            self.calls.phone_connected(&self.device_id);
        }
    }

    /// Reports what the phone made of the pairing request.
    fn answered(&self, answer: PhoneAnswer) {
        if self.owner.is_current() {
            self.calls.phone_answered(&self.device_id, answer);
        }
    }

    async fn ask_to_pair(&mut self, session: &mut PhoneSession) -> Result<(), SessionError> {
        let Some(request) = self.calls.request_for(&self.device_id) else {
            return Ok(());
        };
        if self.service.ask_to_pair(session, request).await? {
            self.answered(PhoneAnswer::Waiting);
        }
        Ok(())
    }
}

/// Whether calls are ready on the phone of `session`, or `None` until its
/// first status arrives. A phone without the feature has none to offer.
fn calls_ready(session: &PhoneSession) -> Option<bool> {
    if !session.shared_features().contains(&Feature::Calls) {
        return Some(false);
    }
    let states = session.peer_availability()?;
    Some(
        states
            .get(&Feature::Calls)
            .is_some_and(Availability::is_ready),
    )
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use link2linux_core::calls::CallNotification;
    use link2linux_protocol::v1::{
        NotificationAction, NotificationActionKind, NotificationCategory, PhoneNotification,
    };

    use super::*;
    use crate::calls::center::Phone;
    use crate::link::lock;
    use crate::phone::PhoneBoards;
    use crate::view::{Device, EventSink, PairingState};

    struct NoEvents;

    impl EventSink for NoEvents {
        fn pairing_changed(&self, _state: &PairingState) {}
        fn devices_changed(&self, _devices: &[Device]) {}
    }

    fn ringing() -> CallNotification {
        CallNotification::of(&PhoneNotification {
            key: "k1".into(),
            title: "Anna".into(),
            category: NotificationCategory::Call.into(),
            actions: vec![NotificationAction {
                id: "answer".into(),
                kind: NotificationActionKind::AnswerCall.into(),
                ..NotificationAction::default()
            }],
            ..PhoneNotification::default()
        })
        .unwrap()
    }

    #[test]
    fn a_replaced_task_ending_late_leaves_the_phones_calls_alone() {
        let calls = Arc::new(Calls::new(
            None,
            true,
            PathBuf::from("/nonexistent/calls.json"),
        ));
        calls.set_phones(vec![("p1".into(), "Pixel".into())]);
        let boards = Arc::new(PhoneBoards::new(Arc::new(NoEvents), true));
        let mut old = CallsDriver::new("p1", calls.clone(), boards.open("p1"));
        lock(&calls.center).set_note("p1", "k1", Some(ringing()));
        calls.session_started("p1");

        // A new connection replaces the old task, whose session ends after
        // the new one started.
        let _new = CallsDriver::new("p1", calls.clone(), boards.open("p1"));
        calls.session_started("p1");
        old.session_ended();

        let phones = [Phone {
            id: "p1".into(),
            name: "Pixel".into(),
        }];
        assert_eq!(lock(&calls.center).calls_of(&phones[0], &phones).len(), 1);
    }
}
