//! The desktop clipboard, through Wayland data-control where the compositor
//! offers it (`ext-data-control-v1`, or the wlroots protocol it replaced),
//! and through X11 selections otherwise.
//!
//! The X11 path also serves Wayland desktops without data-control, such as
//! GNOME, as long as Xwayland runs: the compositor copies between its own
//! clipboard and the X11 `CLIPBOARD` selection both ways, and an X11 client
//! may watch, read and set that selection without focus.
//!
//! Both libraries block, so each backend makes its calls on threads of its
//! own and answers over channels: no call blocks the async runtime. Only the
//! clipboard is used, never the primary selection (the text selected last,
//! pasted with the middle button). Clipboard text is never logged, only its
//! length.

mod wayland;
mod x11;

use std::fmt::Display;
use std::io;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use tokio::sync::{Mutex, oneshot, watch};

use crate::{Clipboard, ClipboardAccess, ClipboardText, PlatformError};

const SERVICE: &str = "clipboard";

/// Text types, best first. All are read as UTF-8; X11's `STRING`, which is
/// Latin-1, is left out, since the apps that offer it offer these too.
const TEXT_TYPES: [&str; 3] = ["text/plain;charset=utf-8", "UTF8_STRING", "text/plain"];
/// A list of URIs, which names the files when files are copied.
const URI_LIST: &str = "text/uri-list";
/// Offered by GNOME's file manager, and others, when files are copied.
const COPIED_FILES: &str = "x-special/gnome-copied-files";
/// Set to `secret` by password managers, so clipboard managers leave the
/// text out of their history.
const PASSWORD_HINT: &str = "x-kde-passwordManagerHint";

/// Clipboard content over this size is not read. It is far over what can be
/// shared, and is refused rather than truncated.
const MAX_READ_BYTES: usize = 16 << 20;
/// Reading the password hint stops here; its value is one short word.
const MAX_HINT_BYTES: usize = 64;

/// How long the app that copied may take to start sending the clipboard, or
/// to send each next part of it, before the read fails.
const TRANSFER_TIMEOUT: Duration = Duration::from_secs(2);
/// How long [`LinuxClipboard::open`] waits for each display server to
/// answer. Xwayland may be started on the first connection to it.
const OPEN_TIMEOUT: Duration = Duration::from_secs(2);

fn unavailable(reason: impl Display) -> PlatformError {
    PlatformError::Unavailable {
        service: SERVICE,
        reason: reason.to_string(),
    }
}

fn too_long() -> PlatformError {
    unavailable(format_args!(
        "the clipboard holds more than {} MiB",
        MAX_READ_BYTES >> 20
    ))
}

/// The desktop clipboard: Wayland data-control where the compositor offers
/// it, X11 selections otherwise.
pub struct LinuxClipboard {
    backend: Backend,
    /// Clipboard changes. Held across calls to [`Clipboard::changed`], so a
    /// change made while no call waits is reported to the next one.
    changes: Mutex<watch::Receiver<()>>,
}

enum Backend {
    Wayland(wayland::WaylandClipboard),
    X11(x11::X11Clipboard),
    /// Neither: only the app's own window may use the clipboard.
    FocusOnly,
}

impl std::fmt::Debug for LinuxClipboard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let backend = match self.backend {
            Backend::Wayland(_) => "Wayland data-control",
            Backend::X11(_) => "X11",
            Backend::FocusOnly => "none",
        };
        f.debug_struct("LinuxClipboard")
            .field("backend", &backend)
            .finish_non_exhaustive()
    }
}

impl LinuxClipboard {
    /// The clipboard of the desktop session the app runs in. Never fails,
    /// and waits only a moment for the display server: without a way to
    /// reach the clipboard in the background, its access is
    /// [`ClipboardAccess::FocusOnly`].
    #[must_use]
    pub fn open() -> Self {
        let (sender, changes) = watch::channel(());
        let backend = if let Some(wayland) = wayland::WaylandClipboard::open(sender.clone()) {
            tracing::debug!("clipboard through Wayland data-control");
            Backend::Wayland(wayland)
        } else if let Some(x11) = x11::X11Clipboard::open(sender) {
            tracing::debug!("clipboard through X11 selections");
            Backend::X11(x11)
        } else {
            tracing::debug!("no clipboard access in the background");
            Backend::FocusOnly
        };
        Self {
            backend,
            changes: Mutex::new(changes),
        }
    }
}

impl Clipboard for LinuxClipboard {
    fn access(&self) -> ClipboardAccess {
        match self.backend {
            Backend::Wayland(_) | Backend::X11(_) => ClipboardAccess::Background,
            Backend::FocusOnly => ClipboardAccess::FocusOnly,
        }
    }

    async fn read(&self) -> Result<Option<ClipboardText>, PlatformError> {
        match &self.backend {
            Backend::Wayland(wayland) => wayland.read().await,
            Backend::X11(x11) => x11.read().await,
            Backend::FocusOnly => Err(no_background_access()),
        }
    }

    async fn write(&self, text: &ClipboardText) -> Result<(), PlatformError> {
        match &self.backend {
            Backend::Wayland(wayland) => wayland.write(text.clone()).await,
            Backend::X11(x11) => x11.write(text.clone()).await,
            Backend::FocusOnly => Err(no_background_access()),
        }
    }

    async fn changed(&self) -> Result<(), PlatformError> {
        if matches!(self.backend, Backend::FocusOnly) {
            return std::future::pending().await;
        }
        let mut changes = self.changes.lock().await;
        changes
            .changed()
            .await
            .map_err(|_| unavailable("the connection to the clipboard closed"))
    }
}

fn no_background_access() -> PlatformError {
    unavailable("this desktop session lets only the focused window use the clipboard")
}

/// A thread that makes blocking clipboard calls one at a time, with a
/// context of type `C` that only it uses. It stops when dropped.
struct Worker<C> {
    jobs: mpsc::Sender<Job<C>>,
}

type Job<C> = Box<dyn FnOnce(&mut C) + Send>;

impl<C: Send + 'static> Worker<C> {
    fn spawn(name: &str, mut context: C) -> io::Result<Self> {
        let (jobs, queue) = mpsc::channel::<Job<C>>();
        thread::Builder::new().name(name.into()).spawn(move || {
            for job in queue {
                job(&mut context);
            }
        })?;
        Ok(Self { jobs })
    }

    /// Runs `job` on the thread and waits for its result. Cancel-safe: a
    /// result nobody waits for any more is dropped.
    async fn run<T: Send + 'static>(
        &self,
        job: impl FnOnce(&mut C) -> Result<T, PlatformError> + Send + 'static,
    ) -> Result<T, PlatformError> {
        let (reply, result) = oneshot::channel();
        self.jobs
            .send(Box::new(move |context| {
                let _ = reply.send(job(context));
            }))
            .map_err(|_| stopped())?;
        result.await.map_err(|_| stopped())?
    }
}

fn stopped() -> PlatformError {
    unavailable("the clipboard thread stopped")
}

/// What the types the clipboard is offered in say about its content.
#[derive(Debug, PartialEq, Eq)]
struct Offered<'a> {
    /// The text type to read, if any.
    text: Option<&'a str>,
    /// Files were copied, for certain.
    files: bool,
    /// A list of URIs is offered, which names files if files were copied.
    uri_list: bool,
    /// The password hint is offered; its value says whether the text is
    /// secret.
    password_hint: bool,
}

impl<'a> Offered<'a> {
    fn new(types: &[&'a str]) -> Self {
        let offers = |wanted: &str| types.iter().any(|t| t.eq_ignore_ascii_case(wanted));
        let text = TEXT_TYPES.iter().find_map(|wanted| {
            types
                .iter()
                .find(|t| t.eq_ignore_ascii_case(wanted))
                .copied()
        });
        Self {
            text,
            files: offers(COPIED_FILES),
            uri_list: offers(URI_LIST),
            password_hint: offers(PASSWORD_HINT),
        }
    }
}

/// Reads the clipboard text, given the types it is offered in and a way to
/// fetch it in one of them, up to a size. `fetch` answers `None` when the
/// app that copied refuses that type, or the clipboard changed meanwhile.
///
/// Copied files are not text: they come with a list of `file:` URIs, and
/// often with their paths as text too, which is left alone.
fn read_text(
    types: &[&str],
    mut fetch: impl FnMut(&str, usize) -> Result<Option<Vec<u8>>, PlatformError>,
) -> Result<Option<ClipboardText>, PlatformError> {
    let offered = Offered::new(types);
    let Some(text_type) = offered.text else {
        return Ok(None);
    };
    if offered.files {
        return Ok(None);
    }
    if offered.uri_list && fetch(URI_LIST, MAX_READ_BYTES)?.is_some_and(|list| names_files(&list)) {
        return Ok(None);
    }
    // A hint that is offered but cannot be read counts as secret: the text
    // may be a password, and is then shared only where passwords are.
    let sensitive = offered.password_hint
        && fetch(PASSWORD_HINT, MAX_HINT_BYTES)
            .ok()
            .flatten()
            .is_none_or(|hint| is_secret(&hint));
    let Some(bytes) = fetch(text_type, MAX_READ_BYTES)? else {
        return Ok(None);
    };
    tracing::debug!(bytes = bytes.len(), sensitive, "read the clipboard");
    Ok(decode(bytes).map(|text| ClipboardText { text, sensitive }))
}

/// Whether a `text/uri-list` names a local file: one `file:` URI makes the
/// copy a file copy. Lines starting with `#` are comments.
fn names_files(uri_list: &[u8]) -> bool {
    String::from_utf8_lossy(uri_list)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .any(|uri| {
            uri.get(..5)
                .is_some_and(|scheme| scheme.eq_ignore_ascii_case("file:"))
        })
}

/// Whether the password hint's value marks the text as secret.
fn is_secret(hint: &[u8]) -> bool {
    hint.strip_suffix(b"\0").unwrap_or(hint).trim_ascii() == b"secret"
}

/// The text, read as UTF-8; bytes that are not are replaced. `None` when
/// empty.
fn decode(bytes: Vec<u8>) -> Option<String> {
    if bytes.is_empty() {
        return None;
    }
    Some(
        String::from_utf8(bytes)
            .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned()),
    )
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    #[test]
    fn picks_the_best_text_type() {
        let offered = Offered::new(&["TARGETS", "text/plain", "UTF8_STRING", "STRING"]);
        assert_eq!(offered.text, Some("UTF8_STRING"));
        let offered = Offered::new(&["text/plain", "text/plain;charset=UTF-8", "UTF8_STRING"]);
        assert_eq!(offered.text, Some("text/plain;charset=UTF-8"));
        assert_eq!(Offered::new(&["text/plain"]).text, Some("text/plain"));
        assert_eq!(Offered::new(&["image/png", "STRING", "TEXT"]).text, None);
    }

    #[test]
    fn sees_copied_files_and_the_password_hint() {
        let nautilus = Offered::new(&[
            "x-special/gnome-copied-files",
            "text/uri-list",
            "UTF8_STRING",
        ]);
        assert!(nautilus.files && nautilus.uri_list && !nautilus.password_hint);
        let manager = Offered::new(&["text/plain;charset=utf-8", "x-kde-passwordManagerHint"]);
        assert!(!manager.files && !manager.uri_list && manager.password_hint);
    }

    #[test]
    fn tells_file_lists_from_links() {
        assert!(names_files(b"file:///home/ana/report.pdf\r\n"));
        assert!(names_files(
            b"# copied\r\nhttps://example.com/\r\nFILE:///tmp/a%20b\r\n"
        ));
        assert!(!names_files(b"https://example.com/page\r\n"));
        assert!(!names_files(b"# file:///not/a/uri\n\n"));
        assert!(!names_files(b""));
    }

    #[test]
    fn reads_the_secret_mark() {
        assert!(is_secret(b"secret"));
        assert!(is_secret(b"secret\n"));
        assert!(is_secret(b"secret\0"));
        assert!(!is_secret(b"public"));
        assert!(!is_secret(b""));
        assert!(!is_secret(b"secrets"));
    }

    #[test]
    fn decodes_text() {
        assert_eq!(decode(Vec::new()), None);
        assert_eq!(decode("héllo".into()), Some("héllo".into()));
        assert_eq!(decode(b"a\xffb".to_vec()), Some("a\u{fffd}b".into()));
    }

    /// Runs [`read_text`] over a clipboard holding `contents`, and returns
    /// what it read and which types it fetched.
    fn read_from(
        contents: &[(&str, &[u8])],
    ) -> (Result<Option<ClipboardText>, PlatformError>, Vec<String>) {
        let types: Vec<&str> = contents.iter().map(|(t, _)| *t).collect();
        let contents: HashMap<&str, &[u8]> = contents.iter().copied().collect();
        let mut fetched = Vec::new();
        let result = read_text(&types, |t, limit| {
            fetched.push(t.to_owned());
            let data = contents[t];
            if data.len() > limit {
                return Err(too_long());
            }
            Ok(Some(data.to_vec()))
        });
        (result, fetched)
    }

    #[test]
    fn reads_text_and_its_mark() {
        let (text, fetched) = read_from(&[
            ("UTF8_STRING", b"hunter2"),
            ("x-kde-passwordManagerHint", b"secret"),
        ]);
        let text = text.unwrap().unwrap();
        assert_eq!((text.text.as_str(), text.sensitive), ("hunter2", true));
        assert_eq!(fetched, ["x-kde-passwordManagerHint", "UTF8_STRING"]);

        let (text, _) = read_from(&[("text/plain", b"note")]);
        let text = text.unwrap().unwrap();
        assert_eq!((text.text.as_str(), text.sensitive), ("note", false));
    }

    #[test]
    fn an_unreadable_password_hint_counts_as_secret() {
        let types = ["UTF8_STRING", "x-kde-passwordManagerHint"];
        let text = read_text(&types, |t, _| match t {
            "UTF8_STRING" => Ok(Some(b"hunter2".to_vec())),
            _ => Err(unavailable("no answer")),
        });
        assert!(text.unwrap().unwrap().sensitive);
        let text = read_text(&types, |t, _| {
            Ok((t == "UTF8_STRING").then(|| b"x".to_vec()))
        });
        assert!(text.unwrap().unwrap().sensitive);

        let (text, _) = read_from(&[
            ("UTF8_STRING", b"note"),
            ("x-kde-passwordManagerHint", b"public"),
        ]);
        assert!(!text.unwrap().unwrap().sensitive);
    }

    #[test]
    fn copied_files_are_not_text() {
        let (text, fetched) = read_from(&[
            ("text/uri-list", b"file:///home/ana/a.txt\r\n"),
            ("UTF8_STRING", b"/home/ana/a.txt"),
        ]);
        assert_eq!(text.unwrap(), None);
        assert_eq!(fetched, ["text/uri-list"]);

        let (text, fetched) = read_from(&[
            ("x-special/gnome-copied-files", b"copy\nfile:///a"),
            ("UTF8_STRING", b"/a"),
        ]);
        assert_eq!(text.unwrap(), None);
        assert!(fetched.is_empty());

        // A copied link is text.
        let (text, _) = read_from(&[
            ("text/uri-list", b"https://example.com/"),
            ("text/plain;charset=utf-8", b"https://example.com/"),
        ]);
        assert_eq!(text.unwrap().unwrap().text, "https://example.com/");
    }

    #[test]
    fn no_text_reads_as_none() {
        let (text, fetched) = read_from(&[("image/png", b"\x89PNG")]);
        assert_eq!(text.unwrap(), None);
        assert!(fetched.is_empty());
        let (text, _) = read_from(&[("UTF8_STRING", b"")]);
        assert_eq!(text.unwrap(), None);
        assert_eq!(read_text(&[], |_, _| unreachable!()).unwrap(), None);
    }

    #[test]
    fn refuses_more_than_it_reads() {
        let big = vec![b'a'; MAX_READ_BYTES + 1];
        let (text, _) = read_from(&[("UTF8_STRING", &big)]);
        assert!(matches!(text, Err(PlatformError::Unavailable { .. })));
    }

    #[tokio::test]
    async fn without_a_backend_only_the_focused_window_has_access() {
        let (_, changes) = watch::channel(());
        let clipboard = LinuxClipboard {
            backend: Backend::FocusOnly,
            changes: Mutex::new(changes),
        };
        assert_eq!(clipboard.access(), ClipboardAccess::FocusOnly);
        assert!(matches!(
            clipboard.read().await,
            Err(PlatformError::Unavailable { .. })
        ));
        let text = ClipboardText {
            text: "x".into(),
            sensitive: false,
        };
        assert!(clipboard.write(&text).await.is_err());
        assert!(
            tokio::time::timeout(Duration::from_millis(50), clipboard.changed())
                .await
                .is_err()
        );
    }

    /// Needs a session that lets apps use the clipboard in the background:
    /// X11, Xwayland, or a Wayland compositor with data-control. Replaces
    /// the clipboard while it runs, and puts the text on it before back:
    /// `DISPLAY=:0 cargo test -p link2linux-platform -- --ignored background_clipboard`
    #[tokio::test]
    #[ignore = "needs a desktop session"]
    async fn background_clipboard() {
        let clipboard = LinuxClipboard::open();
        assert_eq!(
            clipboard.access(),
            ClipboardAccess::Background,
            "{clipboard:?}"
        );
        let saved = clipboard.read().await.unwrap();
        let written = ClipboardText {
            text: "link2linux clipboard test".into(),
            sensitive: true,
        };
        let outcome = async {
            clipboard.write(&written).await?;
            tokio::time::timeout(TRANSFER_TIMEOUT, clipboard.changed())
                .await
                .map_err(|_| unavailable("no change reported in time"))??;
            clipboard.read().await
        }
        .await;
        let restored = match saved {
            Some(saved) => clipboard.write(&saved).await,
            None => Ok(()),
        };
        // A clipboard manager, such as GNOME's, keeps the last text it
        // copied once the app that wrote it exits: give it time to.
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert_eq!(outcome.unwrap(), Some(written));
        restored.unwrap();
    }

    #[tokio::test]
    async fn the_worker_runs_jobs_in_order() {
        let worker = Worker::spawn("test-worker", Vec::new()).unwrap();
        worker
            .run(|seen: &mut Vec<u32>| {
                seen.push(1);
                Ok(())
            })
            .await
            .unwrap();
        let seen = worker
            .run(|seen: &mut Vec<u32>| {
                seen.push(2);
                Ok(seen.clone())
            })
            .await
            .unwrap();
        assert_eq!(seen, [1, 2]);
    }
}
