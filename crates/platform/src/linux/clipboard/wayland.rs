//! The clipboard through Wayland data-control (`ext-data-control-v1`, or
//! `zwlr-data-control-unstable-v1`), which KDE Plasma, Sway, Hyprland, niri
//! and COSMIC offer to apps in the background, through `wl-clipboard-rs`.
//!
//! One thread watches the clipboard; another reads and writes it, each call
//! on a connection of its own. A written text is served by a thread of
//! `wl-clipboard-rs` until another app copies.

use std::io;
use std::os::fd::AsFd;
use std::thread;
use std::time::Instant;

use rustix::event::{PollFd, PollFlags, Timespec};
use tokio::sync::watch;
use wl_clipboard_rs::copy;
use wl_clipboard_rs::paste::{self, ClipboardType, Seat};
use wl_clipboard_rs::watch::{CancelHandle, ClipboardType as Watched, Watcher};

use super::{
    OPEN_TIMEOUT, PlatformError, TRANSFER_TIMEOUT, Worker, read_text, too_long, unavailable,
};
use crate::ClipboardText;

pub(super) struct WaylandClipboard {
    worker: Worker<()>,
    watcher: CancelHandle,
}

impl Drop for WaylandClipboard {
    fn drop(&mut self) {
        self.watcher.cancel();
    }
}

impl WaylandClipboard {
    /// The clipboard through data-control, or `None` when the session is not
    /// Wayland or the compositor does not offer it.
    pub(super) fn open(changes: watch::Sender<()>) -> Option<Self> {
        let (ready, started) = std::sync::mpsc::sync_channel(1);
        let watching = thread::Builder::new()
            .name("wayland-clipboard-watch".into())
            .spawn(move || watch_changes(&ready, &changes));
        if let Err(error) = watching {
            tracing::warn!(%error, "cannot start watching the Wayland clipboard");
            return None;
        }
        let watcher = match started.recv_timeout(OPEN_TIMEOUT) {
            Ok(Ok(watcher)) => watcher,
            Ok(Err(error)) => {
                tracing::debug!(%error, "no Wayland data-control");
                return None;
            }
            Err(_) => {
                tracing::warn!("the Wayland compositor did not answer in time");
                return None;
            }
        };
        match Worker::spawn("wayland-clipboard", ()) {
            Ok(worker) => Some(Self { worker, watcher }),
            Err(error) => {
                tracing::warn!(%error, "cannot start the Wayland clipboard thread");
                watcher.cancel();
                None
            }
        }
    }

    pub(super) async fn read(&self) -> Result<Option<ClipboardText>, PlatformError> {
        self.worker.run(|()| read()).await
    }

    pub(super) async fn write(&self, text: ClipboardText) -> Result<(), PlatformError> {
        self.worker.run(move |()| write(text)).await
    }
}

/// Watches the clipboard until cancelled, and reports each change on
/// `changes`. Says on `ready` whether it could start.
fn watch_changes(
    ready: &std::sync::mpsc::SyncSender<Result<CancelHandle, paste::Error>>,
    changes: &watch::Sender<()>,
) {
    let mut watcher = match Watcher::new(Watched::Regular, Seat::Unspecified) {
        Ok(watcher) => watcher,
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    // The compositor sends the clipboard as it is when the watch starts,
    // which `Watcher::new` already received: that is not a change.
    if let Err(error) = watcher.next_event() {
        let _ = ready.send(Err(error));
        return;
    }
    if ready.send(Ok(watcher.cancel_handle())).is_err() {
        return;
    }
    loop {
        match watcher.next_event() {
            Ok(Some(_)) => {
                changes.send_replace(());
            }
            Ok(None) => return,
            Err(error) => {
                tracing::warn!(%error, "stopped watching the Wayland clipboard");
                return;
            }
        }
    }
}

fn read() -> Result<Option<ClipboardText>, PlatformError> {
    let types = match paste::get_mime_types_ordered(ClipboardType::Regular, Seat::Unspecified) {
        Ok(types) => types,
        Err(paste::Error::ClipboardEmpty | paste::Error::NoSeats) => return Ok(None),
        Err(error) => return Err(unavailable(error)),
    };
    let types: Vec<&str> = types.iter().map(String::as_str).collect();
    read_text(&types, fetch)
}

/// The clipboard in type `mime_type`, up to `limit` bytes.
fn fetch(mime_type: &str, limit: usize) -> Result<Option<Vec<u8>>, PlatformError> {
    let pipe = match paste::get_contents(
        ClipboardType::Regular,
        Seat::Unspecified,
        paste::MimeType::Specific(mime_type),
    ) {
        Ok((pipe, _)) => pipe,
        // The clipboard changed since its types were read.
        Err(paste::Error::ClipboardEmpty | paste::Error::NoMimeType | paste::Error::NoSeats) => {
            return Ok(None);
        }
        Err(error) => return Err(unavailable(error)),
    };
    read_pipe(&pipe, limit).map(Some)
}

/// Reads what the app that copied writes into `pipe`, until it closes the
/// pipe. An app that stops writing fails the read after a while rather than
/// holding it.
fn read_pipe(pipe: &impl AsFd, limit: usize) -> Result<Vec<u8>, PlatformError> {
    let mut data = Vec::new();
    let mut buffer = vec![0; 64 << 10];
    let mut deadline = Instant::now() + TRANSFER_TIMEOUT;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(unavailable("the app that copied stopped sending"));
        }
        let timeout = Timespec::try_from(left).map_err(unavailable)?;
        let mut fds = [PollFd::new(pipe, PollFlags::IN)];
        match rustix::event::poll(&mut fds, Some(&timeout)) {
            Ok(0) | Err(rustix::io::Errno::INTR) => continue,
            Ok(_) => {}
            Err(error) => return Err(io::Error::from(error).into()),
        }
        match rustix::io::read(pipe, &mut buffer) {
            Ok(0) => return Ok(data),
            Ok(read) => {
                data.extend_from_slice(&buffer[..read]);
                if data.len() > limit {
                    return Err(too_long());
                }
                deadline = Instant::now() + TRANSFER_TIMEOUT;
            }
            Err(rustix::io::Errno::INTR | rustix::io::Errno::AGAIN) => {}
            Err(error) => return Err(io::Error::from(error).into()),
        }
    }
}

fn write(text: ClipboardText) -> Result<(), PlatformError> {
    let mut options = copy::Options::new();
    // Offered with the password hint set to `secret`.
    options.sensitive(text.sensitive);
    let bytes = text.text.len();
    options
        .copy(
            copy::Source::Bytes(text.text.into_bytes().into_boxed_slice()),
            copy::MimeType::Text,
        )
        .map_err(unavailable)?;
    tracing::debug!(bytes, sensitive = text.sensitive, "wrote the clipboard");
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::super::MAX_READ_BYTES;
    use super::*;

    #[test]
    fn reads_a_pipe_to_its_end() {
        let (reader, mut writer) = io::pipe().unwrap();
        let sending = thread::spawn(move || {
            for _ in 0..4 {
                writer.write_all(&[b'a'; 100 << 10]).unwrap();
            }
        });
        let data = read_pipe(&reader, MAX_READ_BYTES).unwrap();
        sending.join().unwrap();
        assert_eq!(data.len(), 400 << 10);
    }

    #[test]
    fn refuses_a_pipe_over_the_limit() {
        let (reader, mut writer) = io::pipe().unwrap();
        writer.write_all(b"secret!").unwrap();
        drop(writer);
        assert!(read_pipe(&reader, 6).is_err());
    }

    #[test]
    fn gives_up_on_a_silent_app() {
        let (reader, _writer) = io::pipe().unwrap();
        let started = Instant::now();
        assert!(read_pipe(&reader, MAX_READ_BYTES).is_err());
        assert!(started.elapsed() >= TRANSFER_TIMEOUT);
    }

    /// Needs a Wayland compositor with data-control (KDE Plasma, Sway,
    /// Hyprland, niri or COSMIC), and replaces the clipboard while it runs:
    /// `cargo test -p link2linux-platform -- --ignored wayland_clipboard`
    #[tokio::test]
    #[ignore = "needs a Wayland compositor with data-control"]
    async fn wayland_clipboard_round_trip() {
        let (changes, mut changed) = watch::channel(());
        let clipboard = WaylandClipboard::open(changes).expect("data-control");
        let saved = clipboard.read().await.unwrap();
        let outcome = async {
            let secret = ClipboardText {
                text: "ü".repeat(128 << 10),
                sensitive: true,
            };
            changed.mark_unchanged();
            clipboard.write(secret.clone()).await?;
            tokio::time::timeout(TRANSFER_TIMEOUT, changed.changed())
                .await
                .map_err(|_| unavailable("no change reported in time"))?
                .map_err(unavailable)?;
            let read = clipboard.read().await?;
            let plain = ClipboardText {
                text: "plain".into(),
                sensitive: false,
            };
            clipboard.write(plain.clone()).await?;
            Ok::<_, PlatformError>((read, secret, clipboard.read().await?, plain))
        }
        .await;
        if let Some(saved) = saved {
            clipboard.write(saved).await.unwrap();
            // The text is served by this process: give a clipboard manager
            // time to copy it before the test exits.
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
        let (read_secret, secret, read_plain, plain) = outcome.unwrap();
        assert_eq!(read_secret, Some(secret));
        assert_eq!(read_plain, Some(plain));
    }
}
