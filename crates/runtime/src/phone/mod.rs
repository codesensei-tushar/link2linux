//! Each phone's status and ringing it.
//!
//! The core's [`StatusService`] and [`RingService`] apply the protocol's
//! rules. Each phone's [`StatusDriver`] and [`RingDriver`] run one in its
//! session loop and put what the UI shows on the [`PhoneBoards`], which
//! keep it for the snapshots the shell asks for and report each change.
//! [`dto`] has what the UI receives.

pub mod dto;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use link2linux_core::ring::RingService;
use link2linux_core::session::{Availability, SessionError, SessionEvent};
use link2linux_core::status::StatusService;
use tokio::time::Instant;

pub use dto::{PhoneStatus, RingStatus};

use crate::link::lock;
use crate::notify::PhoneSession;
use crate::view::EventSink;

/// What the user asks of a phone's status or ringing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhoneCommand {
    /// Make the phone ring.
    Ring,
    /// Stop the phone ringing.
    StopRinging,
    /// The user showed or hid the phones' status on this desktop.
    StatusSwitch,
}

/// What the UI shows of one phone.
#[derive(Debug, Clone, Default)]
struct Board {
    status: PhoneStatus,
    ring: RingStatus,
}

/// What the UI shows of every paired phone's status and ringing, and the
/// user's switch for the status. Shared by every phone's drivers.
pub struct PhoneBoards {
    events: Arc<dyn EventSink>,
    /// The user shows the phones' status on this desktop.
    shown: AtomicBool,
    boards: Mutex<HashMap<String, Board>>,
}

impl PhoneBoards {
    /// Boards that report to `events`, with the status shown when `shown`.
    #[must_use]
    pub fn new(events: Arc<dyn EventSink>, shown: bool) -> Self {
        Self {
            events,
            shown: AtomicBool::new(shown),
            boards: Mutex::default(),
        }
    }

    /// Whether the user shows the phones' status on this desktop.
    #[must_use]
    pub fn is_shown(&self) -> bool {
        self.shown.load(Ordering::Relaxed)
    }

    /// Shows or hides the phones' status. Returns whether that changed.
    pub fn set_shown(&self, shown: bool) -> bool {
        self.shown.swap(shown, Ordering::Relaxed) != shown
    }

    /// What this desktop reports for the status feature: `DISABLED` while
    /// the user hides it.
    #[must_use]
    pub fn status_availability(&self) -> Availability {
        if self.is_shown() {
            Availability::Ready
        } else {
            Availability::Disabled
        }
    }

    /// Phone `id`'s status as the UI shows it.
    #[must_use]
    pub fn status(&self, id: &str) -> PhoneStatus {
        lock(&self.boards)
            .get(id)
            .map(|board| board.status.clone())
            .unwrap_or_default()
    }

    /// Ringing phone `id` as the UI shows it.
    #[must_use]
    pub fn ring(&self, id: &str) -> RingStatus {
        lock(&self.boards)
            .get(id)
            .map(|board| board.ring)
            .unwrap_or_default()
    }

    /// Forgets phone `id`, which is no longer paired.
    pub fn forget(&self, id: &str) {
        lock(&self.boards).remove(id);
    }

    /// Records phone `id`'s status, and reports it if it changed. The lock
    /// stays held while the sink runs, so snapshots arrive in order.
    fn set_status(&self, id: &str, status: PhoneStatus) {
        let mut boards = lock(&self.boards);
        let board = boards.entry(id.to_owned()).or_default();
        if board.status != status {
            board.status = status;
            self.events.phone_status_changed(id, &board.status);
        }
    }

    /// Records ringing phone `id`, and reports it if it changed.
    fn set_ring(&self, id: &str, ring: RingStatus) {
        let mut boards = lock(&self.boards);
        let board = boards.entry(id.to_owned()).or_default();
        if board.ring != ring {
            board.ring = ring;
            self.events.ring_changed(id, &board.ring);
        }
    }
}

/// One phone's status, as its session loop runs it.
pub struct StatusDriver {
    device_id: String,
    service: StatusService,
    boards: Arc<PhoneBoards>,
}

impl StatusDriver {
    /// The driver for phone `device_id`, which shows on `boards`.
    #[must_use]
    pub fn new(device_id: impl Into<String>, boards: Arc<PhoneBoards>) -> Self {
        Self {
            device_id: device_id.into(),
            service: StatusService::new(),
            boards,
        }
    }

    /// See [`StatusService::session_started`].
    ///
    /// # Errors
    ///
    /// Fails when the session fails.
    pub async fn session_started(
        &mut self,
        session: &mut PhoneSession,
    ) -> Result<(), SessionError> {
        let started = self.service.session_started(session).await;
        self.publish();
        started
    }

    /// See [`StatusService::handle_event`].
    ///
    /// # Errors
    ///
    /// Fails when the session fails.
    pub async fn handle_event(
        &mut self,
        session: &mut PhoneSession,
        event: SessionEvent,
    ) -> Result<Option<SessionEvent>, SessionError> {
        let rest = self.service.handle_event(session, event).await;
        self.publish();
        rest
    }

    /// The user showed or hid the status: tells the phone, and asks for the
    /// status again once it is shown.
    ///
    /// # Errors
    ///
    /// Fails when the session fails.
    pub async fn switched(&mut self, session: &mut PhoneSession) -> Result<(), SessionError> {
        let switched = self
            .service
            .set_shown(session, self.boards.is_shown())
            .await;
        self.publish();
        switched
    }

    /// See [`StatusService::session_ended`].
    pub fn session_ended(&mut self) {
        self.service.session_ended();
        self.publish();
    }

    fn publish(&self) {
        self.boards
            .set_status(&self.device_id, dto::status(self.service.state()));
    }
}

/// Ringing one phone, as its session loop runs it.
pub struct RingDriver {
    device_id: String,
    service: RingService,
    boards: Arc<PhoneBoards>,
}

impl RingDriver {
    /// The driver for phone `device_id`, which shows on `boards`.
    #[must_use]
    pub fn new(device_id: impl Into<String>, boards: Arc<PhoneBoards>) -> Self {
        Self {
            device_id: device_id.into(),
            service: RingService::new(),
            boards,
        }
    }

    /// See [`RingService::session_started`].
    pub fn session_started(&mut self, session: &PhoneSession) {
        self.service.session_started(session);
        self.publish();
    }

    /// See [`RingService::handle_event`].
    pub fn handle_event(
        &mut self,
        session: &PhoneSession,
        event: SessionEvent,
    ) -> Option<SessionEvent> {
        let rest = self.service.handle_event(session, event);
        self.publish();
        rest
    }

    /// Acts on what the user asked. `session` is `None` between sessions,
    /// when a ring tells the user the phone is not connected.
    ///
    /// # Errors
    ///
    /// Fails when the session fails.
    pub async fn command(
        &mut self,
        session: Option<&mut PhoneSession>,
        command: PhoneCommand,
    ) -> Result<(), SessionError> {
        let done = match (command, session) {
            (PhoneCommand::Ring, Some(session)) => self.service.ring(session).await,
            (PhoneCommand::Ring, None) => {
                self.service.not_connected();
                Ok(())
            }
            (PhoneCommand::StopRinging, Some(session)) => self.service.stop(session).await,
            (PhoneCommand::StopRinging | PhoneCommand::StatusSwitch, _) => Ok(()),
        };
        self.publish();
        done
    }

    /// See [`RingService::next_deadline`].
    #[must_use]
    pub fn next_deadline(&self) -> Option<Instant> {
        self.service.next_deadline()
    }

    /// See [`RingService::tick`].
    pub fn tick(&mut self, now: Instant) {
        self.service.tick(now);
        self.publish();
    }

    /// See [`RingService::session_ended`].
    pub fn session_ended(&mut self) {
        self.service.session_ended();
        self.publish();
    }

    fn publish(&self) {
        self.boards
            .set_ring(&self.device_id, dto::ring(self.service.state()));
    }
}
