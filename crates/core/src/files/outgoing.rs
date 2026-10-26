//! A transfer to the phone: its files, where sending stands, and the hash
//! states it can go on from.
//!
//! The hash of each file covers all of it, so a transfer that goes on in
//! the middle of a file needs the hash of the bytes before. The sender
//! keeps the hash state at the end of every chunk the phone has not
//! answered, and at the last one it answered: a phone that resumes where
//! it stopped always lands on one of those. Anywhere else, the bytes before
//! are read and hashed again.

use std::fs::{self, File};
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use aws_lc_rs::digest;
use link2linux_protocol::v1::{FileAccept, FileChunk, FileInfo, FileOffer};
use tokio::time::Instant;

use super::disk::{self, SourceError, blocking};
use super::{
    CHUNK_LEN, Direction, MAX_FILES, Transfer, TransferFile, TransferId, TransferState, names,
};

/// Why files cannot be sent.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SendError {
    /// No files were given.
    #[error("no files to send")]
    NoFiles,
    /// More than [`MAX_FILES`] files were given.
    #[error("at most {MAX_FILES} files go in one transfer")]
    TooMany,
    /// One of them is a folder, which version 1 cannot send.
    #[error("folders cannot be sent")]
    Directory(PathBuf),
    /// One of them is not a regular file.
    #[error("only regular files can be sent")]
    NotAFile(PathBuf),
    /// One of them cannot be read.
    #[error("a file cannot be read: {source}")]
    Unreadable {
        /// The file.
        path: PathBuf,
        /// Why.
        source: io::Error,
    },
    /// Looking at the files failed.
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// One file to send.
struct Source {
    path: PathBuf,
    /// The name it goes out with.
    name: String,
    size: u64,
    mime_type: String,
    /// When it was last modified as the user sent it. A file whose time
    /// changed is not the one the user sent anymore.
    modified: Option<SystemTime>,
}

impl Source {
    /// Looks at the file at `path`.
    fn new(path: PathBuf) -> Result<Self, SendError> {
        let unreadable = |path: &PathBuf, source| SendError::Unreadable {
            path: path.clone(),
            source,
        };
        let metadata = fs::metadata(&path).map_err(|e| unreadable(&path, e))?;
        if metadata.is_dir() {
            return Err(SendError::Directory(path));
        }
        if !metadata.is_file() {
            return Err(SendError::NotAFile(path));
        }
        File::open(&path).map_err(|e| unreadable(&path, e))?;
        let name = names::outgoing(path.file_name().unwrap_or_default());
        Ok(Self {
            mime_type: names::mime_type(&name).to_owned(),
            name,
            size: metadata.len(),
            modified: metadata.modified().ok(),
            path,
        })
    }
}

/// Checks that the files at `paths` can go in one transfer, as
/// [`FileService::send`](super::FileService::send) will, so the user hears
/// at once why not. It looks at every file, so it blocks.
///
/// # Errors
///
/// As [`FileService::send`](super::FileService::send).
pub fn check_sendable(paths: &[PathBuf]) -> Result<(), SendError> {
    if paths.is_empty() {
        return Err(SendError::NoFiles);
    }
    if paths.len() > MAX_FILES {
        return Err(SendError::TooMany);
    }
    paths
        .iter()
        .try_for_each(|path| Source::new(path.clone()).map(drop))
}

/// What the answer to a chunk confirms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Mark {
    pub(super) file: usize,
    pub(super) end: u64,
    pub(super) len: usize,
}

/// A transfer this desktop sends to the phone.
pub(super) struct Outgoing {
    pub(super) id: TransferId,
    pub(super) state: TransferState,
    /// Whether an offer went out, in any session: the phone may then hold
    /// partial files that a cancel has to remove.
    pub(super) offered: bool,
    /// Whether the phone accepted the offer in this session, so chunks go
    /// out.
    pub(super) accepted: bool,
    /// Grows whenever the answers to earlier requests stop counting.
    pub(super) generation: u32,
    /// Chunks waiting for their answer.
    pub(super) in_flight: usize,
    /// Answers missed in a row.
    pub(super) misses: u32,
    /// It missed too many answers: it is offered again in the next
    /// session, not in this one.
    pub(super) held: bool,
    /// How often the phone failed its offer with `INTERNAL`.
    pub(super) internal_errors: usize,
    /// When it started waiting or was interrupted, for expiry.
    pub(super) since: Instant,
    /// When a busy phone may be asked again.
    pub(super) retry_at: Option<Instant>,
    started_unix_ms: i64,
    files: Vec<Source>,
    /// Where the next chunk starts.
    file: usize,
    sent: u64,
    reader: Option<Arc<File>>,
    /// The hash of the `sent` bytes, or `None` when it has to be read again.
    hash: Option<digest::Context>,
    /// The hash state at chunk ends the phone may go on from.
    marks: Vec<(usize, u64, digest::Context)>,
    /// Bytes the phone confirmed.
    acked: u64,
}

impl Outgoing {
    /// A transfer of the files at `paths`, waiting to be offered.
    pub(super) async fn new(paths: Vec<PathBuf>) -> Result<Self, SendError> {
        if paths.is_empty() {
            return Err(SendError::NoFiles);
        }
        if paths.len() > MAX_FILES {
            return Err(SendError::TooMany);
        }
        let files = blocking(move || {
            paths
                .into_iter()
                .map(Source::new)
                .collect::<Result<Vec<_>, SendError>>()
        })
        .await?;
        Ok(Self {
            id: TransferId::random(),
            state: TransferState::Waiting,
            offered: false,
            accepted: false,
            generation: 0,
            in_flight: 0,
            misses: 0,
            held: false,
            internal_errors: 0,
            since: Instant::now(),
            retry_at: None,
            started_unix_ms: unix_ms(SystemTime::now()),
            files,
            file: 0,
            sent: 0,
            reader: None,
            hash: Some(new_hash()),
            marks: Vec::new(),
            acked: 0,
        })
    }

    /// The offer for the phone.
    pub(super) fn offer(&self) -> FileOffer {
        FileOffer {
            transfer_id: self.id.0.to_vec(),
            created_unix_ms: self.started_unix_ms,
            files: self
                .files
                .iter()
                .map(|source| FileInfo {
                    name: source.name.clone(),
                    size: source.size,
                    mime_type: source.mime_type.clone(),
                    modified_unix_ms: source.modified.map_or(0, unix_ms),
                })
                .collect(),
        }
    }

    /// Goes on from where the phone's `accept` says, and says what is
    /// wrong with it otherwise.
    pub(super) fn accept(&mut self, accept: &FileAccept) -> Result<(), &'static str> {
        let file = usize::try_from(accept.file_index)
            .ok()
            .filter(|file| *file <= self.files.len())
            .ok_or("file_index is past the last file")?;
        let offset = accept.offset;
        // Every byte of a file but none of its hash cannot happen: the
        // last chunk is never empty unless the file is.
        let size = self.files.get(file).map_or(0, |source| source.size);
        if offset != 0 && offset >= size {
            return Err("offset is past the bytes still to send");
        }
        self.hash = if offset == 0 {
            Some(new_hash())
        } else {
            self.marks
                .iter()
                .find(|(f, o, _)| (*f, *o) == (file, offset))
                .map(|(_, _, hash)| hash.clone())
        };
        self.marks.retain(|(f, o, _)| (*f, *o) == (file, offset));
        self.file = file;
        self.sent = offset;
        self.reader = None;
        self.acked = self.files[..file]
            .iter()
            .fold(offset, |total, source| total.saturating_add(source.size));
        self.accepted = true;
        self.misses = 0;
        Ok(())
    }

    /// Bytes the phone confirmed.
    pub(super) fn done(&self) -> u64 {
        self.acked
    }

    pub(super) fn started_unix_ms(&self) -> i64 {
        self.started_unix_ms
    }

    /// Whether chunks remain to be sent.
    pub(super) fn has_more(&self) -> bool {
        self.accepted && self.state == TransferState::Running && self.file < self.files.len()
    }

    /// Whether the phone has every byte.
    pub(super) fn is_complete(&self) -> bool {
        self.accepted && self.file == self.files.len() && self.in_flight == 0
    }

    /// Reads the next chunk, and moves past it.
    pub(super) async fn next_chunk(&mut self) -> Result<(FileChunk, Mark), SourceError> {
        let index = self.file;
        let (size, offset) = (self.files[index].size, self.sent);
        let stamp = (size, self.files[index].modified);
        let reader = match &self.reader {
            Some(reader) => reader.clone(),
            None => {
                let source = &self.files[index];
                let (path, modified) = (source.path.clone(), source.modified);
                let reader =
                    Arc::new(blocking(move || disk::open_source(&path, size, modified)).await?);
                self.reader = Some(reader.clone());
                reader
            }
        };
        let mut hash = match self.hash.take() {
            Some(hash) => hash,
            None => {
                let file = reader.clone();
                blocking(move || disk::hash_prefix(&file, offset)).await?
            }
        };
        let len = usize::try_from(size - offset).map_or(CHUNK_LEN, |left| left.min(CHUNK_LEN));
        let (data, hash) = blocking(move || {
            disk::read_chunk(&reader, offset, len, stamp, &mut hash).map(|data| (data, hash))
        })
        .await?;
        let end = offset + len as u64;
        let sha256 = if end == size {
            self.file += 1;
            self.sent = 0;
            self.reader = None;
            self.hash = Some(new_hash());
            hash.finish().as_ref().to_vec()
        } else {
            self.sent = end;
            self.marks.push((index, end, hash.clone()));
            self.hash = Some(hash);
            Vec::new()
        };
        let chunk = FileChunk {
            transfer_id: self.id.0.to_vec(),
            file_index: u32::try_from(index).unwrap_or(u32::MAX),
            offset,
            data,
            sha256,
        };
        Ok((
            chunk,
            Mark {
                file: index,
                end,
                len,
            },
        ))
    }

    /// The phone wrote the chunk that `mark` describes.
    pub(super) fn acked(&mut self, mark: Mark) {
        self.in_flight = self.in_flight.saturating_sub(1);
        self.acked = self.acked.saturating_add(mark.len as u64);
        self.misses = 0;
        // The phone writes in order, so it never goes on from before here.
        self.marks
            .retain(|(f, o, _)| (*f, *o) >= (mark.file, mark.end));
    }

    /// Stops sending until the phone accepts a new offer, which happens
    /// once the transfer is `state` again. Answers to what went out before
    /// no longer count.
    pub(super) fn pause(&mut self, state: TransferState, now: Instant) {
        self.state = state;
        self.since = now;
        self.stop();
    }

    /// Ends the transfer as `state`.
    pub(super) fn finish(&mut self, state: TransferState) {
        self.state = state;
        self.stop();
        self.marks.clear();
        self.hash = None;
    }

    fn stop(&mut self) {
        self.accepted = false;
        self.generation = self.generation.wrapping_add(1);
        self.in_flight = 0;
        self.reader = None;
    }

    /// The transfer as the user sees it.
    pub(super) fn view(&self) -> Transfer {
        Transfer {
            id: self.id,
            direction: Direction::Outgoing,
            state: self.state.clone(),
            files: self
                .files
                .iter()
                .map(|source| TransferFile {
                    name: source.name.clone(),
                    size: source.size,
                    mime_type: source.mime_type.clone(),
                    path: Some(source.path.clone()),
                })
                .collect(),
            size: self
                .files
                .iter()
                .fold(0, |total, source| total.saturating_add(source.size)),
            done: self.acked,
            started_unix_ms: self.started_unix_ms,
        }
    }
}

fn new_hash() -> digest::Context {
    digest::Context::new(&digest::SHA256)
}

/// `time` in Unix milliseconds, or 0 before 1970.
pub(super) fn unix_ms(time: SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH).map_or(0, |since| {
        i64::try_from(since.as_millis()).unwrap_or(i64::MAX)
    })
}
