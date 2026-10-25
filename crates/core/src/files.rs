//! Sending files in either direction.
//!
//! A transfer starts with an offer. The receiver accepts it, choosing where
//! the file goes, or declines it. Data then flows in chunks well below the
//! frame size limit, with progress reported along the way.
//!
//! Stub: the types and the [`Files`] trait are in place; the wire messages
//! come with this feature's schema.

use std::path::{Path, PathBuf};

use crate::device::FeatureError;

/// Identifies one transfer for its whole life.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TransferId(pub u64);

/// A file one device offers to send to the other.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileOffer {
    /// The transfer this offer starts.
    pub id: TransferId,
    /// File name, without any directory.
    pub name: String,
    /// Size in bytes.
    pub size: u64,
    /// MIME type, when the sender knows it.
    pub mime_type: Option<String>,
}

/// What happens to transfers, in either direction.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum FileEvent {
    /// The phone offers a file to the desktop.
    Offered(FileOffer),
    /// Bytes moved.
    Progress {
        /// The transfer.
        id: TransferId,
        /// Bytes transferred so far.
        done: u64,
        /// Total size.
        total: u64,
    },
    /// The file arrived complete.
    Completed {
        /// The transfer.
        id: TransferId,
        /// Where it was saved, for files received on the desktop.
        saved_to: Option<PathBuf>,
    },
    /// The transfer was declined, cancelled or failed.
    Failed {
        /// The transfer.
        id: TransferId,
        /// Why, for the user.
        reason: String,
    },
}

/// Requests the desktop makes about file transfers.
pub trait Files: Send + Sync {
    /// Offers the file at `path` to the phone and returns the transfer id.
    fn send_file(
        &self,
        path: &Path,
    ) -> impl Future<Output = Result<TransferId, FeatureError>> + Send;

    /// Accepts an offer from the phone and saves the file as `dest`.
    fn accept(
        &self,
        id: TransferId,
        dest: &Path,
    ) -> impl Future<Output = Result<(), FeatureError>> + Send;

    /// Declines an offer from the phone.
    fn decline(&self, id: TransferId) -> impl Future<Output = Result<(), FeatureError>> + Send;

    /// Stops a transfer in progress, in either direction.
    fn cancel(&self, id: TransferId) -> impl Future<Output = Result<(), FeatureError>> + Send;
}
