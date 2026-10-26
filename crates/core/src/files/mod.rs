//! Sending files in either direction.
//!
//! A transfer is the files the user sent together. The sender offers them
//! with `FileOffer`, and the receiver accepts every offer while the feature
//! is ready, without asking the user, by answering where the bytes start.
//! The bytes follow in order, as `FileChunk`s of at most [`CHUNK_LEN`]
//! bytes with a few unanswered at a time; the last chunk of each file
//! carries the file's SHA-256. A transfer cut short by a lost connection is
//! offered again once the devices reconnect, and goes on from the first
//! byte the receiver does not have. `docs/protocol.md`, "Files", gives the
//! rules.
//!
//! [`FileService`] does both sides for one phone. Like the notifications
//! service it does not own the session, so it can share one with other
//! features:
//!
//! 1. [`FileService::session_started`] once the session is up;
//! 2. every [`SessionEvent`](crate::SessionEvent) through
//!    [`FileService::handle_event`], which keeps the file events and hands
//!    back the rest;
//! 3. the user's sends and cancels through [`FileService::send`] and
//!    [`FileService::cancel`], with or without a session;
//! 4. [`FileService::session_ended`] when the session is gone, and
//!    [`FileService::expire`] by [`FileService::next_deadline`].
//!
//! The files page reads [`FileService::transfers`] and follows
//! [`FileService::subscribe`].
//!
//! Received files go into one directory, the user's Downloads on the
//! desktop. Each is written under a hidden temporary name and gets its own
//! name only once its hash matches; [`remove_leftovers`] cleans up what a
//! run of the app that ended mid-transfer left there.

mod disk;
mod incoming;
mod names;
mod outgoing;
mod service;

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub use outgoing::{SendError, check_sendable};
pub use service::{FileService, ServiceError};

/// The most bytes of a file one chunk carries.
pub const CHUNK_LEN: usize = 256 * 1024;

/// The most chunks a sender leaves unanswered in one session, across all
/// its transfers.
pub const MAX_UNANSWERED_CHUNKS: usize = 8;

/// The most transfers from one device that run at the same time.
pub const MAX_RUNNING_TRANSFERS: usize = 4;

/// The most files in one transfer.
pub const MAX_FILES: usize = 500;

/// How long an unfinished transfer is kept for the devices to reconnect.
pub const KEEP_UNFINISHED: Duration = Duration::from_secs(60 * 60);

/// Names one transfer on both devices: 16 random bytes.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TransferId(pub [u8; 16]);

impl TransferId {
    /// A new random id.
    ///
    /// # Panics
    ///
    /// Panics if the system random generator fails.
    #[must_use]
    pub fn random() -> Self {
        let mut bytes = [0; 16];
        aws_lc_rs::rand::fill(&mut bytes).expect("the system random generator failed");
        Self(bytes)
    }

    /// The id in a message, if it has the right length.
    #[must_use]
    pub fn from_wire(bytes: &[u8]) -> Option<Self> {
        bytes.try_into().ok().map(Self)
    }

    /// Parses the 32 hex digits that [`TransferId`]'s `Display` writes.
    #[must_use]
    pub fn from_hex(hex: &str) -> Option<Self> {
        if hex.len() != 32 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let mut bytes = [0; 16];
        for (byte, pair) in bytes.iter_mut().zip(hex.as_bytes().chunks(2)) {
            let pair = std::str::from_utf8(pair).ok()?;
            *byte = u8::from_str_radix(pair, 16).ok()?;
        }
        Some(Self(bytes))
    }
}

impl fmt::Display for TransferId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.iter().try_for_each(|byte| write!(f, "{byte:02x}"))
    }
}

impl fmt::Debug for TransferId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "TransferId({self})")
    }
}

/// Which way the files go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// From the phone to this desktop.
    Incoming,
    /// From this desktop to the phone.
    Outgoing,
}

/// Where a transfer stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransferState {
    /// Not offered yet: the phone is not connected, the feature is not
    /// ready, or the phone is busy with other transfers.
    Waiting,
    /// Offered, or bytes are moving.
    Running,
    /// The connection dropped. The transfer goes on when the phone is back,
    /// for up to [`KEEP_UNFINISHED`].
    Interrupted,
    /// Every file arrived.
    Completed,
    /// The user cancelled it, on this device or, with `by_peer`, on the
    /// phone.
    Cancelled {
        /// Whether the phone cancelled it.
        by_peer: bool,
    },
    /// It failed.
    Failed(Failure),
}

impl TransferState {
    /// Whether the transfer is over, for good or not.
    #[must_use]
    pub fn is_finished(&self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Cancelled { .. } | Self::Failed(_)
        )
    }
}

/// Why a transfer failed, for the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Failure {
    /// The phone has the feature turned off.
    Disabled,
    /// The phone needs a permission before it can save files.
    PermissionRequired,
    /// The receiving device has no room for the files.
    NoSpace,
    /// The phone refused the transfer or a part of it.
    Refused,
    /// A file to send could not be read.
    Unreadable,
    /// A file changed while it was being sent.
    Changed,
    /// A received file did not match its hash: it was damaged on the way.
    Damaged,
    /// The phone sent bytes that do not fit the transfer.
    Invalid,
    /// A received file could not be saved here.
    WriteFailed,
    /// The phone stopped answering.
    Unanswered,
    /// The phone stayed away for longer than [`KEEP_UNFINISHED`].
    Expired,
}

/// One file of a transfer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferFile {
    /// Its name: the local one for a file sent, the safe one for a file
    /// received, with the number it got when its name was taken.
    pub name: String,
    /// Size in bytes.
    pub size: u64,
    /// The MIME type, or empty when not known.
    pub mime_type: String,
    /// The local file: the one sent, or where a received file was saved
    /// once it is complete.
    pub path: Option<PathBuf>,
}

/// A transfer as the user sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transfer {
    /// Its id.
    pub id: TransferId,
    /// Which way it goes.
    pub direction: Direction,
    /// Where it stands.
    pub state: TransferState,
    /// Its files, in order.
    pub files: Vec<TransferFile>,
    /// Bytes of all files.
    pub size: u64,
    /// Bytes the receiver has, as far as this side knows.
    pub done: u64,
    /// When it started on this device, in Unix milliseconds.
    pub started_unix_ms: i64,
}

/// A change to the transfers, for the files page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransferEvent {
    /// A transfer started, or its state or a file changed.
    Changed(Transfer),
    /// Bytes moved.
    Progress {
        /// The transfer.
        id: TransferId,
        /// [`Transfer::done`] now.
        done: u64,
    },
    /// A finished transfer left the list.
    Removed(TransferId),
}

/// Removes the temporary files that transfers left in `dir` when the app
/// ended before they finished, and returns how many there were. Call it
/// when the app starts, before any session: transfers are never kept
/// across a restart.
///
/// # Errors
///
/// Fails when `dir` cannot be read.
pub async fn remove_leftovers(dir: &Path) -> io::Result<usize> {
    let dir = dir.to_owned();
    disk::blocking(move || disk::remove_leftovers(&dir)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transfer_ids_print_and_parse_as_hex() {
        let id = TransferId([0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 0xff]);
        let hex = id.to_string();
        assert_eq!(hex, "000102030405060708090a0b0c0d0eff");
        assert_eq!(TransferId::from_hex(&hex), Some(id));
        assert_eq!(TransferId::from_hex(&hex.to_uppercase()), Some(id));
        assert_eq!(TransferId::from_hex(&hex[1..]), None);
        assert_eq!(TransferId::from_hex(&"zz".repeat(16)), None);
        assert_eq!(TransferId::from_hex(&"é".repeat(16)), None);
        assert_eq!(TransferId::from_wire(&id.0), Some(id));
        assert_eq!(TransferId::from_wire(&[0; 15]), None);
        assert_ne!(TransferId::random(), TransferId::random());
    }
}
