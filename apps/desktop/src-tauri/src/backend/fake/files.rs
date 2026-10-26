//! Pretend file transfers for the fake backend: files sent complete at
//! once, and the demo script starts with a few transfers to show.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use link2linux_runtime::files::check_files;
use link2linux_runtime::view::{
    TransferDirection, TransferFailure, TransferFileInfo, TransferStatus,
};

use super::script::Script;
use crate::backend::{BackendError, EventSink, FileTransfer, FileTransferEvent};

/// Each phone's pretend transfers.
pub struct FakeFiles {
    transfers: Mutex<HashMap<String, Vec<FileTransfer>>>,
    next_id: Mutex<u128>,
    sink: Arc<dyn EventSink>,
}

impl FakeFiles {
    pub fn new(script: Script, sink: Arc<dyn EventSink>, now_unix_ms: i64) -> Self {
        let mut transfers = HashMap::new();
        if script == Script::Demo {
            transfers.insert("demo-1".to_owned(), demo(now_unix_ms));
        }
        Self {
            transfers: Mutex::new(transfers),
            next_id: Mutex::new(0x100),
            sink,
        }
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<String, Vec<FileTransfer>>> {
        self.transfers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    pub fn transfers(&self, device_id: &str) -> Vec<FileTransfer> {
        self.lock().get(device_id).cloned().unwrap_or_default()
    }

    /// Checks the files as the real backend does, then pretends the phone
    /// took them all.
    pub fn send(
        &self,
        device_id: &str,
        paths: &[PathBuf],
        now_unix_ms: i64,
    ) -> Result<(), BackendError> {
        check_files(paths)?;
        let files: Vec<TransferFileInfo> = paths
            .iter()
            .map(|path| TransferFileInfo {
                name: path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                size: std::fs::metadata(path).map_or(0, |metadata| metadata.len()),
                mime_type: String::new(),
                saved: false,
            })
            .collect();
        let size = files.iter().map(|file| file.size).sum();
        let id = {
            let mut next = self.next_id.lock().unwrap_or_else(PoisonError::into_inner);
            *next += 1;
            format!("{:032x}", *next)
        };
        let transfer = FileTransfer {
            id,
            direction: TransferDirection::Outgoing,
            status: TransferStatus::Completed,
            files,
            size,
            done: size,
            started_unix_ms: now_unix_ms,
        };
        let mut transfers = self.lock();
        transfers
            .entry(device_id.to_owned())
            .or_default()
            .push(transfer.clone());
        self.sink
            .transfer_changed(device_id, &FileTransferEvent::Changed { transfer });
        Ok(())
    }

    pub fn cancel(&self, device_id: &str, transfer_id: &str) -> Result<(), BackendError> {
        let mut transfers = self.lock();
        let transfer = transfers
            .get_mut(device_id)
            .and_then(|list| list.iter_mut().find(|t| t.id == transfer_id))
            .filter(|t| !t.status.is_finished())
            .ok_or(BackendError::NotFound)?;
        transfer.status = TransferStatus::Cancelled { by_phone: false };
        let transfer = transfer.clone();
        self.sink
            .transfer_changed(device_id, &FileTransferEvent::Changed { transfer });
        Ok(())
    }

    pub fn forget(&self, device_id: &str, transfer_id: &str) -> Result<(), BackendError> {
        let mut transfers = self.lock();
        let list = transfers.get_mut(device_id).ok_or(BackendError::NotFound)?;
        let before = list.len();
        list.retain(|t| t.id != transfer_id || !t.status.is_finished());
        if list.len() == before {
            return Err(BackendError::NotFound);
        }
        self.sink.transfer_changed(
            device_id,
            &FileTransferEvent::Removed {
                id: transfer_id.to_owned(),
            },
        );
        Ok(())
    }

    /// Succeeds for a received file that arrived: there is nothing to open.
    pub fn received(
        &self,
        device_id: &str,
        transfer_id: &str,
        index: usize,
    ) -> Result<(), BackendError> {
        let saved = self.lock().get(device_id).is_some_and(|list| {
            list.iter()
                .any(|t| t.id == transfer_id && t.files.get(index).is_some_and(|file| file.saved))
        });
        if saved {
            Ok(())
        } else {
            Err(BackendError::NotFound)
        }
    }
}

/// What the demo phone has sent and received.
fn demo(now_unix_ms: i64) -> Vec<FileTransfer> {
    let file = |name: &str, size: u64, saved: bool| TransferFileInfo {
        name: name.to_owned(),
        size,
        mime_type: String::new(),
        saved,
    };
    let minute = 60_000;
    vec![
        FileTransfer {
            id: format!("{:032x}", 1),
            direction: TransferDirection::Incoming,
            status: TransferStatus::Completed,
            files: vec![
                file("IMG_2041.jpg", 2_412_118, true),
                file("IMG_2042.jpg", 2_198_540, true),
            ],
            size: 4_610_658,
            done: 4_610_658,
            started_unix_ms: now_unix_ms - 42 * minute,
        },
        FileTransfer {
            id: format!("{:032x}", 2),
            direction: TransferDirection::Outgoing,
            status: TransferStatus::Failed {
                reason: TransferFailure::NoSpace,
            },
            files: vec![file("backup.zip", 3_900_000_000, false)],
            size: 3_900_000_000,
            done: 0,
            started_unix_ms: now_unix_ms - 20 * minute,
        },
        FileTransfer {
            id: format!("{:032x}", 3),
            direction: TransferDirection::Incoming,
            status: TransferStatus::Interrupted,
            files: vec![file("Holiday.mp4", 184_000_000, false)],
            size: 184_000_000,
            done: 110_400_000,
            started_unix_ms: now_unix_ms - 5 * minute,
        },
    ]
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex as StdMutex;

    use super::*;
    use crate::backend::{Device, PairingState};

    #[derive(Default)]
    struct Recorder(StdMutex<Vec<FileTransferEvent>>);

    impl EventSink for Recorder {
        fn pairing_changed(&self, _: &PairingState) {}
        fn devices_changed(&self, _: &[Device]) {}
        fn transfer_changed(&self, _: &str, event: &FileTransferEvent) {
            self.0.lock().unwrap().push(event.clone());
        }
    }

    #[test]
    fn sends_complete_at_once_and_finished_ones_can_be_removed() {
        let recorder = Arc::new(Recorder::default());
        let files = FakeFiles::new(Script::Demo, recorder.clone(), 0);
        assert_eq!(files.transfers("demo-1").len(), 3);
        assert!(files.transfers("demo-2").is_empty());

        let path = std::env::current_exe().unwrap();
        files.send("demo-2", &[path], 5).unwrap();
        let sent = files.transfers("demo-2");
        assert_eq!(sent[0].status, TransferStatus::Completed);
        assert_eq!(sent[0].done, sent[0].size);
        assert_eq!(
            files.send("demo-2", &[PathBuf::from("/")], 5),
            Err(BackendError::FolderNotSendable)
        );

        let id = format!("{:032x}", 1);
        assert_eq!(files.received("demo-1", &id, 1), Ok(()));
        assert_eq!(
            files.received("demo-1", &id, 2),
            Err(BackendError::NotFound)
        );
        assert_eq!(files.cancel("demo-1", &id), Err(BackendError::NotFound));
        files.cancel("demo-1", &format!("{:032x}", 3)).unwrap();
        files.forget("demo-1", &id).unwrap();
        assert_eq!(files.transfers("demo-1").len(), 2);

        let events = recorder.0.lock().unwrap();
        assert_eq!(events.len(), 3);
        assert!(matches!(&events[2], FileTransferEvent::Removed { id: removed } if *removed == id));
    }
}
