//! A transfer from the phone: where its bytes go, and the checks they
//! pass on the way.

use std::fs::File;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;

use aws_lc_rs::digest;
use link2linux_protocol::v1::{FileAccept, FileChunk, FileOffer};
use tokio::time::Instant;

use super::disk::{self, blocking};
use super::{CHUNK_LEN, Direction, Transfer, TransferFile, TransferId, TransferState, names};

/// One file of the transfer.
struct Target {
    /// The name and size the phone offered, to recognise a repeated offer.
    offered_name: String,
    size: u64,
    mime_type: String,
    /// The safe name it is saved under, before any number.
    name: String,
    /// Where it is written until its hash matches.
    temp: PathBuf,
    /// Where it was saved once complete.
    saved_to: Option<PathBuf>,
}

/// What a chunk completed.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Written {
    /// Nothing yet: the file goes on.
    Part,
    /// A file, with more to come.
    File,
    /// The last file: the transfer is complete.
    All,
}

/// Why a chunk could not be written. Either way the transfer is over.
#[derive(Debug, thiserror::Error)]
pub(super) enum WriteError {
    /// The file's hash does not match. It has been deleted.
    #[error("the file does not match its hash")]
    Damaged,
    /// Writing or saving the file failed.
    #[error(transparent)]
    Disk(#[from] io::Error),
}

/// A transfer the phone sends to this desktop.
pub(super) struct Incoming {
    pub(super) id: TransferId,
    pub(super) state: TransferState,
    /// Whether the phone offered it in the current session. Only then are
    /// its chunks taken.
    pub(super) live: bool,
    /// When it was interrupted, for expiry.
    pub(super) since: Instant,
    started_unix_ms: i64,
    dir: PathBuf,
    files: Vec<Target>,
    /// The file being written, and how many of its bytes are.
    file: usize,
    written: u64,
    /// Bytes of the files before `file`, plus `written`.
    done: u64,
    writer: Option<Arc<File>>,
    /// The hash of the `written` bytes. Taken while a chunk is being
    /// written.
    hash: Option<digest::Context>,
}

impl Incoming {
    /// A new transfer of the files in `offer`, saved in `dir`. The offer
    /// has passed the protocol's limits.
    pub(super) fn new(
        id: TransferId,
        offer: &FileOffer,
        dir: PathBuf,
        started_unix_ms: i64,
    ) -> Self {
        let files = offer
            .files
            .iter()
            .enumerate()
            .map(|(index, info)| Target {
                offered_name: info.name.clone(),
                size: info.size,
                // The phone's type is only a hint: a saved file is typed by
                // its name.
                mime_type: names::mime_type(&names::received(&info.name)).to_owned(),
                name: names::received(&info.name),
                temp: dir.join(disk::temp_name(id, index)),
                saved_to: None,
            })
            .collect();
        Self {
            id,
            state: TransferState::Running,
            live: true,
            since: Instant::now(),
            started_unix_ms,
            dir,
            files,
            file: 0,
            written: 0,
            done: 0,
            writer: None,
            hash: Some(new_hash()),
        }
    }

    /// Whether `offer` names the same files, in the same order and sizes.
    pub(super) fn matches(&self, offer: &FileOffer) -> bool {
        self.files.len() == offer.files.len()
            && self
                .files
                .iter()
                .zip(&offer.files)
                .all(|(target, info)| target.offered_name == info.name && target.size == info.size)
    }

    /// Where the bytes go on: the first file not complete and how much of
    /// it is written, or past the last file when all are.
    pub(super) fn resume_point(&self) -> FileAccept {
        FileAccept {
            file_index: u32::try_from(self.file).unwrap_or(u32::MAX),
            offset: self.written,
        }
    }

    /// Bytes still to come.
    pub(super) fn remaining(&self) -> u64 {
        self.size().saturating_sub(self.done)
    }

    pub(super) fn done(&self) -> u64 {
        self.done
    }

    pub(super) fn started_unix_ms(&self) -> i64 {
        self.started_unix_ms
    }

    /// How many files the transfer has.
    pub(super) fn file_count(&self) -> usize {
        self.files.len()
    }

    /// Bytes of all its files.
    pub(super) fn size(&self) -> u64 {
        self.files
            .iter()
            .fold(0, |total, target| total.saturating_add(target.size))
    }

    /// Checks that `chunk` continues the transfer where it stands, and
    /// says what is wrong otherwise, for the phone's logs.
    pub(super) fn check(&self, chunk: &FileChunk) -> Result<(), &'static str> {
        let Some(target) = self.files.get(self.file) else {
            return Err("the transfer is complete");
        };
        if usize::try_from(chunk.file_index).ok() != Some(self.file) {
            return Err("the chunk is not of the file being written");
        }
        if chunk.offset != self.written {
            return Err("the chunk does not start where the file stands");
        }
        if chunk.data.len() > CHUNK_LEN {
            return Err("the chunk is too long");
        }
        let end = chunk.offset.saturating_add(chunk.data.len() as u64);
        if end > target.size {
            return Err("the chunk runs past the end of the file");
        }
        if chunk.data.is_empty() && target.size != 0 {
            return Err("an empty chunk of a file that is not empty");
        }
        match (end == target.size, chunk.sha256.len()) {
            (true, 32) | (false, 0) => Ok(()),
            (true, _) => Err("the last chunk of a file needs its SHA-256"),
            (false, _) => Err("only the last chunk of a file carries a hash"),
        }
    }

    /// Writes `chunk`, which passed [`Incoming::check`]. A file whose last
    /// chunk this is gets its own name if its hash matches, and is deleted
    /// otherwise.
    pub(super) async fn write(&mut self, chunk: FileChunk) -> Result<Written, WriteError> {
        let index = self.file;
        let writer = match &self.writer {
            Some(writer) => writer.clone(),
            None => {
                let temp = self.files[index].temp.clone();
                let writer = Arc::new(blocking(move || disk::create_temp(&temp)).await?);
                self.writer = Some(writer.clone());
                writer
            }
        };
        let mut hash = self.hash.take().unwrap_or_else(new_hash);
        let (offset, len, data) = (chunk.offset, chunk.data.len() as u64, chunk.data);
        let file = writer.clone();
        let hash =
            blocking(move || disk::write_chunk(&file, offset, &data, &mut hash).map(|()| hash))
                .await?;
        self.written += len;
        self.done += len;
        if self.written < self.files[index].size {
            self.hash = Some(hash);
            return Ok(Written::Part);
        }

        self.writer = None;
        self.hash = Some(new_hash());
        let target = &self.files[index];
        if hash.finish().as_ref() != chunk.sha256.as_slice() {
            let temp = target.temp.clone();
            blocking(move || disk::remove_if_present(&temp)).await?;
            return Err(WriteError::Damaged);
        }
        let (temp, dir, name) = (target.temp.clone(), self.dir.clone(), target.name.clone());
        let saved = blocking(move || disk::publish(&writer, &temp, &dir, &name)).await?;
        tracing::debug!(path = %saved.display(), "received a file");
        self.files[index].saved_to = Some(saved);
        self.file += 1;
        self.written = 0;
        Ok(if self.file == self.files.len() {
            Written::All
        } else {
            Written::File
        })
    }

    /// Deletes what the transfer has not finished; the finished files stay.
    pub(super) async fn discard(&mut self) {
        self.writer = None;
        let Some(target) = self.files.get(self.file) else {
            return;
        };
        let temp = target.temp.clone();
        if let Err(error) = blocking(move || disk::remove_if_present(&temp)).await {
            tracing::warn!(%error, "could not delete an unfinished download");
        }
    }

    /// The transfer as the user sees it.
    pub(super) fn view(&self) -> Transfer {
        Transfer {
            id: self.id,
            direction: Direction::Incoming,
            state: self.state.clone(),
            files: self
                .files
                .iter()
                .map(|target| TransferFile {
                    name: target
                        .saved_to
                        .as_ref()
                        .and_then(|path| path.file_name())
                        .map_or_else(
                            || target.name.clone(),
                            |name| name.to_string_lossy().into_owned(),
                        ),
                    size: target.size,
                    mime_type: target.mime_type.clone(),
                    path: target.saved_to.clone(),
                })
                .collect(),
            size: self.size(),
            done: self.done,
            started_unix_ms: self.started_unix_ms,
        }
    }
}

fn new_hash() -> digest::Context {
    digest::Context::new(&digest::SHA256)
}
