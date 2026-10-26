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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
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
/// How long a read or write may take in all, so a display server that
/// stops answering cannot hang the caller.
const CALL_TIMEOUT: Duration = Duration::from_secs(10);

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
///
/// Dropping it never blocks: its threads stop on their own once a call in
/// progress ends, and none of them keeps the process from exiting.
pub struct LinuxClipboard {
    backend: Backend,
    link: Link,
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
            .field("alive", &self.link.is_alive())
            .finish_non_exhaustive()
    }
}

impl LinuxClipboard {
    /// The clipboard of the desktop session the app runs in. Never fails:
    /// without a way to reach the clipboard in the background, its access
    /// is [`ClipboardAccess::FocusOnly`].
    ///
    /// Blocks the calling thread while it tries Wayland data-control, then
    /// X11: a display server that is missing or refuses the connection
    /// fails at once, but one that accepts it and does not answer is given
    /// up on after 2 seconds each, so up to 4 seconds in all. Call it at
    /// startup or from a blocking task, such as tokio's `spawn_blocking`,
    /// not on an async runtime's thread.
    #[must_use]
    pub fn open() -> Self {
        Self::open_with(true, None)
    }

    /// Tries Wayland data-control if `wayland`, then the X server named by
    /// `x11_display` (`DISPLAY` when `None`), each once, and says which it
    /// got in one line of log.
    fn open_with(wayland: bool, x11_display: Option<&str>) -> Self {
        let no_wayland = if wayland {
            let (reporter, link) = link();
            match wayland::WaylandClipboard::open(reporter) {
                Ok(clipboard) => {
                    tracing::info!("clipboard through Wayland data-control");
                    return Self {
                        backend: Backend::Wayland(clipboard),
                        link,
                    };
                }
                Err(error) => error.to_string(),
            }
        } else {
            "not tried".to_owned()
        };
        let (reporter, link) = link();
        match x11::X11Clipboard::open(reporter, x11_display) {
            Ok(clipboard) => {
                tracing::info!(%no_wayland, "clipboard through X11 selections");
                Self {
                    backend: Backend::X11(clipboard),
                    link,
                }
            }
            Err(no_x11) => {
                tracing::info!(
                    %no_wayland,
                    %no_x11,
                    "clipboard only while the app's window has focus"
                );
                Self {
                    backend: Backend::FocusOnly,
                    link: Link::none(),
                }
            }
        }
    }

    /// Why the clipboard cannot be used in the background.
    fn no_access(&self) -> PlatformError {
        match self.backend {
            Backend::FocusOnly => {
                unavailable("this desktop session lets only the focused window use the clipboard")
            }
            Backend::Wayland(_) | Backend::X11(_) => lost(),
        }
    }
}

impl Clipboard for LinuxClipboard {
    /// [`ClipboardAccess::Background`] while the backend reaches its display
    /// server; once it lost it, [`ClipboardAccess::FocusOnly`].
    fn access(&self) -> ClipboardAccess {
        if self.link.is_alive() {
            ClipboardAccess::Background
        } else {
            ClipboardAccess::FocusOnly
        }
    }

    async fn read(&self) -> Result<Option<ClipboardText>, PlatformError> {
        if !self.link.is_alive() {
            return Err(self.no_access());
        }
        match &self.backend {
            Backend::Wayland(wayland) => wayland.read().await,
            Backend::X11(x11) => x11.read().await,
            Backend::FocusOnly => Err(self.no_access()),
        }
    }

    async fn write(&self, text: &ClipboardText) -> Result<(), PlatformError> {
        if !self.link.is_alive() {
            return Err(self.no_access());
        }
        match &self.backend {
            Backend::Wayland(wayland) => wayland.write(text.clone()).await,
            Backend::X11(x11) => x11.write(text.clone()).await,
            Backend::FocusOnly => Err(self.no_access()),
        }
    }

    /// Also fails, once, when the backend loses its display server; from
    /// then on [`access`](Self::access) is [`ClipboardAccess::FocusOnly`]
    /// and this never resolves.
    async fn changed(&self) -> Result<(), PlatformError> {
        self.link.changed().await
    }
}

fn lost() -> PlatformError {
    unavailable("lost the connection to the display server")
}

/// Creates the two ends that tie a backend's watching thread to the
/// clipboard.
fn link() -> (Reporter, Link) {
    let (changes, receiver) = watch::channel(());
    let alive = Arc::new(AtomicBool::new(true));
    let reporter = Reporter {
        changes,
        alive: alive.clone(),
    };
    let link = Link {
        alive,
        loss_reported: AtomicBool::new(false),
        changes: Mutex::new(receiver),
    };
    (reporter, link)
}

/// Held by the thread that watches the clipboard for a backend. Dropped when
/// that thread ends, as it does when its connection to the display server
/// fails, and then marks the backend dead.
struct Reporter {
    changes: watch::Sender<()>,
    alive: Arc<AtomicBool>,
}

impl Reporter {
    fn report_change(&self) {
        self.changes.send_replace(());
    }
}

impl Drop for Reporter {
    // Runs before the fields drop, so the backend counts as dead by the time
    // the closed channel wakes `Link::changed`.
    fn drop(&mut self) {
        self.alive.store(false, Ordering::Release);
    }
}

/// The clipboard's end of a [`Reporter`].
struct Link {
    alive: Arc<AtomicBool>,
    /// `changed` already failed for the loss of the display server.
    loss_reported: AtomicBool,
    /// Held across calls to `changed`, so a change made while no call waits
    /// is reported to the next one.
    changes: Mutex<watch::Receiver<()>>,
}

impl Link {
    /// The end of a link to no backend at all.
    fn none() -> Self {
        let (_, link) = link();
        link.loss_reported.store(true, Ordering::Release);
        link
    }

    fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Acquire)
    }

    /// The next change; an error, once, when the backend is lost; after
    /// that, or without a backend, never.
    async fn changed(&self) -> Result<(), PlatformError> {
        if !self.loss_reported.load(Ordering::Acquire) {
            let mut changes = self.changes.lock().await;
            if changes.changed().await.is_ok() {
                return Ok(());
            }
            if !self.loss_reported.swap(true, Ordering::AcqRel) {
                return Err(lost());
            }
        }
        std::future::pending().await
    }
}

/// A thread that makes blocking clipboard calls one at a time, with a
/// context of type `C` that only it uses. It stops when dropped, once the
/// call in progress ends; it is never joined.
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

    /// Runs `job` on the thread and waits for its result, up to
    /// [`CALL_TIMEOUT`]. Cancel-safe: a result nobody waits for any more is
    /// dropped.
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
        tokio::time::timeout(CALL_TIMEOUT, result)
            .await
            .map_err(|_| unavailable("the display server did not answer in time"))?
            .map_err(|_| stopped())?
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

    /// No server is listening on this X display.
    pub(super) const NO_X_SERVER: &str = ":4242";

    #[tokio::test]
    async fn without_a_backend_only_the_focused_window_has_access() {
        let started = std::time::Instant::now();
        let clipboard = LinuxClipboard::open_with(false, Some(NO_X_SERVER));
        // Refused at once, not after the timeout.
        assert!(started.elapsed() < OPEN_TIMEOUT);
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

    /// Live tests replace the clipboard of the display server they reach,
    /// so they run on a private one, as `env -u WAYLAND_DISPLAY xvfb-run -a`
    /// starts, and refuse the desktop session's unless
    /// `L2L_LIVE_SESSION_CLIPBOARD=1` is set. The session's are X display 0,
    /// any X display while `WAYLAND_DISPLAY` is set (that is Xwayland), and
    /// Wayland socket `wayland-0`. Says whether the test runs on the session.
    pub(super) fn guard_session_clipboard(x11: bool, wayland: bool) -> bool {
        let env = |name| std::env::var(name).ok();
        let display = env("DISPLAY");
        let wayland_display = env("WAYLAND_DISPLAY");
        let on_x11_session = x11
            && display.as_deref().is_some_and(|display| {
                wayland_display.is_some() || x_display_number(display) == Some(0)
            });
        let on_wayland_session = wayland && wayland_display.as_deref() == Some("wayland-0");
        let on_session = on_x11_session || on_wayland_session;
        assert!(
            !on_session || env("L2L_LIVE_SESSION_CLIPBOARD").as_deref() == Some("1"),
            "refusing to replace the desktop session's clipboard: run this test on a private \
             display server (for X11, under `env -u WAYLAND_DISPLAY xvfb-run -a`), or set \
             L2L_LIVE_SESSION_CLIPBOARD=1"
        );
        on_session
    }

    /// The display number in an X display name such as `:0`, `:1.0` or
    /// `localhost:10.0`.
    fn x_display_number(display: &str) -> Option<u32> {
        let (_, number) = display.rsplit_once(':')?;
        number.split('.').next()?.parse().ok()
    }

    #[test]
    fn reads_x_display_numbers() {
        assert_eq!(x_display_number(":0"), Some(0));
        assert_eq!(x_display_number(":0.0"), Some(0));
        assert_eq!(x_display_number("unix:0"), Some(0));
        assert_eq!(x_display_number(":99"), Some(99));
        assert_eq!(x_display_number("localhost:10.0"), Some(10));
        assert_eq!(x_display_number(""), None);
        assert_eq!(x_display_number("wayland-0"), None);
    }

    /// Needs a session that lets apps use the clipboard in the background,
    /// and replaces its clipboard while it runs (the text on it before is
    /// put back). Refuses the desktop session's (see
    /// [`guard_session_clipboard`]); run it on a private X server:
    /// `env -u WAYLAND_DISPLAY xvfb-run -a cargo test -p link2linux-platform -- --ignored --test-threads=1 background_clipboard`
    #[tokio::test]
    #[ignore = "replaces the clipboard: run under `env -u WAYLAND_DISPLAY xvfb-run -a`"]
    async fn background_clipboard() {
        let on_session = guard_session_clipboard(true, true);
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
        if on_session {
            // A clipboard manager, such as GNOME's, keeps the last text it
            // copied once the app that wrote it exits: give it time to.
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        assert_eq!(outcome.unwrap(), Some(written));
        restored.unwrap();
    }

    #[tokio::test]
    async fn reports_changes_then_the_loss_once() {
        let (reporter, link) = link();
        assert!(link.is_alive());
        // Made while nobody waited, and reported once though made twice.
        reporter.report_change();
        reporter.report_change();
        link.changed().await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(50), link.changed())
                .await
                .is_err()
        );

        // The backend's thread ends, as when its connection fails, while a
        // call waits.
        let (waited, ()) = tokio::join!(link.changed(), async move {
            tokio::task::yield_now().await;
            drop(reporter);
        });
        assert!(matches!(waited, Err(PlatformError::Unavailable { .. })));
        assert!(!link.is_alive());
        // Once only: then it waits, as without background access.
        assert!(
            tokio::time::timeout(Duration::from_millis(50), link.changed())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn a_loss_before_the_call_is_reported_too() {
        let (reporter, link) = link();
        drop(reporter);
        assert!(!link.is_alive());
        assert!(link.changed().await.is_err());
        let none = Link::none();
        assert!(!none.is_alive());
        assert!(
            tokio::time::timeout(Duration::from_millis(50), none.changed())
                .await
                .is_err()
        );
    }

    /// A private X server, stopped when dropped.
    struct PrivateX {
        server: std::process::Child,
        display: String,
    }

    impl PrivateX {
        fn start() -> Self {
            use std::io::BufRead;
            // Xvfb picks a free display and writes its number to stdout.
            let mut server = std::process::Command::new("Xvfb")
                .args(["-displayfd", "1", "-nolisten", "tcp"])
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .spawn()
                .expect("Xvfb");
            let mut number = String::new();
            io::BufReader::new(server.stdout.take().expect("piped"))
                .read_line(&mut number)
                .expect("a display number");
            let display = format!(":{}", number.trim());
            Self { server, display }
        }

        fn stop(&mut self) {
            let _ = self.server.kill();
            let _ = self.server.wait();
        }
    }

    impl Drop for PrivateX {
        fn drop(&mut self) {
            self.stop();
        }
    }

    /// Starts its own Xvfb, so it never reaches the desktop session:
    /// `cargo test -p link2linux-platform -- --ignored own_xvfb`
    #[tokio::test]
    #[ignore = "starts its own Xvfb"]
    async fn own_xvfb_losing_the_server_ends_background_access() {
        let mut server = PrivateX::start();
        let clipboard = LinuxClipboard::open_with(false, Some(&server.display));
        assert_eq!(clipboard.access(), ClipboardAccess::Background);
        server.stop();
        let lost = tokio::time::timeout(TRANSFER_TIMEOUT, clipboard.changed())
            .await
            .expect("the loss reported in time");
        assert!(matches!(lost, Err(PlatformError::Unavailable { .. })));
        assert_eq!(clipboard.access(), ClipboardAccess::FocusOnly);
        assert!(clipboard.read().await.is_err());
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

    /// Starts its own Xvfb, so it never reaches the desktop session:
    /// `cargo test -p link2linux-platform -- --ignored own_xvfb`
    #[tokio::test]
    #[ignore = "starts its own Xvfb"]
    async fn own_xvfb_dropping_the_clipboard_lets_go_of_the_server() {
        let server = PrivateX::start();
        let holder = LinuxClipboard::open_with(false, Some(&server.display));
        let other = LinuxClipboard::open_with(false, Some(&server.display));
        let text = ClipboardText {
            text: "held".into(),
            sensitive: false,
        };
        holder.write(&text).await.unwrap();
        tokio::time::timeout(TRANSFER_TIMEOUT, other.changed())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(other.read().await.unwrap(), Some(text));

        let started = std::time::Instant::now();
        drop(holder);
        assert!(started.elapsed() < Duration::from_millis(100));
        // Its threads end and close their connections, and the text, which
        // no clipboard manager copied, goes with them.
        tokio::time::timeout(TRANSFER_TIMEOUT, other.changed())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(other.read().await.unwrap(), None);
    }

    /// An app that takes the clipboard of the X server on `display` and
    /// never answers anyone who pastes, while the connection lives.
    fn stall_clipboard(display: &str) -> x11rb::rust_connection::RustConnection {
        use x11rb::connection::Connection;
        use x11rb::protocol::xproto::{ConnectionExt as _, CreateWindowAux, WindowClass};

        let (conn, screen) = x11rb::connect(Some(display)).expect("the X server");
        let window = conn.generate_id().unwrap();
        let root = conn.setup().roots[screen].root;
        conn.create_window(
            0,
            window,
            root,
            0,
            0,
            1,
            1,
            0,
            WindowClass::INPUT_ONLY,
            0,
            &CreateWindowAux::new(),
        )
        .unwrap();
        let clipboard = conn
            .intern_atom(false, b"CLIPBOARD")
            .unwrap()
            .reply()
            .unwrap()
            .atom;
        conn.set_selection_owner(window, clipboard, x11rb::CURRENT_TIME)
            .unwrap();
        let owner = conn
            .get_selection_owner(clipboard)
            .unwrap()
            .reply()
            .unwrap();
        assert_eq!(owner.owner, window);
        conn
    }

    /// Starts its own Xvfb, so it never reaches the desktop session:
    /// `cargo test -p link2linux-platform -- --ignored own_xvfb`
    #[tokio::test]
    #[ignore = "starts its own Xvfb"]
    async fn own_xvfb_dropping_mid_read_does_not_wait() {
        let server = PrivateX::start();
        let _staller = stall_clipboard(&server.display);
        let clipboard = LinuxClipboard::open_with(false, Some(&server.display));
        // A read gives up on the app that never answers.
        let started = std::time::Instant::now();
        assert!(clipboard.read().await.is_err());
        assert!(started.elapsed() < TRANSFER_TIMEOUT + Duration::from_millis(500));
        // Dropped while its reader waits on that app.
        assert!(
            tokio::time::timeout(Duration::from_millis(300), clipboard.read())
                .await
                .is_err()
        );
        let started = std::time::Instant::now();
        drop(clipboard);
        assert!(started.elapsed() < Duration::from_millis(100));
    }

    /// The X displays [`exits_with_busy_clipboard_threads`] uses.
    const EXIT_SERVING: &str = "L2L_TEST_EXIT_SERVING_DISPLAY";
    const EXIT_STALLED: &str = "L2L_TEST_EXIT_STALLED_DISPLAY";

    /// Starts its own Xvfb servers, so it never reaches the desktop
    /// session: `cargo test -p link2linux-platform -- --ignored own_xvfb`
    #[test]
    #[ignore = "starts its own Xvfb"]
    fn own_xvfb_busy_threads_do_not_hold_the_process_open() {
        let serving = PrivateX::start();
        let stalled = PrivateX::start();
        let started = std::time::Instant::now();
        let exited = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "linux::clipboard::tests::exits_with_busy_clipboard_threads",
            ])
            .env(EXIT_SERVING, &serving.display)
            .env(EXIT_STALLED, &stalled.display)
            .output()
            .unwrap();
        let report = String::from_utf8_lossy(&exited.stdout);
        assert!(exited.status.success(), "{report}");
        assert!(report.contains("1 passed"), "{report}");
        // The reader it left waits up to TRANSFER_TIMEOUT more on the app
        // that never answers; the process did not wait for it.
        assert!(
            started.elapsed() < Duration::from_millis(1500),
            "{:?}",
            started.elapsed()
        );
    }

    /// Run by [`own_xvfb_busy_threads_do_not_hold_the_process_open`], in a
    /// process of its own, and does nothing otherwise: leaves one
    /// clipboard's owner thread serving its selection and another's reader
    /// waiting on an app that never answers, as at an app's exit, and ends.
    #[tokio::test]
    #[ignore = "run by own_xvfb_busy_threads_do_not_hold_the_process_open"]
    async fn exits_with_busy_clipboard_threads() {
        let (Ok(serving), Ok(stalled)) = (std::env::var(EXIT_SERVING), std::env::var(EXIT_STALLED))
        else {
            return;
        };
        let holder = LinuxClipboard::open_with(false, Some(&serving));
        let text = ClipboardText {
            text: "held".into(),
            sensitive: false,
        };
        holder.write(&text).await.unwrap();
        let staller = stall_clipboard(&stalled);
        let reader = LinuxClipboard::open_with(false, Some(&stalled));
        assert!(
            tokio::time::timeout(Duration::from_millis(300), reader.read())
                .await
                .is_err()
        );
        // Not dropped, as when the app exits.
        std::mem::forget((holder, reader, staller));
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
