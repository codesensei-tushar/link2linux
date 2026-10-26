//! The clipboard through Wayland data-control (`ext-data-control-v1`, or
//! `zwlr-data-control-unstable-v1`), which KDE Plasma, Sway, Hyprland, niri
//! and COSMIC offer to apps in the background, through `wl-clipboard-rs`.
//!
//! One thread watches the clipboard; another reads and writes it, each call
//! on a connection of its own. A written text is served by a thread of
//! `wl-clipboard-rs` until another app copies.

use std::io;
use std::os::fd::AsFd;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::Instant;

use rustix::event::{PollFd, PollFlags, Timespec};
use wl_clipboard_rs::copy;
use wl_clipboard_rs::paste::{self, ClipboardType, Seat};
use wl_clipboard_rs::watch::{CancelHandle, ClipboardType as Watched, Watcher};

use super::{
    OPEN_TIMEOUT, PlatformError, READ_TIMEOUT, Reporter, Source, TRANSFER_TIMEOUT, Worker,
    too_long, unavailable,
};
use crate::ClipboardText;

pub(super) struct WaylandClipboard {
    /// Reads and writes, with the number of changes the watcher saw.
    worker: Worker<Arc<AtomicU64>>,
    watcher: CancelHandle,
}

impl Drop for WaylandClipboard {
    fn drop(&mut self) {
        self.watcher.cancel();
    }
}

impl WaylandClipboard {
    /// The clipboard through data-control. Fails when the session is not
    /// Wayland, the compositor does not offer data-control, or it does not
    /// answer in time; it is tried once.
    pub(super) fn open(changes: Reporter) -> Result<Self, PlatformError> {
        let (ready, started) = std::sync::mpsc::sync_channel(1);
        let generation = Arc::new(AtomicU64::new(0));
        let watched = generation.clone();
        thread::Builder::new()
            .name("wayland-clipboard-watch".into())
            .spawn(move || watch_changes(&ready, &changes, &watched))?;
        let watcher = started
            .recv_timeout(OPEN_TIMEOUT)
            .map_err(|_| unavailable("the Wayland compositor did not answer in time"))?
            .map_err(unavailable)?;
        match Worker::spawn("wayland-clipboard", generation) {
            Ok(worker) => Ok(Self { worker, watcher }),
            Err(error) => {
                watcher.cancel();
                Err(error.into())
            }
        }
    }

    pub(super) async fn read(&self) -> Result<Option<ClipboardText>, PlatformError> {
        self.worker
            .run(|generation: &mut Arc<AtomicU64>| {
                let mut reading = Reading::new(generation);
                super::read(&mut reading, Instant::now() + READ_TIMEOUT)
            })
            .await
    }

    pub(super) async fn write(&self, text: ClipboardText) -> Result<(), PlatformError> {
        self.worker.run(move |_| write(text)).await
    }
}

/// Watches the clipboard until cancelled, and reports each change on
/// `changes`, counted in `generation`. Says on `ready` whether it could
/// start.
fn watch_changes(
    ready: &std::sync::mpsc::SyncSender<Result<CancelHandle, paste::Error>>,
    changes: &Reporter,
    generation: &AtomicU64,
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
                generation.fetch_add(1, Ordering::AcqRel);
                changes.report_change();
            }
            Ok(None) => return,
            Err(error) => {
                tracing::warn!(%error, "stopped watching the Wayland clipboard");
                return;
            }
        }
    }
}

/// The clipboard as one read sees it. Each call makes a connection of its
/// own and reaches whatever copy is current by then, so the read checks at
/// its end that the copy it began with is still there.
struct Reading<'a> {
    /// Clipboard changes the watcher saw, and their number when the read
    /// began.
    generation: &'a AtomicU64,
    began_at: u64,
    /// The types offered when the read began.
    types: Vec<String>,
    deadline: Instant,
}

impl<'a> Reading<'a> {
    fn new(generation: &'a AtomicU64) -> Self {
        Self {
            generation,
            began_at: 0,
            types: Vec::new(),
            deadline: Instant::now(),
        }
    }
}

/// The types the clipboard is offered in now; empty when it holds nothing.
fn offered_types() -> Result<Vec<String>, PlatformError> {
    match paste::get_mime_types_ordered(ClipboardType::Regular, Seat::Unspecified) {
        Ok(types) => Ok(types),
        Err(paste::Error::ClipboardEmpty | paste::Error::NoSeats) => Ok(Vec::new()),
        Err(error) => Err(unavailable(error)),
    }
}

impl Source for Reading<'_> {
    fn begin(&mut self, deadline: Instant) -> Result<Option<Vec<String>>, PlatformError> {
        self.deadline = deadline;
        self.began_at = self.generation.load(Ordering::Acquire);
        self.types = offered_types()?;
        Ok((!self.types.is_empty()).then(|| self.types.clone()))
    }

    fn fetch(&mut self, mime_type: &str, limit: usize) -> Result<Option<Vec<u8>>, PlatformError> {
        let pipe = match paste::get_contents(
            ClipboardType::Regular,
            Seat::Unspecified,
            paste::MimeType::Specific(mime_type),
        ) {
            Ok((pipe, _)) => pipe,
            // The clipboard changed since its types were read.
            Err(
                paste::Error::ClipboardEmpty | paste::Error::NoMimeType | paste::Error::NoSeats,
            ) => return Ok(None),
            Err(error) => return Err(unavailable(error)),
        };
        read_pipe(&pipe, limit, self.deadline).map(Some)
    }

    /// The watcher may hear of a copy after this read's connections reached
    /// it, so the types are asked for again too: a copy that differs in
    /// sensitivity or in being files differs in its types.
    fn changed_since_begin(&mut self) -> Result<bool, PlatformError> {
        Ok(self.generation.load(Ordering::Acquire) != self.began_at
            || offered_types()? != self.types)
    }
}

/// Reads what the app that copied writes into `pipe`, until it closes the
/// pipe. An app that stops writing for a while, or is still writing at
/// `deadline`, fails the read rather than holding it.
fn read_pipe(pipe: &impl AsFd, limit: usize, deadline: Instant) -> Result<Vec<u8>, PlatformError> {
    let mut data = Vec::new();
    let mut buffer = vec![0; 64 << 10];
    let mut stalls_at = (Instant::now() + TRANSFER_TIMEOUT).min(deadline);
    loop {
        let left = stalls_at.saturating_duration_since(Instant::now());
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
                stalls_at = (Instant::now() + TRANSFER_TIMEOUT).min(deadline);
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

    use std::time::Duration;

    use super::super::MAX_READ_BYTES;
    use super::super::link;
    use super::super::tests::require_session_opt_in;
    use super::*;

    fn far() -> Instant {
        Instant::now() + Duration::from_secs(60)
    }

    #[test]
    fn reads_a_pipe_to_its_end() {
        let (reader, mut writer) = io::pipe().unwrap();
        let sending = thread::spawn(move || {
            for _ in 0..4 {
                writer.write_all(&[b'a'; 100 << 10]).unwrap();
            }
        });
        let data = read_pipe(&reader, MAX_READ_BYTES, far()).unwrap();
        sending.join().unwrap();
        assert_eq!(data.len(), 400 << 10);
    }

    #[test]
    fn refuses_a_pipe_over_the_limit() {
        let (reader, mut writer) = io::pipe().unwrap();
        writer.write_all(b"secret!").unwrap();
        drop(writer);
        assert!(read_pipe(&reader, 6, far()).is_err());
    }

    #[test]
    fn gives_up_on_a_silent_app() {
        let (reader, _writer) = io::pipe().unwrap();
        let started = Instant::now();
        assert!(read_pipe(&reader, MAX_READ_BYTES, far()).is_err());
        assert!(started.elapsed() >= TRANSFER_TIMEOUT);
    }

    #[test]
    fn gives_up_at_the_deadline_on_an_app_that_keeps_sending() {
        let (reader, mut writer) = io::pipe().unwrap();
        thread::spawn(move || {
            // A byte now and then, well within the time each may take.
            while writer.write_all(b"a").is_ok() {
                thread::sleep(Duration::from_millis(200));
            }
        });
        let started = Instant::now();
        let deadline = started + Duration::from_secs(1);
        assert!(read_pipe(&reader, MAX_READ_BYTES, deadline).is_err());
        let took = started.elapsed();
        assert!(took >= Duration::from_secs(1) && took < Duration::from_millis(1500));
    }

    /// Needs a Wayland compositor with data-control (KDE Plasma, Sway,
    /// Hyprland, niri or COSMIC), and replaces the clipboard of the one
    /// `WAYLAND_DISPLAY` names while it runs (the text on it before is put
    /// back), so it runs only when `L2L_LIVE_SESSION_CLIPBOARD=1` says to:
    /// `L2L_LIVE_SESSION_CLIPBOARD=1 cargo test -p link2linux-platform -- --ignored wayland_clipboard`
    #[tokio::test]
    #[ignore = "replaces the session's clipboard: needs L2L_LIVE_SESSION_CLIPBOARD=1"]
    async fn wayland_clipboard_round_trip() {
        require_session_opt_in();
        let (reporter, changed) = link();
        let clipboard = WaylandClipboard::open(reporter).expect("data-control");
        let saved = clipboard.read().await.unwrap();
        let outcome = async {
            let secret = ClipboardText {
                text: "ü".repeat(128 << 10),
                sensitive: true,
            };
            changed.changes.lock().await.mark_unchanged();
            clipboard.write(secret.clone()).await?;
            tokio::time::timeout(TRANSFER_TIMEOUT, changed.changed())
                .await
                .map_err(|_| unavailable("no change reported in time"))??;
            let read = clipboard.read().await?;
            let plain = ClipboardText {
                text: "plain".into(),
                sensitive: false,
            };
            clipboard.write(plain.clone()).await?;
            Ok::<_, PlatformError>((read, secret, clipboard.read().await?, plain))
        }
        .await;
        let restored = match saved {
            Some(saved) => clipboard.write(saved).await,
            None => Ok(()),
        };
        // The text is served by this process: give a clipboard manager time
        // to copy it before the test exits.
        tokio::time::sleep(Duration::from_secs(1)).await;
        let (read_secret, secret, read_plain, plain) = outcome.unwrap();
        assert_eq!(read_secret, Some(secret));
        assert_eq!(read_plain, Some(plain));
        restored.unwrap();
    }
}
