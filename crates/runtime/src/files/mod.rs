//! File transfers with each phone.
//!
//! The core's [`FileService`] does the transfers. Each phone's
//! [`FilesDriver`] runs one in its session loop, keeps the board the files
//! page reads, and says when a transfer ended with a banner; the
//! [`FilesFactory`] makes them, and cleans up after an earlier run when
//! the app starts. [`dto`] has what the files page receives.

pub mod driver;
pub mod dto;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub use link2linux_core::files::shown_name;
use link2linux_core::files::{
    FileService, IncomingBytes, SendError, Transfer, TransferId, check_sendable,
};
use link2linux_core::session::Availability;
use link2linux_platform::{FileStore, PlatformError};

pub use driver::{Board, DriverParts, FilesDriver};

use crate::link::lock;
use crate::notify::{BoxFuture, Presenter};
use crate::view::{BackendError, EventSink};

/// A [`FileStore`] usable as a trait object.
pub trait Opener: Send + Sync {
    /// See [`FileStore::downloads_dir`].
    ///
    /// # Errors
    ///
    /// As [`FileStore::downloads_dir`].
    fn downloads_dir(&self) -> Result<PathBuf, PlatformError>;

    /// See [`FileStore::open`].
    fn open<'a>(&'a self, path: &'a Path) -> BoxFuture<'a, Result<(), PlatformError>>;

    /// See [`FileStore::show_in_folder`].
    fn show_in_folder<'a>(&'a self, path: &'a Path) -> BoxFuture<'a, Result<(), PlatformError>>;
}

impl<F: FileStore> Opener for F {
    fn downloads_dir(&self) -> Result<PathBuf, PlatformError> {
        FileStore::downloads_dir(self)
    }

    fn open<'a>(&'a self, path: &'a Path) -> BoxFuture<'a, Result<(), PlatformError>> {
        Box::pin(FileStore::open(self, path))
    }

    fn show_in_folder<'a>(&'a self, path: &'a Path) -> BoxFuture<'a, Result<(), PlatformError>> {
        Box::pin(FileStore::show_in_folder(self, path))
    }
}

/// Stands in for the desktop's files when the shell has none: nothing can
/// be received or opened.
pub(crate) struct NoFiles;

impl FileStore for NoFiles {
    fn downloads_dir(&self) -> Result<PathBuf, PlatformError> {
        Err(PlatformError::Unsupported("received files"))
    }

    async fn open(&self, _path: &Path) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported("opening files"))
    }

    async fn show_in_folder(&self, _path: &Path) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported("opening folders"))
    }
}

/// Whether files can be used on this desktop: the user's switch, and a
/// place for received files. Shared by every phone's driver.
#[derive(Debug)]
pub struct FilesSwitch {
    enabled: AtomicBool,
    can_receive: bool,
}

impl FilesSwitch {
    /// What this desktop reports for the feature: `UNAVAILABLE` without a
    /// Downloads directory, `DISABLED` while the user turned it off.
    #[must_use]
    pub fn availability(&self) -> Availability {
        if !self.can_receive {
            Availability::Unavailable
        } else if self.enabled.load(Ordering::Relaxed) {
            Availability::Ready
        } else {
            Availability::Disabled
        }
    }
}

/// Makes each phone's [`FilesDriver`], and keeps their boards for the
/// files page.
pub struct FilesFactory {
    opener: Arc<dyn Opener>,
    /// Where received files go, when the desktop says.
    downloads: Option<PathBuf>,
    switch: Arc<FilesSwitch>,
    boards: Mutex<HashMap<String, Board>>,
    /// What every phone's transfers still have to write into Downloads.
    incoming: Arc<IncomingBytes>,
}

impl FilesFactory {
    /// A factory whose drivers save into the Downloads directory that
    /// `opener` names, and open files through it, with files turned on when
    /// `enabled`. It removes what transfers of an earlier run left there,
    /// so make it once, when the app starts and before any session; that
    /// reads the directory, so it blocks.
    #[must_use]
    pub fn new(opener: Arc<dyn Opener>, enabled: bool) -> Self {
        let downloads = opener
            .downloads_dir()
            .inspect_err(
                |error| tracing::warn!(%error, "no Downloads directory for received files"),
            )
            .ok();
        if let Some(dir) = &downloads {
            match link2linux_core::files::remove_leftovers(dir) {
                Ok(0) => {}
                Ok(removed) => tracing::info!(removed, "removed unfinished downloads"),
                Err(error) => tracing::debug!(%error, "could not look for unfinished downloads"),
            }
        }
        let switch = Arc::new(FilesSwitch {
            enabled: AtomicBool::new(enabled),
            can_receive: downloads.is_some(),
        });
        Self {
            opener,
            downloads,
            switch,
            boards: Mutex::default(),
            incoming: IncomingBytes::new(),
        }
    }

    /// What this desktop reports for the feature now.
    #[must_use]
    pub fn availability(&self) -> Availability {
        self.switch.availability()
    }

    /// Turns files on or off. Each phone's driver tells its phone when it
    /// gets [`FilesDriver::switched`].
    pub fn set_enabled(&self, enabled: bool) {
        self.switch.enabled.store(enabled, Ordering::Relaxed);
    }

    /// The driver of phone `device_id`, named `phone_name`, which stays with
    /// it across sessions.
    pub fn create(
        &self,
        device_id: &str,
        phone_name: &str,
        events: Arc<dyn EventSink>,
        presenter: Arc<dyn Presenter>,
    ) -> FilesDriver {
        let board = Board::default();
        lock(&self.boards).insert(device_id.to_owned(), board.clone());
        // Without a Downloads directory offers are refused, so this one is
        // never written to.
        let dir = self.downloads.clone().unwrap_or_default();
        FilesDriver::new(DriverParts {
            service: FileService::new(dir).with_shared_incoming(&self.incoming),
            device_id: device_id.to_owned(),
            phone_name: phone_name.to_owned(),
            board,
            events,
            presenter,
            opener: self.opener.clone(),
            switch: self.switch.clone(),
        })
    }

    /// The transfers with phone `device_id`, oldest first.
    #[must_use]
    pub fn transfers(&self, device_id: &str) -> Option<Vec<Transfer>> {
        let board = lock(&self.boards).get(device_id).cloned()?;
        let mut transfers = lock(&board).clone();
        transfers.sort_by_key(|transfer| transfer.started_unix_ms);
        Some(transfers)
    }

    /// Where file `index` of transfer `transfer_id` from phone `device_id`
    /// was saved, once it arrived.
    #[must_use]
    pub fn received_file(
        &self,
        device_id: &str,
        transfer_id: &str,
        index: usize,
    ) -> Option<PathBuf> {
        let id = TransferId::from_hex(transfer_id)?;
        let board = lock(&self.boards).get(device_id).cloned()?;
        let board = lock(&board);
        let transfer = board.iter().find(|t| t.id == id)?;
        if transfer.direction != link2linux_core::files::Direction::Incoming {
            return None;
        }
        transfer.files.get(index)?.path.clone()
    }

    /// What opens and shows files.
    #[must_use]
    pub fn opener(&self) -> Arc<dyn Opener> {
        self.opener.clone()
    }

    /// Phone `device_id` is no longer paired: drops its transfers. The
    /// files it sent stay.
    pub fn forget(&self, device_id: &str) {
        lock(&self.boards).remove(device_id);
    }
}

/// Checks that the files at `paths` can go to a phone in one transfer,
/// telling the user why not. It looks at every file, so it blocks.
///
/// # Errors
///
/// Fails for a folder, a file that cannot be read, and for none or too
/// many files.
pub fn check_files(paths: &[PathBuf]) -> Result<(), BackendError> {
    check_sendable(paths).map_err(|error| send_error(&error))
}

/// What the user is told when files cannot be sent.
#[must_use]
pub fn send_error(error: &SendError) -> BackendError {
    match error {
        SendError::Directory(_) => BackendError::FolderNotSendable,
        SendError::TooMany => BackendError::TooManyFiles,
        SendError::NoFiles => BackendError::WrongState,
        _ => BackendError::FileUnreadable,
    }
}
