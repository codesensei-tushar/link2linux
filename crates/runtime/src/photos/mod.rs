//! Each phone's recent photos.
//!
//! The core's [`PhotoService`] applies the protocol's rules. Each phone's
//! [`PhotosDriver`] runs one in its session loop, puts what the photos page
//! shows on the phone's [`PhoneBoard`], and fetches thumbnails and photos
//! for the [`PhotoCommand`]s the shell sends. [`SavedPhotos`] remembers
//! where the user saved which photo, so the shell opens it by its id and
//! never names a path. [`dto`] has what the UI receives.

pub mod dto;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use link2linux_core::photos::{PhotoError, PhotoService, PhotoWaiter, ThumbnailWaiter};
use link2linux_core::session::{SessionError, SessionEvent};
use tokio::time::Instant;

use crate::link::lock;
use crate::notify::PhoneSession;
use crate::phone::PhoneBoard;

/// What the shell asks of a phone's photos. Each carries where its answer
/// goes.
#[derive(Debug)]
pub enum PhotoCommand {
    /// A thumbnail of the photo, at most `max_size` pixels on its longest
    /// side.
    Thumbnail {
        id: String,
        max_size: u32,
        reply: ThumbnailWaiter,
    },
    /// The photo's original file.
    Fetch { id: String, reply: PhotoWaiter },
}

impl PhotoCommand {
    /// Answers the command at once with `error`.
    fn refuse(self, error: PhotoError) {
        match self {
            Self::Thumbnail { reply, .. } => drop(reply.send(Err(error))),
            Self::Fetch { reply, .. } => drop(reply.send(Err(error))),
        }
    }
}

/// One phone's photos, as its session loop runs them.
pub struct PhotosDriver {
    service: PhotoService,
    board: PhoneBoard,
}

impl PhotosDriver {
    /// The driver for the phone of `board`, which shows on it.
    #[must_use]
    pub fn new(board: PhoneBoard) -> Self {
        Self {
            service: PhotoService::new(),
            board,
        }
    }

    /// See [`PhotoService::session_started`].
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

    /// See [`PhotoService::handle_event`].
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

    /// Acts on what the shell asked. `session` is `None` between sessions,
    /// when nothing can be fetched.
    ///
    /// # Errors
    ///
    /// Fails when the session fails.
    pub fn command(
        &mut self,
        session: Option<&mut PhoneSession>,
        command: PhotoCommand,
    ) -> Result<(), SessionError> {
        let Some(session) = session else {
            command.refuse(PhotoError::NotConnected);
            return Ok(());
        };
        match command {
            PhotoCommand::Thumbnail {
                id,
                max_size,
                reply,
            } => self.service.thumbnail(session, &id, max_size, reply),
            PhotoCommand::Fetch { id, reply } => self.service.fetch(session, &id, reply),
        }
    }

    /// See [`PhotoService::next_deadline`].
    #[must_use]
    pub fn next_deadline(&self) -> Option<Instant> {
        self.service.next_deadline()
    }

    /// See [`PhotoService::tick`].
    ///
    /// # Errors
    ///
    /// Fails when the session fails.
    pub async fn tick(&mut self, session: &mut PhoneSession) -> Result<(), SessionError> {
        let ticked = self.service.tick(session, Instant::now()).await;
        self.publish();
        ticked
    }

    /// See [`PhotoService::session_ended`].
    pub fn session_ended(&mut self) {
        self.service.session_ended();
        self.publish();
    }

    fn publish(&self) {
        self.board
            .set_photos(dto::photos(self.service.state(), self.service.photos()));
    }
}

/// Where the user saved which photo, by phone and photo id, for as long as
/// the app runs.
#[derive(Debug, Default)]
pub struct SavedPhotos {
    saved: Mutex<HashMap<(String, String), PathBuf>>,
}

impl SavedPhotos {
    /// Notes that photo `photo_id` of phone `device_id` was saved at `path`.
    pub fn insert(&self, device_id: &str, photo_id: &str, path: PathBuf) {
        lock(&self.saved).insert((device_id.to_owned(), photo_id.to_owned()), path);
    }

    /// Where photo `photo_id` of phone `device_id` was saved last.
    #[must_use]
    pub fn get(&self, device_id: &str, photo_id: &str) -> Option<PathBuf> {
        lock(&self.saved)
            .get(&(device_id.to_owned(), photo_id.to_owned()))
            .cloned()
    }

    /// Forgets phone `device_id`, which is no longer paired.
    pub fn forget(&self, device_id: &str) {
        lock(&self.saved).retain(|(device, _), _| device != device_id);
    }
}
