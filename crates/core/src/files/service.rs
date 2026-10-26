//! Both sides of the files feature for one phone, driven by a [`Session`].

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use link2linux_protocol::Payload;
use link2linux_protocol::v1::{
    Ack, ErrorCode, ErrorReply, FileAccept, FileCancel, FileChunk, FileOffer,
};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::broadcast;
use tokio::time::Instant;

use super::disk::{self, SourceError, blocking};
use super::incoming::{Incoming, WriteError, Written};
use super::names::MAX_NAME_BYTES;
use super::outgoing::{Mark, Outgoing, SendError, unix_ms};
use super::{
    Failure, KEEP_UNFINISHED, MAX_FILES, MAX_RUNNING_TRANSFERS, MAX_UNANSWERED_CHUNKS, Transfer,
    TransferEvent, TransferId, TransferState,
};
use crate::Feature;
use crate::session::{Availability, Session, SessionError, SessionEvent};

/// The most bytes of frames that may wait to be written before no more
/// chunks are queued: well below the 1 MiB at which the session stops
/// reading, so it keeps reading the phone's chunks while it sends its own.
const MAX_QUEUED_BYTES: usize = 512 * 1024;

/// Room a transfer from the phone leaves free on the disk.
const SPARE_SPACE: u64 = 64 * 1024 * 1024;

/// Answers a transfer may miss in a row before it fails.
const MAX_MISSES: u32 = 3;

/// How long a busy phone is left alone when it does not say.
const BUSY_RETRY: Duration = Duration::from_secs(5);

/// Finished transfers kept in the list.
const MAX_FINISHED: usize = 100;

/// Cancels kept for a phone that is away.
const MAX_CANCELS: usize = 64;

/// Why the service could not go on with the session.
#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    /// The session failed.
    #[error(transparent)]
    Session(#[from] SessionError),
}

/// Reports the bytes free for the user in the file system of a directory.
type FreeSpace = Arc<dyn Fn(&Path) -> io::Result<u64> + Send + Sync>;

/// What a request this service sent waits for.
enum Pending {
    Offer {
        id: TransferId,
        generation: u32,
    },
    Chunk {
        id: TransferId,
        generation: u32,
        mark: Mark,
    },
    Cancel,
}

/// The files feature with one phone: the transfers it sends to this desktop
/// and those this desktop sends to it.
pub struct FileService {
    /// Where received files go.
    dir: PathBuf,
    free_space: FreeSpace,
    incoming: Vec<Incoming>,
    outgoing: Vec<Outgoing>,
    pending: HashMap<u64, Pending>,
    /// Transfers to cancel on the phone once it is reachable.
    cancels: Vec<TransferId>,
    events: broadcast::Sender<TransferEvent>,
}

impl FileService {
    /// A service that saves received files in `dir`, normally the user's
    /// Downloads. The directory is made when the first offer arrives.
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            free_space: Arc::new(disk::free_space),
            incoming: Vec::new(),
            outgoing: Vec::new(),
            pending: HashMap::new(),
            cancels: Vec::new(),
            events: broadcast::channel(256).0,
        }
    }

    /// Replaces how free space is measured, for tests.
    #[must_use]
    pub fn with_free_space(
        mut self,
        free_space: impl Fn(&Path) -> io::Result<u64> + Send + Sync + 'static,
    ) -> Self {
        self.free_space = Arc::new(free_space);
        self
    }

    /// Every transfer in the list, oldest first.
    pub fn transfers(&self) -> Vec<Transfer> {
        let mut transfers: Vec<Transfer> = self
            .incoming
            .iter()
            .map(Incoming::view)
            .chain(self.outgoing.iter().map(Outgoing::view))
            .collect();
        transfers.sort_by_key(|transfer| transfer.started_unix_ms);
        transfers
    }

    /// Each change to the transfers, as it happens.
    pub fn subscribe(&self) -> broadcast::Receiver<TransferEvent> {
        self.events.subscribe()
    }

    /// Whether a transfer is running, which the user sees happen.
    pub fn is_busy(&self) -> bool {
        self.incoming
            .iter()
            .any(|t| t.state == TransferState::Running)
            || self
                .outgoing
                .iter()
                .any(|t| t.state == TransferState::Running)
    }

    /// Starts sending the files at `paths` to the phone, as one transfer.
    /// It waits until [`FileService::run`] finds the feature ready, for up
    /// to [`KEEP_UNFINISHED`].
    ///
    /// # Errors
    ///
    /// Fails, sending nothing, for no files or too many, a folder, or a
    /// file that is not regular or cannot be read.
    pub async fn send(&mut self, paths: Vec<PathBuf>) -> Result<TransferId, SendError> {
        let files = paths.len();
        let transfer = Outgoing::new(paths).await?;
        let id = transfer.id;
        tracing::info!(transfer = %id, files, "sending files");
        self.publish(TransferEvent::Changed(transfer.view()));
        self.outgoing.push(transfer);
        Ok(id)
    }

    /// Cancels a transfer that is not finished, in either direction, and
    /// tells the phone at the next [`FileService::run`]. A received file
    /// that is not complete is deleted; complete ones stay. Returns whether
    /// there was such a transfer.
    pub async fn cancel(&mut self, id: TransferId) -> bool {
        if let Some(transfer) = self
            .outgoing
            .iter_mut()
            .find(|t| t.id == id && !t.state.is_finished())
        {
            transfer.finish(TransferState::Cancelled { by_peer: false });
            let (offered, view) = (transfer.offered, transfer.view());
            if offered {
                self.cancel_on_phone(id);
            }
            self.publish(TransferEvent::Changed(view));
        } else if let Some(transfer) = self
            .incoming
            .iter_mut()
            .find(|t| t.id == id && !t.state.is_finished())
        {
            transfer.discard().await;
            transfer.state = TransferState::Cancelled { by_peer: false };
            transfer.live = false;
            let view = transfer.view();
            self.cancel_on_phone(id);
            self.publish(TransferEvent::Changed(view));
        } else {
            return false;
        }
        tracing::info!(transfer = %id, "cancelled a transfer");
        self.prune();
        true
    }

    /// Removes a finished transfer from the list. Returns whether there
    /// was one.
    pub fn forget(&mut self, id: TransferId) -> bool {
        let before = self.incoming.len() + self.outgoing.len();
        self.incoming
            .retain(|t| t.id != id || !t.state.is_finished());
        self.outgoing
            .retain(|t| t.id != id || !t.state.is_finished());
        let removed = self.incoming.len() + self.outgoing.len() < before;
        if removed {
            self.publish(TransferEvent::Removed(id));
        }
        removed
    }

    /// Sends what can go now: offers for waiting transfers, as many as the
    /// phone runs at once, cancels, and chunks up to the session's limits.
    /// While the feature is not ready on both devices, running transfers
    /// pause instead, and go on with a new offer once it is.
    ///
    /// Call it once the session is up ([`FileService::session_started`]
    /// does), after [`FileService::send`] and [`FileService::cancel`], and
    /// at [`FileService::next_deadline`]. [`FileService::handle_event`]
    /// calls it after each event.
    ///
    /// # Errors
    ///
    /// Fails when the session does.
    pub async fn run<S>(&mut self, session: &mut Session<S>) -> Result<(), ServiceError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let now = Instant::now();
        self.expire(now).await;
        if !session.is_ready(Feature::Files) {
            self.pause_all(TransferState::Waiting, now);
            return Ok(());
        }
        for id in std::mem::take(&mut self.cancels) {
            let request = Payload::FileCancel(FileCancel {
                transfer_id: id.0.to_vec(),
            });
            let seq = session.queue_request(request)?;
            self.pending.insert(seq, Pending::Cancel);
        }
        self.offer_waiting(session, now)?;
        self.pump(session).await?;
        Ok(())
    }

    /// The session is up: sends what waits. Transfers from the phone go on
    /// once it offers them again.
    ///
    /// # Errors
    ///
    /// Fails when the session does.
    pub async fn session_started<S>(&mut self, session: &mut Session<S>) -> Result<(), ServiceError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        self.run(session).await
    }

    /// The session is gone. Running transfers are kept, for up to
    /// [`KEEP_UNFINISHED`], to go on in the next session.
    pub fn session_ended(&mut self) {
        self.pending.clear();
        self.pause_all(TransferState::Interrupted, Instant::now());
    }

    /// Ends the unfinished transfers kept for longer than
    /// [`KEEP_UNFINISHED`], deleting what was received of them.
    pub async fn expire(&mut self, now: Instant) {
        let mut changed = Vec::new();
        for transfer in &mut self.outgoing {
            let waiting = matches!(
                transfer.state,
                TransferState::Waiting | TransferState::Interrupted
            );
            if waiting && now >= transfer.since + KEEP_UNFINISHED {
                transfer.finish(TransferState::Failed(Failure::Expired));
                changed.push(transfer.view());
            }
        }
        for transfer in &mut self.incoming {
            if transfer.state == TransferState::Interrupted
                && now >= transfer.since + KEEP_UNFINISHED
            {
                transfer.discard().await;
                transfer.state = TransferState::Failed(Failure::Expired);
                changed.push(transfer.view());
            }
        }
        if !changed.is_empty() {
            tracing::info!(transfers = changed.len(), "unfinished transfers expired");
            for view in changed {
                self.publish(TransferEvent::Changed(view));
            }
            self.prune();
        }
    }

    /// When [`FileService::expire`] or [`FileService::run`] next has
    /// something to do without an event: a transfer expires, or a busy
    /// phone may be asked again.
    pub fn next_deadline(&self) -> Option<Instant> {
        let outgoing = self.outgoing.iter().filter_map(|t| match t.state {
            TransferState::Waiting | TransferState::Interrupted => {
                let expiry = t.since + KEEP_UNFINISHED;
                Some(t.retry_at.map_or(expiry, |retry| retry.min(expiry)))
            }
            _ => None,
        });
        let incoming = self
            .incoming
            .iter()
            .filter(|t| t.state == TransferState::Interrupted)
            .map(|t| t.since + KEEP_UNFINISHED);
        outgoing.chain(incoming).min()
    }

    /// Takes the events of the files feature, including
    /// [`SessionEvent::Drained`], and returns every other event, for the
    /// caller to handle. Then sends what can go, as
    /// [`FileService::run`] does, so a `FeatureStatus` that makes the
    /// feature ready starts the waiting transfers; it is returned too.
    ///
    /// # Errors
    ///
    /// Fails when the session does.
    pub async fn handle_event<S>(
        &mut self,
        session: &mut Session<S>,
        event: SessionEvent,
    ) -> Result<Option<SessionEvent>, ServiceError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let rest = match event {
            SessionEvent::Request {
                seq,
                payload: Payload::FileOffer(offer),
            } => {
                let answer = self.answer_offer(session, offer).await;
                session.queue_reply(seq, answer)?;
                None
            }
            SessionEvent::Request {
                seq,
                payload: Payload::FileChunk(chunk),
            } => {
                let answer = self.answer_chunk(session, chunk).await;
                session.queue_reply(seq, answer)?;
                None
            }
            SessionEvent::Request {
                seq,
                payload: Payload::FileCancel(cancel),
            } => {
                session.queue_reply(seq, Payload::Ack(Ack {}))?;
                if let Some(id) = TransferId::from_wire(&cancel.transfer_id) {
                    self.cancelled_by_phone(id).await;
                }
                None
            }
            SessionEvent::Reply(reply) => match self.pending.remove(&reply.request) {
                Some(pending) => {
                    self.on_answer(pending, reply.into_result());
                    None
                }
                None => Some(SessionEvent::Reply(reply)),
            },
            // Asked for by the pump, which goes on below.
            SessionEvent::Drained => None,
            SessionEvent::RequestTimedOut { request } => match self.pending.remove(&request) {
                Some(pending) => {
                    self.on_timeout(pending);
                    None
                }
                None => Some(SessionEvent::RequestTimedOut { request }),
            },
            other => Some(other),
        };
        self.run(session).await?;
        Ok(rest)
    }

    /// The answer to the phone's offer: where the bytes start, or why not.
    async fn answer_offer<S>(&mut self, session: &Session<S>, offer: FileOffer) -> Payload
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        if let Some(refusal) = refusal(session) {
            return refusal;
        }
        let Some(id) = TransferId::from_wire(&offer.transfer_id) else {
            return error(ErrorCode::InvalidArgument, "transfer_id is not 16 bytes");
        };
        if offer.files.is_empty() || offer.files.len() > MAX_FILES {
            return error(ErrorCode::InvalidArgument, "an offer has 1 to 500 files");
        }
        if offer
            .files
            .iter()
            .any(|file| file.name.is_empty() || file.name.len() > MAX_NAME_BYTES)
        {
            return error(ErrorCode::InvalidArgument, "a name has 1 to 255 bytes");
        }

        if let Some(transfer) = self.incoming.iter_mut().find(|t| t.id == id) {
            if !transfer.matches(&offer) {
                return error(
                    ErrorCode::InvalidArgument,
                    "the offer does not match the transfer it repeats",
                );
            }
            match transfer.state {
                TransferState::Completed => {}
                TransferState::Running | TransferState::Interrupted => {
                    transfer.state = TransferState::Running;
                    transfer.live = true;
                    tracing::info!(transfer = %id, "receiving files again");
                }
                _ => return error(ErrorCode::NotFound, "the transfer was cancelled"),
            }
            let accept = transfer.resume_point();
            let view = transfer.view();
            self.publish(TransferEvent::Changed(view));
            return Payload::FileAccept(accept);
        }

        let running = self
            .incoming
            .iter()
            .filter(|t| t.live && t.state == TransferState::Running);
        if running.clone().count() >= MAX_RUNNING_TRANSFERS {
            return Payload::ErrorReply(ErrorReply {
                code: ErrorCode::Busy.into(),
                detail: "4 transfers are running".into(),
                retry_after_ms: 0,
                ..ErrorReply::default()
            });
        }
        let needed = offer
            .files
            .iter()
            .map(|file| file.size)
            .chain(running.map(Incoming::remaining))
            .fold(SPARE_SPACE, u64::saturating_add);
        let (dir, free_space) = (self.dir.clone(), self.free_space.clone());
        let free = blocking(move || {
            fs::create_dir_all(&dir)?;
            Ok::<_, io::Error>(free_space(&dir))
        })
        .await;
        match free {
            Err(e) => {
                tracing::warn!(error = %e, "cannot make the directory for received files");
                return error(ErrorCode::Internal, "cannot save files");
            }
            Ok(Ok(free)) if free < needed => {
                tracing::info!(free, needed, "no room for the files the phone offers");
                return error(ErrorCode::NoSpace, "the files do not fit");
            }
            Ok(Ok(_)) => {}
            Ok(Err(error)) => tracing::debug!(%error, "free space unknown"),
        }

        let transfer = Incoming::new(id, &offer, self.dir.clone(), unix_ms(SystemTime::now()));
        tracing::info!(transfer = %id, files = offer.files.len(), "receiving files");
        self.publish(TransferEvent::Changed(transfer.view()));
        self.incoming.push(transfer);
        Payload::FileAccept(FileAccept {
            file_index: 0,
            offset: 0,
        })
    }

    /// The answer to one of the phone's chunks, once it is written.
    async fn answer_chunk<S>(&mut self, session: &Session<S>, chunk: FileChunk) -> Payload
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        if let Some(refusal) = refusal(session) {
            return refusal;
        }
        let found = TransferId::from_wire(&chunk.transfer_id).and_then(|id| {
            self.incoming
                .iter()
                .position(|t| t.id == id && t.live && t.state == TransferState::Running)
        });
        let Some(index) = found else {
            return error(ErrorCode::NotFound, "no such transfer");
        };
        let transfer = &mut self.incoming[index];
        if let Err(detail) = transfer.check(&chunk) {
            tracing::warn!(transfer = %transfer.id, detail, "the phone sent a chunk that does not fit");
            self.end_incoming(index, Failure::Invalid).await;
            return error(ErrorCode::InvalidArgument, detail);
        }
        match transfer.write(chunk).await {
            Ok(Written::Part) => {
                let (id, done) = (transfer.id, transfer.done());
                self.publish(TransferEvent::Progress { id, done });
            }
            Ok(Written::File) => {
                let view = transfer.view();
                self.publish(TransferEvent::Changed(view));
            }
            Ok(Written::All) => {
                transfer.state = TransferState::Completed;
                transfer.live = false;
                tracing::info!(transfer = %transfer.id, "received files");
                let view = transfer.view();
                self.publish(TransferEvent::Changed(view));
                self.prune();
            }
            Err(WriteError::Damaged) => {
                tracing::warn!(transfer = %transfer.id, "a received file does not match its hash");
                self.end_incoming(index, Failure::Damaged).await;
                return error(
                    ErrorCode::InvalidArgument,
                    "the file does not match its hash",
                );
            }
            Err(WriteError::Disk(e)) => {
                let full = e.raw_os_error() == Some(rustix::io::Errno::NOSPC.raw_os_error());
                tracing::warn!(error = %e, transfer = %transfer.id, "could not save a received file");
                return if full {
                    self.end_incoming(index, Failure::NoSpace).await;
                    error(ErrorCode::NoSpace, "the disk is full")
                } else {
                    self.end_incoming(index, Failure::WriteFailed).await;
                    error(ErrorCode::Internal, "could not save the file")
                };
            }
        }
        Payload::Ack(Ack {})
    }

    /// Ends the transfer from the phone at `index` as failed, deleting what
    /// is not finished.
    async fn end_incoming(&mut self, index: usize, failure: Failure) {
        let transfer = &mut self.incoming[index];
        transfer.discard().await;
        transfer.state = TransferState::Failed(failure);
        transfer.live = false;
        let view = transfer.view();
        self.publish(TransferEvent::Changed(view));
        self.prune();
    }

    /// The phone cancelled transfer `id`.
    async fn cancelled_by_phone(&mut self, id: TransferId) {
        let cancelled = TransferState::Cancelled { by_peer: true };
        let view = if let Some(transfer) = self
            .outgoing
            .iter_mut()
            .find(|t| t.id == id && !t.state.is_finished())
        {
            transfer.finish(cancelled);
            transfer.view()
        } else if let Some(transfer) = self
            .incoming
            .iter_mut()
            .find(|t| t.id == id && !t.state.is_finished())
        {
            transfer.discard().await;
            transfer.state = cancelled;
            transfer.live = false;
            transfer.view()
        } else {
            return;
        };
        tracing::info!(transfer = %id, "the phone cancelled a transfer");
        self.publish(TransferEvent::Changed(view));
        self.prune();
    }

    /// Handles the phone's answer to one of this side's requests.
    fn on_answer(&mut self, pending: Pending, answer: Result<Payload, ErrorReply>) {
        let (id, generation) = match &pending {
            Pending::Cancel => return,
            Pending::Offer { id, generation } | Pending::Chunk { id, generation, .. } => {
                (*id, *generation)
            }
        };
        // Answers to what went out before a pause or a cancel do not count.
        let Some(transfer) = self.outgoing.iter_mut().find(|t| {
            t.id == id && t.generation == generation && t.state == TransferState::Running
        }) else {
            return;
        };
        let event = match (pending, answer) {
            (Pending::Offer { .. }, Ok(Payload::FileAccept(accept))) => {
                match transfer.accept(&accept) {
                    Ok(()) if transfer.is_complete() => {
                        transfer.finish(TransferState::Completed);
                        tracing::info!(transfer = %id, "the phone already had the files");
                    }
                    Ok(()) => {}
                    Err(detail) => {
                        tracing::warn!(transfer = %id, detail, "the phone accepted an offer at a position that does not exist");
                        transfer.finish(TransferState::Failed(Failure::Refused));
                        self.cancels.push(id);
                    }
                }
                TransferEvent::Changed(transfer.view())
            }
            (Pending::Offer { .. }, Err(refused))
                if matches!(refused.code(), ErrorCode::Busy | ErrorCode::Unavailable) =>
            {
                let now = Instant::now();
                let wait = match refused.retry_after_ms {
                    0 => BUSY_RETRY,
                    ms => Duration::from_millis(ms.into()),
                };
                let since = transfer.since;
                transfer.pause(TransferState::Waiting, now);
                // Being turned away does not restart the hour it is kept.
                transfer.since = since;
                transfer.retry_at = Some(now + wait);
                TransferEvent::Changed(transfer.view())
            }
            (Pending::Chunk { mark, .. }, Ok(_)) => {
                transfer.acked(mark);
                if transfer.is_complete() {
                    transfer.finish(TransferState::Completed);
                    tracing::info!(transfer = %id, "sent files");
                    TransferEvent::Changed(transfer.view())
                } else {
                    TransferEvent::Progress {
                        id,
                        done: transfer.done(),
                    }
                }
            }
            (_, Err(refused)) => {
                tracing::info!(
                    transfer = %id,
                    code = refused.code,
                    detail = %refused.detail,
                    "the phone refused a transfer"
                );
                transfer.finish(TransferState::Failed(failure_of(&refused)));
                TransferEvent::Changed(transfer.view())
            }
            // The session lets only `FileAccept` answer an offer, and only
            // `Ack` a chunk.
            (_, Ok(_)) => return,
        };
        let finished = transfer.state.is_finished();
        self.publish(event);
        if finished {
            self.prune();
        }
    }

    /// One of this side's requests got no answer in time. The transfer is
    /// offered again, unless it missed too many answers.
    fn on_timeout(&mut self, pending: Pending) {
        let (Pending::Offer { id, generation } | Pending::Chunk { id, generation, .. }) = pending
        else {
            return;
        };
        let Some(transfer) = self.outgoing.iter_mut().find(|t| {
            t.id == id && t.generation == generation && t.state == TransferState::Running
        }) else {
            return;
        };
        transfer.misses += 1;
        if transfer.misses >= MAX_MISSES {
            tracing::info!(transfer = %id, "the phone stopped answering");
            transfer.finish(TransferState::Failed(Failure::Unanswered));
            self.cancels.push(id);
        } else {
            transfer.pause(TransferState::Waiting, Instant::now());
        }
        let view = transfer.view();
        self.publish(TransferEvent::Changed(view));
        self.prune();
    }

    /// Offers the waiting transfers, as many as the phone runs at once.
    fn offer_waiting<S>(
        &mut self,
        session: &mut Session<S>,
        now: Instant,
    ) -> Result<(), SessionError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let mut running = self
            .outgoing
            .iter()
            .filter(|t| t.state == TransferState::Running)
            .count();
        for transfer in &mut self.outgoing {
            if running >= MAX_RUNNING_TRANSFERS {
                break;
            }
            let waiting = matches!(
                transfer.state,
                TransferState::Waiting | TransferState::Interrupted
            );
            if !waiting || transfer.retry_at.is_some_and(|at| at > now) {
                continue;
            }
            let seq = session.queue_request(Payload::FileOffer(transfer.offer()))?;
            self.pending.insert(
                seq,
                Pending::Offer {
                    id: transfer.id,
                    generation: transfer.generation,
                },
            );
            transfer.state = TransferState::Running;
            transfer.offered = true;
            transfer.retry_at = None;
            running += 1;
            let _ = self.events.send(TransferEvent::Changed(transfer.view()));
        }
        Ok(())
    }

    /// Queues chunks while the session takes them: at most
    /// [`MAX_UNANSWERED_CHUNKS`] unanswered, and not too many bytes waiting
    /// to be written. The transfer with the fewest chunks out goes next, so
    /// transfers share the session.
    async fn pump<S>(&mut self, session: &mut Session<S>) -> Result<(), SessionError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        loop {
            let unanswered = self
                .pending
                .values()
                .filter(|p| matches!(p, Pending::Chunk { .. }))
                .count();
            if unanswered >= MAX_UNANSWERED_CHUNKS {
                return Ok(());
            }
            if session.queued_len() >= MAX_QUEUED_BYTES {
                // Go on once the phone has read some: answers may not come
                // before it has more.
                session.wake_when_drained(MAX_QUEUED_BYTES / 2);
                return Ok(());
            }
            let next = self
                .outgoing
                .iter()
                .enumerate()
                .filter(|(_, t)| t.has_more())
                .min_by_key(|(_, t)| t.in_flight)
                .map(|(index, _)| index);
            let Some(index) = next else {
                return Ok(());
            };
            let transfer = &mut self.outgoing[index];
            match transfer.next_chunk().await {
                Ok((chunk, mark)) => {
                    let seq = session.queue_request(Payload::FileChunk(chunk))?;
                    self.pending.insert(
                        seq,
                        Pending::Chunk {
                            id: transfer.id,
                            generation: transfer.generation,
                            mark,
                        },
                    );
                    transfer.in_flight += 1;
                }
                Err(error) => {
                    let failure = match error {
                        SourceError::Changed => Failure::Changed,
                        SourceError::Unreadable(_) => Failure::Unreadable,
                    };
                    tracing::warn!(%error, transfer = %transfer.id, "stopped sending files");
                    transfer.finish(TransferState::Failed(failure));
                    let (id, view) = (transfer.id, transfer.view());
                    let request = Payload::FileCancel(FileCancel {
                        transfer_id: id.0.to_vec(),
                    });
                    let seq = session.queue_request(request)?;
                    self.pending.insert(seq, Pending::Cancel);
                    self.publish(TransferEvent::Changed(view));
                    self.prune();
                }
            }
        }
    }

    /// Pauses every running transfer: those to the phone wait, as `state`,
    /// for a new offer, and those from it for the phone's.
    fn pause_all(&mut self, state: TransferState, now: Instant) {
        for transfer in &mut self.outgoing {
            if transfer.state == TransferState::Running {
                transfer.pause(state.clone(), now);
                let _ = self.events.send(TransferEvent::Changed(transfer.view()));
            }
        }
        for transfer in &mut self.incoming {
            transfer.live = false;
            if transfer.state == TransferState::Running {
                transfer.state = TransferState::Interrupted;
                transfer.since = now;
                let _ = self.events.send(TransferEvent::Changed(transfer.view()));
            }
        }
    }

    fn cancel_on_phone(&mut self, id: TransferId) {
        if !self.cancels.contains(&id) {
            if self.cancels.len() >= MAX_CANCELS {
                self.cancels.remove(0);
            }
            self.cancels.push(id);
        }
    }

    /// Drops the oldest finished transfers beyond [`MAX_FINISHED`].
    fn prune(&mut self) {
        let mut finished: Vec<(i64, TransferId)> = self
            .incoming
            .iter()
            .filter(|t| t.state.is_finished())
            .map(|t| (t.started_unix_ms(), t.id))
            .chain(
                self.outgoing
                    .iter()
                    .filter(|t| t.state.is_finished())
                    .map(|t| (t.started_unix_ms(), t.id)),
            )
            .collect();
        if finished.len() <= MAX_FINISHED {
            return;
        }
        finished.sort_unstable();
        let excess = finished.len() - MAX_FINISHED;
        for (_, id) in finished.into_iter().take(excess) {
            self.forget(id);
        }
    }

    fn publish(&self, event: TransferEvent) {
        // No subscriber is fine: the page may not be open.
        let _ = self.events.send(event);
    }
}

/// The refusal of a request while the feature is not ready on this
/// desktop.
fn refusal<S>(session: &Session<S>) -> Option<Payload>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    match session.availability().get(&Feature::Files) {
        Some(Availability::Ready) => None,
        Some(Availability::PermissionRequired(permission)) => {
            Some(Payload::ErrorReply(ErrorReply {
                code: ErrorCode::PermissionRequired.into(),
                detail: "files need a permission on this desktop".into(),
                permission: permission.clone(),
                ..ErrorReply::default()
            }))
        }
        _ => Some(error(ErrorCode::Disabled, "files are turned off")),
    }
}

/// Why the phone's refusal failed a transfer.
fn failure_of(refused: &ErrorReply) -> Failure {
    match refused.code() {
        ErrorCode::Disabled => Failure::Disabled,
        ErrorCode::PermissionRequired => Failure::PermissionRequired,
        ErrorCode::NoSpace => Failure::NoSpace,
        _ => Failure::Refused,
    }
}

fn error(code: ErrorCode, detail: &str) -> Payload {
    Payload::ErrorReply(ErrorReply {
        code: code.into(),
        detail: detail.into(),
        ..ErrorReply::default()
    })
}
