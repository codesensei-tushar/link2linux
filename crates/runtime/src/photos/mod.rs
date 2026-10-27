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

/// One saved photo: where it is, and what the page says about it.
#[derive(Debug, Clone)]
struct SavedRecord {
    path: PathBuf,
    saved: dto::SavedPhoto,
}

/// Where the user saved which version of which photo, for as long as the
/// app runs. A version is the photo's id together with its
/// `modified_unix_ms`: a newer version is a different photo to save.
#[derive(Debug, Default)]
pub struct SavedPhotos {
    saved: Mutex<HashMap<(String, String, i64), SavedRecord>>,
}

impl SavedPhotos {
    /// Notes that version `modified_unix_ms` of photo `photo_id` on phone
    /// `device_id` was saved at `path`, under `name` in `folder`.
    pub fn insert(
        &self,
        device_id: &str,
        photo_id: &str,
        modified_unix_ms: i64,
        path: PathBuf,
        name: &str,
        folder: &str,
    ) {
        lock(&self.saved).insert(
            (device_id.to_owned(), photo_id.to_owned(), modified_unix_ms),
            SavedRecord {
                path,
                saved: dto::SavedPhoto {
                    name: name.to_owned(),
                    folder: folder.to_owned(),
                },
            },
        );
    }

    /// The save of that version, when the file is still there. A file that
    /// has gone is forgotten, so saving the photo writes it again.
    #[must_use]
    pub fn reusable(
        &self,
        device_id: &str,
        photo_id: &str,
        modified_unix_ms: i64,
    ) -> Option<dto::SavedPhoto> {
        let key = (device_id.to_owned(), photo_id.to_owned(), modified_unix_ms);
        let mut saved = lock(&self.saved);
        let record = saved.get(&key)?;
        if record.path.is_file() {
            return Some(record.saved.clone());
        }
        saved.remove(&key);
        None
    }

    /// Where that version was saved, when this run saved it.
    #[must_use]
    pub fn path(&self, device_id: &str, photo_id: &str, modified_unix_ms: i64) -> Option<PathBuf> {
        lock(&self.saved)
            .get(&(device_id.to_owned(), photo_id.to_owned(), modified_unix_ms))
            .map(|record| record.path.clone())
    }

    /// Marks the photos in `photos` that this run saved.
    pub fn apply(&self, device_id: &str, photos: &mut dto::PhonePhotos) {
        let saved = lock(&self.saved);
        for photo in &mut photos.photos {
            photo.saved = saved
                .get(&(
                    device_id.to_owned(),
                    photo.id.clone(),
                    photo.modified_unix_ms,
                ))
                .map(|record| record.saved.clone());
        }
    }

    /// Forgets phone `device_id`, which is no longer paired.
    pub fn forget(&self, device_id: &str) {
        lock(&self.saved).retain(|(device, _, _), _| device != device_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_save_is_remembered_for_that_version_of_the_photo() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("IMG_7 (1).jpg");
        std::fs::write(&path, b"bytes").unwrap();
        let saved = SavedPhotos::default();
        saved.insert("phone", "7", 9, path.clone(), "IMG_7 (1).jpg", "Pictures");

        let found = saved.reusable("phone", "7", 9).unwrap();
        assert_eq!(found.name, "IMG_7 (1).jpg");
        assert_eq!(found.folder, "Pictures");
        assert!(saved.reusable("phone", "7", 10).is_none());
        assert_eq!(saved.path("phone", "7", 9).unwrap(), path);

        std::fs::remove_file(&path).unwrap();
        assert!(saved.reusable("phone", "7", 9).is_none());
        saved.insert("phone", "7", 9, path, "IMG_7 (1).jpg", "Downloads");
        saved.forget("phone");
        assert!(saved.path("phone", "7", 9).is_none());
    }
}
