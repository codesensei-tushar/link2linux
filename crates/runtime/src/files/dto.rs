//! File transfers as the UI receives them.
//!
//! Local paths stay out of these types: the page names a file by its
//! transfer and its position, and the runtime looks the path up when the
//! user opens it.

use link2linux_core::files::{Direction, Failure, Transfer, TransferEvent, TransferState};
use serde::{Deserialize, Serialize};
use specta::Type;

/// Which way the files go.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum TransferDirection {
    /// From the phone to this computer.
    Incoming,
    /// From this computer to the phone.
    Outgoing,
}

/// Why a transfer failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum TransferFailure {
    /// Files are turned off on the phone.
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
    /// A received file was damaged on the way.
    Damaged,
    /// The phone sent bytes that do not fit the transfer.
    Invalid,
    /// A received file could not be saved.
    WriteFailed,
    /// The phone stayed away for more than an hour.
    Expired,
}

/// Where a transfer stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum TransferStatus {
    /// Not started: the phone is away, not ready, or busy.
    Waiting,
    /// Bytes are moving.
    Running,
    /// The connection dropped; it goes on when the phone is back.
    Interrupted,
    Completed,
    #[serde(rename_all = "camelCase")]
    Cancelled {
        /// Cancelled on the phone rather than here.
        by_phone: bool,
    },
    Failed {
        reason: TransferFailure,
    },
}

impl TransferStatus {
    /// Whether the transfer is over.
    #[must_use]
    pub fn is_finished(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Cancelled { .. } | Self::Failed { .. }
        )
    }
}

/// One file of a transfer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct TransferFileInfo {
    /// The file's name: as sent, or as saved here, with the number it got
    /// when the name was taken.
    pub name: String,
    #[specta(type = specta_typescript::Number)]
    pub size: u64,
    /// The MIME type, or empty when not known.
    pub mime_type: String,
    /// A received file that arrived, which the user can open.
    pub saved: bool,
}

/// A transfer with a phone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct FileTransfer {
    /// 32 hexadecimal digits.
    pub id: String,
    pub direction: TransferDirection,
    pub status: TransferStatus,
    pub files: Vec<TransferFileInfo>,
    /// Bytes of all files.
    #[specta(type = specta_typescript::Number)]
    pub size: u64,
    /// Bytes the receiver has.
    #[specta(type = specta_typescript::Number)]
    pub done: u64,
    /// When it started on this computer.
    #[specta(type = specta_typescript::Number)]
    pub started_unix_ms: i64,
}

/// A change to a phone's transfers, as pushed to the UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum FileTransferEvent {
    /// A transfer started, or its status or a file changed: the whole
    /// transfer, which replaces the earlier one.
    Changed { transfer: FileTransfer },
    /// Bytes moved.
    Progress {
        id: String,
        #[specta(type = specta_typescript::Number)]
        done: u64,
    },
    /// A finished transfer left the list.
    Removed { id: String },
}

/// The UI's view of `transfer`.
#[must_use]
pub fn transfer(transfer: &Transfer) -> FileTransfer {
    FileTransfer {
        id: transfer.id.to_string(),
        direction: match transfer.direction {
            Direction::Incoming => TransferDirection::Incoming,
            Direction::Outgoing => TransferDirection::Outgoing,
        },
        status: status(&transfer.state),
        files: transfer
            .files
            .iter()
            .map(|file| TransferFileInfo {
                name: file.name.clone(),
                size: file.size,
                mime_type: file.mime_type.clone(),
                saved: transfer.direction == Direction::Incoming && file.path.is_some(),
            })
            .collect(),
        size: transfer.size,
        done: transfer.done,
        started_unix_ms: transfer.started_unix_ms,
    }
}

/// The UI's view of `event`.
#[must_use]
pub fn event(event: &TransferEvent) -> FileTransferEvent {
    match event {
        TransferEvent::Changed(changed) => FileTransferEvent::Changed {
            transfer: transfer(changed),
        },
        TransferEvent::Progress { id, done } => FileTransferEvent::Progress {
            id: id.to_string(),
            done: *done,
        },
        TransferEvent::Removed(id) => FileTransferEvent::Removed { id: id.to_string() },
    }
}

fn status(state: &TransferState) -> TransferStatus {
    match state {
        TransferState::Waiting => TransferStatus::Waiting,
        TransferState::Running => TransferStatus::Running,
        TransferState::Interrupted => TransferStatus::Interrupted,
        TransferState::Completed => TransferStatus::Completed,
        TransferState::Cancelled { by_peer } => TransferStatus::Cancelled { by_phone: *by_peer },
        TransferState::Failed(failure) => TransferStatus::Failed {
            reason: self::failure(*failure),
        },
    }
}

fn failure(failure: Failure) -> TransferFailure {
    use TransferFailure as F;
    match failure {
        Failure::Disabled => F::Disabled,
        Failure::PermissionRequired => F::PermissionRequired,
        Failure::NoSpace => F::NoSpace,
        Failure::Unreadable => F::Unreadable,
        Failure::Changed => F::Changed,
        Failure::Damaged => F::Damaged,
        Failure::Invalid => F::Invalid,
        Failure::WriteFailed => F::WriteFailed,
        Failure::Expired => F::Expired,
        // A reason added later reads as a refusal until it has its own.
        _ => F::Refused,
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use link2linux_core::files::{TransferFile, TransferId};

    use super::*;

    fn received(state: TransferState) -> Transfer {
        Transfer {
            id: TransferId([0xab; 16]),
            direction: Direction::Incoming,
            state,
            files: vec![
                TransferFile {
                    name: "photo (1).jpg".into(),
                    size: 10,
                    mime_type: "image/jpeg".into(),
                    path: Some(PathBuf::from("/home/ana/Downloads/photo (1).jpg")),
                },
                TransferFile {
                    name: "notes.txt".into(),
                    size: 5,
                    mime_type: String::new(),
                    path: None,
                },
            ],
            size: 15,
            done: 12,
            started_unix_ms: 1_700_000_000_000,
        }
    }

    #[test]
    fn transfers_carry_names_and_progress_but_no_paths() {
        let view = transfer(&received(TransferState::Running));
        assert_eq!(view.id, "ab".repeat(16));
        assert_eq!(view.direction, TransferDirection::Incoming);
        assert_eq!(view.status, TransferStatus::Running);
        assert_eq!((view.size, view.done), (15, 12));
        assert!(view.files[0].saved);
        assert!(!view.files[1].saved);
        let json = serde_json::to_string(&view).unwrap();
        assert!(!json.contains("Downloads"), "{json}");
        assert!(json.contains("\"startedUnixMs\":1700000000000"), "{json}");

        let mut sent = received(TransferState::Completed);
        sent.direction = Direction::Outgoing;
        assert!(transfer(&sent).files.iter().all(|f| !f.saved));
    }

    #[test]
    fn states_and_events_read_as_the_page_expects() {
        let cancelled = transfer(&received(TransferState::Cancelled { by_peer: true }));
        assert_eq!(
            serde_json::to_value(cancelled.status).unwrap(),
            serde_json::json!({ "kind": "cancelled", "byPhone": true })
        );
        let failed = transfer(&received(TransferState::Failed(Failure::NoSpace)));
        assert_eq!(
            serde_json::to_value(failed.status).unwrap(),
            serde_json::json!({ "kind": "failed", "reason": "noSpace" })
        );
        assert!(failed.status.is_finished());
        assert!(!TransferStatus::Interrupted.is_finished());

        let progress = event(&TransferEvent::Progress {
            id: TransferId([1; 16]),
            done: 7,
        });
        assert_eq!(
            serde_json::to_value(progress).unwrap(),
            serde_json::json!({ "type": "progress", "id": "01".repeat(16), "done": 7 })
        );
        let removed = event(&TransferEvent::Removed(TransferId([2; 16])));
        assert_eq!(
            removed,
            FileTransferEvent::Removed {
                id: "02".repeat(16)
            }
        );
    }
}
