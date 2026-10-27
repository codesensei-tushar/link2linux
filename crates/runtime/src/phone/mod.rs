//! Each phone's status and ringing it.
//!
//! The core's [`StatusService`] and [`RingService`] apply the protocol's
//! rules. Each phone's [`StatusDriver`] and [`RingDriver`] run one in its
//! session loop and put what the UI shows on the phone's [`PhoneBoard`].
//! The [`PhoneBoards`] keep every phone's, for the snapshots the shell asks
//! for, and report each change. [`dto`] has what the UI receives.

pub mod dto;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
    /// The [`PhoneBoard`] that writes it: the drivers of an earlier task
    /// for the phone, still ending, no longer do.
    owner: u64,
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
    next_owner: AtomicU64,
}

impl PhoneBoards {
    /// Boards that report to `events`, with the status shown when `shown`.
    #[must_use]
    pub fn new(events: Arc<dyn EventSink>, shown: bool) -> Self {
        Self {
            events,
            shown: AtomicBool::new(shown),
            boards: Mutex::default(),
            next_owner: AtomicU64::new(0),
        }
    }

    /// A new board for phone `id`, which the drivers of its new task write
    /// to. What the drivers of an earlier task write from then on is
    /// dropped.
    #[must_use]
    pub fn open(self: &Arc<Self>, id: &str) -> PhoneBoard {
        let owner = self.next_owner.fetch_add(1, Ordering::Relaxed);
        lock(&self.boards).insert(
            id.to_owned(),
            Board {
                owner,
                ..Board::default()
            },
        );
        PhoneBoard {
            boards: self.clone(),
            device_id: id.to_owned(),
            owner,
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

    /// Forgets phone `id`, which is no longer paired: what its drivers
    /// write from then on is dropped.
    pub fn forget(&self, id: &str) {
        lock(&self.boards).remove(id);
    }
}

/// One phone's board, as its drivers write it.
#[derive(Clone)]
pub struct PhoneBoard {
    boards: Arc<PhoneBoards>,
    device_id: String,
    owner: u64,
}

impl PhoneBoard {
    /// Whether the user shows the phones' status on this desktop.
    #[must_use]
    pub fn is_shown(&self) -> bool {
        self.boards.is_shown()
    }

    /// Records the phone's status, and reports it if it changed. The lock
    /// stays held while the sink runs, so snapshots arrive in order.
    fn set_status(&self, status: PhoneStatus) {
        let mut boards = lock(&self.boards.boards);
        if let Some(board) = boards.get_mut(&self.device_id)
            && board.owner == self.owner
            && board.status != status
        {
            board.status = status;
            self.boards
                .events
                .phone_status_changed(&self.device_id, &board.status);
        }
    }

    /// Records ringing the phone, and reports it if it changed.
    fn set_ring(&self, ring: RingStatus) {
        let mut boards = lock(&self.boards.boards);
        if let Some(board) = boards.get_mut(&self.device_id)
            && board.owner == self.owner
            && board.ring != ring
        {
            board.ring = ring;
            self.boards
                .events
                .ring_changed(&self.device_id, &board.ring);
        }
    }
}

/// One phone's status, as its session loop runs it.
pub struct StatusDriver {
    service: StatusService,
    board: PhoneBoard,
}

impl StatusDriver {
    /// The driver for the phone of `board`, which shows on it.
    #[must_use]
    pub fn new(board: PhoneBoard) -> Self {
        Self {
            service: StatusService::new(),
            board,
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
        let shown = self.board.is_shown();
        let switched = self.service.set_shown(session, shown).await;
        self.publish();
        switched
    }

    /// See [`StatusService::session_ended`].
    pub fn session_ended(&mut self) {
        self.service.session_ended();
        self.publish();
    }

    fn publish(&self) {
        self.board.set_status(dto::status(self.service.state()));
    }
}

/// Ringing one phone, as its session loop runs it.
pub struct RingDriver {
    service: RingService,
    board: PhoneBoard,
}

impl RingDriver {
    /// The driver for the phone of `board`, which shows on it.
    #[must_use]
    pub fn new(board: PhoneBoard) -> Self {
        Self {
            service: RingService::new(),
            board,
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
        self.board.set_ring(dto::ring(self.service.state()));
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex as StdMutex;

    use super::*;
    use crate::view::{Device, PairingState, RingPhase};

    #[derive(Default)]
    struct Rings(StdMutex<Vec<(String, RingStatus)>>);

    impl EventSink for Rings {
        fn pairing_changed(&self, _state: &PairingState) {}
        fn devices_changed(&self, _devices: &[Device]) {}
        fn ring_changed(&self, device_id: &str, ring: &RingStatus) {
            self.0.lock().unwrap().push((device_id.to_owned(), *ring));
        }
    }

    fn ringing() -> RingStatus {
        RingStatus {
            phase: RingPhase::Ringing,
            blocked: None,
            ..RingStatus::default()
        }
    }

    #[test]
    fn only_the_newest_task_of_a_phone_writes_its_board() {
        let sink = Arc::new(Rings::default());
        let boards = Arc::new(PhoneBoards::new(sink.clone(), true));
        let old = boards.open("phone");
        let new = boards.open("phone");

        new.set_ring(ringing());
        // The old task ends after the new one started: nothing changes.
        old.set_ring(RingStatus::default());
        assert_eq!(boards.ring("phone"), ringing());
        assert_eq!(*sink.0.lock().unwrap(), [("phone".to_owned(), ringing())]);

        // Unpaired: nothing writes it again, and it reads as away.
        boards.forget("phone");
        new.set_ring(ringing());
        assert_eq!(boards.ring("phone"), RingStatus::default());
        assert_eq!(sink.0.lock().unwrap().len(), 1);
    }

    #[test]
    fn the_status_switch_sets_what_the_desktop_reports() {
        let boards = PhoneBoards::new(Arc::new(Rings::default()), true);
        assert_eq!(boards.status_availability(), Availability::Ready);
        assert!(!boards.set_shown(true));
        assert!(boards.set_shown(false));
        assert_eq!(boards.status_availability(), Availability::Disabled);
        assert_eq!(boards.status("phone"), PhoneStatus::Disconnected);
    }
}
