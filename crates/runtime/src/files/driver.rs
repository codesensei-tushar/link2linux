//! One phone's file transfers in the runtime: the core's service, the board
//! the files page reads, and the banners that say a transfer ended.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use link2linux_core::Feature;
use link2linux_core::files::{
    Direction, Failure, FileService, SendError, ServiceError, Transfer, TransferEvent, TransferId,
    TransferState,
};
use link2linux_core::session::SessionEvent;
use link2linux_platform::{
    DesktopAction, DesktopNotification, NotificationId, NotificationResponse,
};
use tokio::sync::broadcast::{self, error::TryRecvError};
use tokio::time::Instant;

use super::{FilesSwitch, Opener, dto};
use crate::link::lock;
use crate::notify::{PhoneSession, Presenter};
use crate::view::EventSink;

/// The transfers with one phone, as the files page and the banners read
/// them, with the local paths the page never sees.
pub type Board = Arc<Mutex<Vec<Transfer>>>;

/// The key of a banner's "Show in folder" button.
const SHOW_FILE_KEY: &str = "files-show";

/// How often a running transfer's progress goes to the page.
const PROGRESS_EVERY: Duration = Duration::from_millis(250);

/// One phone's file transfers, driven by its session loop like the
/// notifications driver: [`Self::session_started`], every event through
/// [`Self::handle_event`], [`Self::session_ended`], and
/// [`Self::tick`] at [`Self::next_deadline`]. The user's sends and cancels
/// work with or without a session.
pub struct FilesDriver {
    service: FileService,
    changes: broadcast::Receiver<TransferEvent>,
    device_id: String,
    phone_name: String,
    board: Board,
    events: Arc<dyn EventSink>,
    presenter: Arc<dyn Presenter>,
    opener: Arc<dyn Opener>,
    switch: Arc<FilesSwitch>,
    /// The banners on screen, with the transfer each tells about.
    banners: HashMap<NotificationId, TransferId>,
    /// Transfers that ended and still need their banner.
    due: Vec<Transfer>,
    /// When each running transfer's progress last went to the page.
    progress_sent: HashMap<TransferId, Instant>,
}

/// What a [`FilesDriver`] needs from the runtime around it.
pub struct DriverParts {
    pub service: FileService,
    pub device_id: String,
    pub phone_name: String,
    pub board: Board,
    pub events: Arc<dyn EventSink>,
    pub presenter: Arc<dyn Presenter>,
    pub opener: Arc<dyn Opener>,
    pub switch: Arc<FilesSwitch>,
}

impl FilesDriver {
    /// A driver for one phone.
    #[must_use]
    pub fn new(parts: DriverParts) -> Self {
        let changes = parts.service.subscribe();
        Self {
            service: parts.service,
            changes,
            device_id: parts.device_id,
            phone_name: parts.phone_name,
            board: parts.board,
            events: parts.events,
            presenter: parts.presenter,
            opener: parts.opener,
            switch: parts.switch,
            banners: HashMap::new(),
            due: Vec::new(),
            progress_sent: HashMap::new(),
        }
    }

    /// Whether a transfer is running, which the user sees happen.
    #[must_use]
    pub fn is_busy(&self) -> bool {
        self.service.is_busy()
    }

    /// The session is up: sends what waits.
    ///
    /// # Errors
    ///
    /// Fails when the session does.
    pub async fn session_started(
        &mut self,
        session: &mut PhoneSession,
    ) -> Result<(), ServiceError> {
        let started = self.service.session_started(session).await;
        self.publish().await;
        started
    }

    /// Takes the events of the files feature and returns the rest.
    ///
    /// # Errors
    ///
    /// Fails when the session does.
    pub async fn handle_event(
        &mut self,
        session: &mut PhoneSession,
        event: SessionEvent,
    ) -> Result<Option<SessionEvent>, ServiceError> {
        let rest = self.service.handle_event(session, event).await;
        self.publish().await;
        rest
    }

    /// The user turned files on or off: tells the phone, and pauses or goes
    /// on with the transfers.
    ///
    /// # Errors
    ///
    /// Fails when the session does.
    pub async fn switched(&mut self, session: &mut PhoneSession) -> Result<(), ServiceError> {
        session
            .set_availability(Feature::Files, self.switch.availability())
            .await?;
        let ran = self.service.run(session).await;
        self.publish().await;
        ran
    }

    /// The session is over: running transfers pause.
    pub fn session_ended(&mut self) {
        self.service.session_ended();
        self.drain();
    }

    /// When [`Self::tick`] has something to do.
    #[must_use]
    pub fn next_deadline(&self) -> Option<Instant> {
        self.service.next_deadline()
    }

    /// Ends transfers kept too long, and sends what waited for a busy
    /// phone when `session` is up.
    ///
    /// # Errors
    ///
    /// Fails when the session does.
    pub async fn tick(&mut self, session: Option<&mut PhoneSession>) -> Result<(), ServiceError> {
        self.service.expire(Instant::now()).await;
        let ran = match session {
            Some(session) => self.service.run(session).await,
            None => Ok(()),
        };
        self.publish().await;
        ran
    }

    /// Sends the files at `paths` as one transfer: at once when `session`
    /// is up, otherwise when the phone is back, for up to an hour.
    ///
    /// # Errors
    ///
    /// Fails, sending nothing, for files that cannot be sent; the shell
    /// checked them before, so only when they changed since.
    pub async fn send(
        &mut self,
        session: Option<&mut PhoneSession>,
        paths: Vec<PathBuf>,
    ) -> Result<(), SendError> {
        let sent = self.service.send(paths).await.map(drop);
        self.after_command(session).await;
        sent
    }

    /// Cancels transfer `id`, telling the phone when `session` is up, and
    /// otherwise once it is back.
    pub async fn cancel(&mut self, session: Option<&mut PhoneSession>, id: TransferId) {
        self.service.cancel(id).await;
        self.after_command(session).await;
    }

    /// The phone is unpaired, or the app is closing: deletes what arrived
    /// of the transfers from it that are not finished.
    pub async fn discard_all(&mut self) {
        self.service.discard_all().await;
    }

    /// Removes finished transfer `id` from the list.
    pub fn forget(&mut self, id: TransferId) {
        self.service.forget(id);
        self.drain();
    }

    /// Acts on a click on one of this driver's banners: "Show in
    /// folder". Returns whether the response was for one of them.
    pub fn handle_response(&mut self, response: &NotificationResponse) -> bool {
        match response {
            NotificationResponse::Action { id, key } => {
                let Some(transfer) = self.banners.get(id) else {
                    return false;
                };
                let path = lock(&self.board)
                    .iter()
                    .find(|t| t.id == *transfer)
                    .and_then(|t| t.files.iter().find_map(|file| file.path.clone()));
                if let Some(path) = path {
                    let opener = self.opener.clone();
                    if key == SHOW_FILE_KEY {
                        tokio::spawn(async move {
                            if let Err(error) = opener.show_in_folder(&path).await {
                                tracing::warn!(%error, "could not show a received file");
                            }
                        });
                    }
                }
                true
            }
            NotificationResponse::Closed { id, .. } => self.banners.remove(id).is_some(),
            _ => false,
        }
    }

    async fn after_command(&mut self, session: Option<&mut PhoneSession>) {
        if let Some(session) = session
            && let Err(error) = self.service.run(session).await
        {
            tracing::debug!(%error, "files could not go on in this session");
        }
        self.publish().await;
    }

    /// Takes the service's changes to the board and the page, then shows
    /// the banners of transfers that ended.
    async fn publish(&mut self) {
        self.drain();
        for transfer in std::mem::take(&mut self.due) {
            let Some((banner, when_locked)) = banner(&transfer, &self.phone_name) else {
                continue;
            };
            match self.presenter.show_own(&banner, &when_locked, None).await {
                Ok(id) => {
                    self.banners.insert(id, transfer.id);
                }
                Err(error) => tracing::warn!(%error, "could not say a transfer ended"),
            }
        }
    }

    /// Applies the service's changes to the board and passes them to the
    /// page, keeping the transfers that ended for their banners.
    fn drain(&mut self) {
        loop {
            match self.changes.try_recv() {
                Ok(event) => self.apply(event),
                Err(TryRecvError::Lagged(_)) => {
                    // Missed some: the page gets the whole list again.
                    let transfers = self.service.transfers();
                    for transfer in &transfers {
                        self.events.transfer_changed(
                            &self.device_id,
                            &dto::FileTransferEvent::Changed {
                                transfer: dto::transfer(transfer),
                            },
                        );
                    }
                    *lock(&self.board) = transfers;
                }
                Err(TryRecvError::Empty | TryRecvError::Closed) => return,
            }
        }
    }

    fn apply(&mut self, event: TransferEvent) {
        let mut board = lock(&self.board);
        match &event {
            TransferEvent::Changed(transfer) => {
                let was_finished = board
                    .iter()
                    .find(|t| t.id == transfer.id)
                    .is_some_and(|t| t.state.is_finished());
                if transfer.state.is_finished() && !was_finished {
                    self.due.push(transfer.clone());
                    self.progress_sent.remove(&transfer.id);
                }
                match board.iter_mut().find(|t| t.id == transfer.id) {
                    Some(old) => *old = transfer.clone(),
                    None => board.push(transfer.clone()),
                }
            }
            TransferEvent::Progress { id, done } => {
                if let Some(transfer) = board.iter_mut().find(|t| t.id == *id) {
                    transfer.done = *done;
                }
                let now = Instant::now();
                let recent = self
                    .progress_sent
                    .get(id)
                    .is_some_and(|sent| now < *sent + PROGRESS_EVERY);
                if recent {
                    return;
                }
                self.progress_sent.insert(*id, now);
            }
            TransferEvent::Removed(id) => {
                board.retain(|t| t.id != *id);
                self.progress_sent.remove(id);
            }
        }
        drop(board);
        self.events
            .transfer_changed(&self.device_id, &dto::event(&event));
    }
}

/// The banner that says `transfer` with `phone` ended, if it gets one:
/// files that arrived, and transfers that failed or that the phone
/// cancelled. It names no file. The second form is for a locked screen,
/// without the phone's name either, as that comes from the phone.
fn banner(transfer: &Transfer, phone: &str) -> Option<(DesktopNotification, DesktopNotification)> {
    let incoming = transfer.direction == Direction::Incoming;
    let count = transfer.files.len();
    let files = if count == 1 {
        "a file".to_owned()
    } else {
        format!("{count} files")
    };
    let (summary, locked, body, actions, category) = match &transfer.state {
        TransferState::Completed if incoming => {
            // Only "Show in folder": the banner names no file, so a click
            // must never start one. Open is on the files page, where the
            // name is visible.
            let actions = vec![action(SHOW_FILE_KEY, "Show in folder")];
            (
                format!("Received {files} from {phone}"),
                format!("Received {files}"),
                "Saved in Downloads".to_owned(),
                actions,
                "transfer.complete",
            )
        }
        TransferState::Failed(failure) => (
            if incoming {
                format!("Couldn't receive files from {phone}")
            } else {
                format!("Couldn't send files to {phone}")
            },
            if incoming {
                "Couldn't receive files".to_owned()
            } else {
                "Couldn't send files".to_owned()
            },
            failure_text(*failure, incoming, phone),
            Vec::new(),
            "transfer.error",
        ),
        TransferState::Cancelled { by_peer: true } => (
            if incoming {
                format!("{phone} stopped sending files")
            } else {
                format!("{phone} cancelled the files you sent")
            },
            "A file transfer was cancelled".to_owned(),
            String::new(),
            Vec::new(),
            "transfer.error",
        ),
        _ => return None,
    };
    let shown = DesktopNotification {
        app_name: "link2linux".to_owned(),
        summary,
        body,
        actions,
        category: Some(category.to_owned()),
        ..DesktopNotification::default()
    };
    let when_locked = DesktopNotification {
        summary: locked,
        body: String::new(),
        ..shown.clone()
    };
    Some((shown, when_locked))
}

fn action(key: &str, label: &str) -> DesktopAction {
    DesktopAction {
        key: key.to_owned(),
        label: label.to_owned(),
    }
}

/// Why a transfer failed, in plain words, as the files page says it.
fn failure_text(failure: Failure, incoming: bool, phone: &str) -> String {
    match failure {
        // Also what a send gets that waited an hour for a phone with files
        // off.
        Failure::Disabled => format!("Files are turned off on {phone}."),
        Failure::PermissionRequired => format!("{phone} needs permission to save files."),
        Failure::NoSpace if incoming => "There isn't enough space on this computer.".to_owned(),
        Failure::NoSpace => format!("There isn't enough space on {phone}."),
        Failure::Unreadable => "A file couldn't be read.".to_owned(),
        Failure::Changed => "A file changed while it was being sent.".to_owned(),
        Failure::Damaged => "A file arrived damaged, so it was deleted.".to_owned(),
        Failure::Invalid => format!("{phone} sent the files incorrectly."),
        Failure::WriteFailed => "A file couldn't be saved.".to_owned(),
        Failure::Expired => format!("{phone} didn't reconnect within an hour."),
        _ => format!("{phone} didn't accept the files."),
    }
}

#[cfg(test)]
mod tests {
    use link2linux_core::files::TransferFile;

    use super::*;

    fn transfer(direction: Direction, state: TransferState, files: usize) -> Transfer {
        Transfer {
            id: TransferId([1; 16]),
            direction,
            state,
            files: (0..files)
                .map(|n| TransferFile {
                    name: format!("secret plan {n}.pdf"),
                    size: 10,
                    mime_type: String::new(),
                    path: Some(PathBuf::from(format!(
                        "/home/ana/Downloads/secret plan {n}.pdf"
                    ))),
                })
                .collect(),
            size: 10,
            done: 10,
            started_unix_ms: 0,
        }
    }

    fn keys(banner: &DesktopNotification) -> Vec<&str> {
        banner.actions.iter().map(|a| a.key.as_str()).collect()
    }

    #[test]
    fn received_files_get_a_banner_with_show_only_and_no_names() {
        let received = transfer(Direction::Incoming, TransferState::Completed, 1);
        let (one, locked) = banner(&received, "Pixel").unwrap();
        assert_eq!(one.summary, "Received a file from Pixel");
        assert_eq!(keys(&one), [SHOW_FILE_KEY]);
        assert_eq!(one.category.as_deref(), Some("transfer.complete"));
        assert_eq!(locked.summary, "Received a file");
        assert_eq!(keys(&locked), keys(&one));

        let received = transfer(Direction::Incoming, TransferState::Completed, 2);
        let (two, _) = banner(&received, "Pixel").unwrap();
        assert_eq!(two.summary, "Received 2 files from Pixel");
        assert_eq!(keys(&two), [SHOW_FILE_KEY]);
        for shown in [one, two, locked] {
            assert!(!format!("{shown:?}").contains("secret plan"), "{shown:?}");
        }
    }

    #[test]
    fn failures_and_the_phones_cancels_get_one_but_not_the_users_own_actions() {
        let full = TransferState::Failed(Failure::NoSpace);
        let (failed, locked) = banner(&transfer(Direction::Outgoing, full, 1), "Pixel").unwrap();
        assert_eq!(failed.summary, "Couldn't send files to Pixel");
        assert_eq!(failed.body, "There isn't enough space on Pixel.");
        assert!(failed.actions.is_empty());
        assert!(!format!("{locked:?}").contains("Pixel"), "{locked:?}");

        let cancelled = TransferState::Cancelled { by_peer: true };
        let (stopped, _) = banner(&transfer(Direction::Incoming, cancelled, 1), "Pixel").unwrap();
        assert_eq!(stopped.summary, "Pixel stopped sending files");

        for quiet in [
            transfer(Direction::Outgoing, TransferState::Completed, 1),
            transfer(
                Direction::Incoming,
                TransferState::Cancelled { by_peer: false },
                1,
            ),
            transfer(Direction::Incoming, TransferState::Running, 1),
        ] {
            assert_eq!(banner(&quiet, "Pixel"), None, "{quiet:?}");
        }
    }
}
