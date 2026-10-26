//! What the clipboard services of all paired phones share.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use link2linux_platform::{Clipboard, ClipboardText};
use tokio::sync::{Notify, broadcast};
use tokio::time::Instant;

use super::{
    Clip, ClipboardEvent, ClipboardSettings, Digest, NotShared, PhoneClipboard, RECENT_COPY, check,
};

/// Text a phone pushed that waits for the window to gain focus, on a
/// desktop that lets the app use its clipboard only then. It goes on the
/// clipboard when the window gains focus or the user clicks its pop-up.
#[derive(Clone)]
pub struct Held {
    /// The text.
    pub clip: Clip,
    /// The phone that pushed it.
    pub device_id: String,
}

impl fmt::Debug for Held {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Held")
            .field("clip", &self.clip)
            .field("device_id", &self.device_id)
            .finish()
    }
}

/// What the desktop clipboard held when the desktop last looked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Content {
    Text(Digest),
    /// No text: copied files, an image, or nothing.
    NoText,
}

/// Counts the starts and ends of writes of a phone's text to the
/// clipboard. A read that a write overlapped may return what the text
/// replaced, so [`ClipboardHub::copied`] drops it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Generation(u64);

/// What a phone's text replaced in the hub, to put back if the text could
/// not be written.
pub(super) struct Marked {
    /// Which write it was.
    mark: u64,
    seen: Option<Content>,
    from_phone: Option<Digest>,
    latest: Option<(Arc<Clip>, Instant)>,
}

/// How many times [`ClipboardHub::read_copy`] reads the clipboard again
/// after a phone's text overtook its read.
const MAX_READS: usize = 4;

#[derive(Default)]
struct State {
    settings: ClipboardSettings,
    /// The app's window has focus, so a clipboard that works only then
    /// works now.
    focused: bool,
    seen: Option<Content>,
    /// The text a phone put on the clipboard, until the clipboard holds
    /// something else. It is not shared on by itself: not back to that
    /// phone, and not to the others.
    from_phone: Option<Digest>,
    generation: u64,
    /// Writes of a phone's text started so far, and those still running.
    marks: u64,
    writing: u32,
    /// The latest copy made on this desktop, while the clipboard holds it
    /// and it may be shared, and when it was made.
    latest: Option<(Arc<Clip>, Instant)>,
    held: Option<Held>,
    /// Each phone's state, as its service last reported it.
    phones: HashMap<String, PhoneClipboard>,
}

/// The desktop-wide part of the clipboard feature, shared by the service
/// of every paired phone. Each copy of the handle shares the same state.
#[derive(Clone)]
pub struct ClipboardHub {
    state: Arc<Mutex<State>>,
    events: broadcast::Sender<ClipboardEvent>,
    /// Told when no write of a phone's text runs any more.
    settled: Arc<Notify>,
}

impl ClipboardHub {
    /// A hub with the user's `settings`.
    #[must_use]
    pub fn new(settings: ClipboardSettings) -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                settings,
                ..State::default()
            })),
            events: broadcast::channel(64).0,
            settled: Arc::new(Notify::new()),
        }
    }

    /// The user's settings.
    #[must_use]
    pub fn settings(&self) -> ClipboardSettings {
        self.lock().settings
    }

    /// Changes the user's settings. Turning sharing off drops what waits:
    /// the latest copy and the text held for the window. A copy the old
    /// settings kept here stays here: the user can still send it. Each
    /// phone's service tells its phone with
    /// [`ClipboardService::sharing_changed`](super::ClipboardService::sharing_changed).
    pub fn set_settings(&self, settings: ClipboardSettings) {
        let mut state = self.lock();
        state.settings = settings;
        if !settings.sharing {
            state.latest = None;
            state.held = None;
        }
    }

    /// Everything that happens from now on.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<ClipboardEvent> {
        self.events.subscribe()
    }

    pub(super) fn emit(&self, event: ClipboardEvent) {
        // No subscriber is fine: the page may not be open.
        let _ = self.events.send(event);
    }

    /// Says whether the app's window has focus.
    pub fn set_focused(&self, focused: bool) {
        self.lock().focused = focused;
    }

    /// Whether the app's window has focus.
    #[must_use]
    pub fn is_focused(&self) -> bool {
        self.lock().focused
    }

    /// The text that waits for the window, if any.
    #[must_use]
    pub fn held(&self) -> Option<Held> {
        self.lock().held.clone()
    }

    /// Each phone's state, as its service last reported it.
    #[must_use]
    pub fn phones(&self) -> HashMap<String, PhoneClipboard> {
        self.lock().phones.clone()
    }

    /// Phone `device_id` is no longer paired: its state goes, and so does
    /// text it pushed that waits for the window.
    pub fn forget_phone(&self, device_id: &str) {
        let mut state = self.lock();
        state.phones.remove(device_id);
        if state
            .held
            .as_ref()
            .is_some_and(|held| held.device_id == device_id)
        {
            state.held = None;
        }
    }

    /// The count to pass to [`Self::copied`] for a read of the clipboard
    /// that starts now.
    #[must_use]
    pub fn generation(&self) -> Generation {
        Generation(self.lock().generation)
    }

    /// Reads the clipboard after a change, and returns the copy to share
    /// with every phone, as [`Self::copied`] does. A read that a write of a
    /// phone's text overlapped is made again once the write is over, so
    /// neither the text the write replaced nor a copy the user made just
    /// before it is lost or mistaken.
    pub async fn read_copy<C: Clipboard>(&self, clipboard: &C, now: Instant) -> Option<Arc<Clip>> {
        for _ in 0..MAX_READS {
            self.writes_done().await;
            let since = self.generation();
            let text = match clipboard.read().await {
                Ok(text) => text,
                Err(error) => {
                    tracing::debug!(%error, "could not read the clipboard");
                    return None;
                }
            };
            if let Some(copy) = self.take(text, now, since) {
                return copy;
            }
        }
        None
    }

    /// The clipboard holds `text` now, or no text when `None`, as read in a
    /// read that started at `since` ([`Self::generation`]). Returns the
    /// copy to share with every phone, or `None` when nothing new is to be
    /// shared: the same content as before, the text a phone put there, a
    /// read that a write of a phone's text overlapped, or a copy the limits
    /// or the settings keep here. Too long text and secrets are reported as
    /// [`ClipboardEvent::NotShared`].
    pub fn copied(
        &self,
        text: Option<ClipboardText>,
        now: Instant,
        since: Generation,
    ) -> Option<Arc<Clip>> {
        self.take(text, now, since).flatten()
    }

    /// As [`Self::copied`], with `None` for a read that a write of a
    /// phone's text overlapped.
    fn take(
        &self,
        text: Option<ClipboardText>,
        now: Instant,
        since: Generation,
    ) -> Option<Option<Arc<Clip>>> {
        let mut state = self.lock();
        if state.generation != since.0 || state.writing > 0 {
            return None;
        }
        let Some(text) = text else {
            state.seen = Some(Content::NoText);
            state.from_phone = None;
            state.latest = None;
            return Some(None);
        };
        let clip = Clip::new(text);
        let content = Content::Text(clip.digest());
        if state.seen.replace(content) == Some(content) {
            return Some(None);
        }
        if state.from_phone == Some(clip.digest()) {
            return Some(None);
        }
        state.from_phone = None;
        match check(&clip, state.settings) {
            Ok(()) => {
                let clip = Arc::new(clip);
                state.latest = Some((clip.clone(), now));
                Some(Some(clip))
            }
            Err(reason) => {
                // The clipboard moved on to a copy that stays here.
                state.latest = None;
                drop(state);
                if matches!(reason, NotShared::TooLarge { .. } | NotShared::Secret) {
                    tracing::info!(bytes = clip.len(), ?reason, "a copy is not shared");
                    self.emit(ClipboardEvent::NotShared(reason));
                }
                Some(None)
            }
        }
    }

    /// Waits until no write of a phone's text runs.
    async fn writes_done(&self) {
        loop {
            let settled = self.settled.notified();
            if self.lock().writing == 0 {
                return;
            }
            settled.await;
        }
    }

    /// The user asked to send `text` to every phone. Returns the copy to
    /// send, or why it cannot go. A phone whose session starts within
    /// [`RECENT_COPY`] from now gets it too.
    ///
    /// # Errors
    ///
    /// Fails when the clipboard holds no text, when the limits or the
    /// settings keep it here, and while sharing is off.
    pub fn send(&self, text: Option<ClipboardText>, now: Instant) -> Result<Arc<Clip>, NotShared> {
        let clip = Clip::new(text.ok_or(NotShared::NoText)?);
        let mut state = self.lock();
        check(&clip, state.settings)?;
        state.seen = Some(Content::Text(clip.digest()));
        let clip = Arc::new(clip);
        state.latest = Some((clip.clone(), now));
        Ok(clip)
    }

    /// Reads the clipboard and sends it, as [`Self::send`] does.
    ///
    /// # Errors
    ///
    /// As for [`Self::send`]. A clipboard that cannot be read counts as
    /// holding no text.
    pub async fn send_clipboard<C: Clipboard>(
        &self,
        clipboard: &C,
        now: Instant,
    ) -> Result<Arc<Clip>, NotShared> {
        let text = clipboard.read().await.unwrap_or_else(|error| {
            tracing::info!(%error, "could not read the clipboard");
            None
        });
        let sent = self.send(text, now);
        if let Err(reason) = sent {
            self.emit(ClipboardEvent::NotShared(reason));
        }
        sent
    }

    /// The window gained focus, on a desktop that lets the app use its
    /// clipboard only then. Text that waits goes on the clipboard, and what
    /// the clipboard held is not shared: the desktop could not see when it
    /// was copied, and it is most likely older than the phone's text.
    /// Otherwise the clipboard is read, and a copy the user made while the
    /// window was away is returned as for [`Self::copied`]. Call it after
    /// [`Self::set_focused`] said so; it does nothing once the window has
    /// lost focus again.
    pub async fn focused<C: Clipboard>(&self, clipboard: &C, now: Instant) -> Option<Arc<Clip>> {
        if !self.is_focused() {
            return None;
        }
        if let Some((held, marked)) = self.release() {
            match clipboard.write(held.clip.text()).await {
                Ok(()) => {
                    self.written(marked, true);
                    self.emit(ClipboardEvent::Released {
                        device_id: held.device_id,
                    });
                }
                Err(error) => {
                    tracing::warn!(%error, "could not put the phone's text on the clipboard");
                    self.put_back(held, marked);
                }
            }
            return None;
        }
        self.read_copy(clipboard, now).await
    }

    /// The latest copy, if the user made it within [`RECENT_COPY`] before
    /// `now`.
    pub(super) fn recent(&self, now: Instant) -> Option<Arc<Clip>> {
        let state = self.lock();
        let (clip, at) = state.latest.as_ref()?;
        (now.saturating_duration_since(*at) < RECENT_COPY).then(|| clip.clone())
    }

    /// A phone's text is about to go on the clipboard: when the clipboard
    /// reports it, it is not shared on, and reads that overlap the write are
    /// dropped. Pass the result to [`Self::written`] once the write is over.
    pub(super) fn mark(&self, digest: Digest) -> Marked {
        let mut state = self.lock();
        state.generation += 1;
        state.marks += 1;
        state.writing += 1;
        Marked {
            mark: state.marks,
            seen: state.seen.replace(Content::Text(digest)),
            from_phone: state.from_phone.replace(digest),
            latest: state.latest.take(),
        }
    }

    /// The write of a phone's text is over. When it failed, the clipboard
    /// still holds what it held, and that is put back, unless a newer phone
    /// text was marked meanwhile. Returns whether it was put back.
    pub(super) fn written(&self, marked: Marked, ok: bool) -> bool {
        let mut state = self.lock();
        state.writing = state.writing.saturating_sub(1);
        // A read that overlapped the write's end is old news too.
        state.generation += 1;
        let restored = !ok && state.marks == marked.mark;
        if restored {
            state.seen = marked.seen;
            state.from_phone = marked.from_phone;
            state.latest = marked.latest;
        }
        let settled = state.writing == 0;
        drop(state);
        if settled {
            self.settled.notify_waiters();
        }
        restored
    }

    /// Keeps a phone's text for the window, replacing older text.
    pub(super) fn hold(&self, clip: Clip, device_id: String) {
        self.lock().held = Some(Held { clip, device_id });
    }

    /// A phone's text went on the clipboard: older text that waited for
    /// the window must not replace it.
    pub(super) fn discard_held(&self) {
        self.lock().held = None;
    }

    /// Notes a phone's state.
    pub(super) fn set_phone(&self, device_id: &str, phone: PhoneClipboard) {
        self.lock().phones.insert(device_id.to_owned(), phone);
    }

    /// Takes the text that waits, to put it on the clipboard now.
    fn release(&self) -> Option<(Held, Marked)> {
        let held = self.lock().held.take()?;
        let marked = self.mark(held.clip.digest());
        Some((held, marked))
    }

    /// Holds `held` again after it could not go on the clipboard, unless a
    /// phone's newer text arrived meanwhile.
    fn put_back(&self, held: Held, marked: Marked) {
        if self.written(marked, false) {
            self.lock().held.get_or_insert(held);
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        // The state stays consistent between statements.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex as StdMutex;
    use std::sync::atomic::{AtomicBool, Ordering};

    use link2linux_platform::{ClipboardAccess, PlatformError};
    use tokio::time::Duration;

    use super::super::MAX_TEXT_BYTES;
    use super::*;

    fn text(text: &str) -> ClipboardText {
        ClipboardText {
            text: text.to_owned(),
            sensitive: false,
        }
    }

    fn secret(text: &str) -> ClipboardText {
        ClipboardText {
            text: text.to_owned(),
            sensitive: true,
        }
    }

    /// The clipboard holds `content` now, read in a read that started now.
    fn copy(hub: &ClipboardHub, content: ClipboardText) -> Option<Arc<Clip>> {
        hub.copied(Some(content), Instant::now(), hub.generation())
    }

    fn hold(hub: &ClipboardHub, text: &str, device_id: &str) {
        hub.hold(Clip::new(self::text(text)), device_id.to_owned());
    }

    fn mark(hub: &ClipboardHub, text: &str) -> Marked {
        hub.mark(Clip::new(self::text(text)).digest())
    }

    /// A phone's text goes on the clipboard.
    fn put(hub: &ClipboardHub, text: &str) {
        let marked = mark(hub, text);
        hub.written(marked, true);
    }

    /// A clipboard that works only while the window has focus.
    #[derive(Default)]
    struct FocusOnly {
        content: StdMutex<Option<ClipboardText>>,
        broken: AtomicBool,
        /// Runs once during the next read, after it took the content.
        during_read: StdMutex<Option<Box<dyn FnOnce() + Send>>>,
    }

    impl FocusOnly {
        fn holding(text: &str) -> Self {
            let clipboard = Self::default();
            *clipboard.content.lock().unwrap() = Some(self::text(text));
            clipboard
        }

        fn text(&self) -> Option<String> {
            self.content.lock().unwrap().clone().map(|text| text.text)
        }
    }

    impl Clipboard for FocusOnly {
        fn access(&self) -> ClipboardAccess {
            ClipboardAccess::FocusOnly
        }

        async fn read(&self) -> Result<Option<ClipboardText>, PlatformError> {
            if self.broken.load(Ordering::Relaxed) {
                return Err(PlatformError::Unsupported("clipboard"));
            }
            let text = self.content.lock().unwrap().clone();
            let during = self.during_read.lock().unwrap().take();
            if let Some(during) = during {
                during();
            }
            Ok(text)
        }

        async fn write(&self, text: &ClipboardText) -> Result<(), PlatformError> {
            if self.broken.load(Ordering::Relaxed) {
                return Err(PlatformError::Unsupported("clipboard"));
            }
            *self.content.lock().unwrap() = Some(text.clone());
            Ok(())
        }

        async fn changed(&self) -> Result<(), PlatformError> {
            std::future::pending().await
        }
    }

    fn not_shared(events: &mut broadcast::Receiver<ClipboardEvent>) -> Vec<NotShared> {
        std::iter::from_fn(|| events.try_recv().ok())
            .filter_map(|event| match event {
                ClipboardEvent::NotShared(reason) => Some(reason),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_new_copy_is_shared_once() {
        let hub = ClipboardHub::new(ClipboardSettings::default());
        let clip = copy(&hub, text("a")).unwrap();
        assert_eq!(clip.text().text, "a");
        // The same content reported again, as when a clipboard manager
        // takes the copy over, is nothing new.
        assert!(copy(&hub, text("a")).is_none());
        assert!(copy(&hub, text("b")).is_some());
        assert!(copy(&hub, text("a")).is_some());
        // Copied files in between: the same text again is a new copy.
        assert!(hub.copied(None, Instant::now(), hub.generation()).is_none());
        assert!(copy(&hub, text("a")).is_some());
    }

    #[test]
    fn a_phones_text_is_not_shared_on_until_something_else_is_copied() {
        let hub = ClipboardHub::new(ClipboardSettings::default());
        put(&hub, "from the phone");
        assert!(copy(&hub, text("from the phone")).is_none());
        // A settings change does not make it new.
        hub.set_settings(ClipboardSettings::default());
        assert!(copy(&hub, text("from the phone")).is_none());

        // The user copies something else, then the phone's text again: a
        // copy of their own.
        assert!(copy(&hub, text("mine")).is_some());
        assert!(copy(&hub, text("from the phone")).is_some());
    }

    #[test]
    fn a_read_that_a_phones_write_overlaps_is_dropped() {
        let hub = ClipboardHub::new(ClipboardSettings::default());
        put(&hub, "first");
        // A read starts before the next phone text is marked...
        let before = hub.generation();
        let marked = mark(&hub, "second");
        // ...another starts while it is written...
        let during = hub.generation();
        assert!(
            hub.copied(Some(text("first")), Instant::now(), during)
                .is_none()
        );
        hub.written(marked, true);
        // ...and both return what the write replaced, which goes nowhere.
        for since in [before, during] {
            assert!(
                hub.copied(Some(text("first")), Instant::now(), since)
                    .is_none()
            );
        }
        // The second push is still recognized when the clipboard reports
        // it.
        assert!(copy(&hub, text("second")).is_none());
        assert!(copy(&hub, text("first")).is_some());
    }

    #[test]
    fn a_phones_text_that_did_not_go_on_the_clipboard_leaves_the_hub_as_it_was() {
        let hub = ClipboardHub::new(ClipboardSettings::default());
        put(&hub, "earlier phone text");
        let failed = mark(&hub, "x");
        assert!(hub.written(failed, false));
        // The earlier phone text is still on the clipboard and still not
        // shared on; x was never there.
        assert!(copy(&hub, text("earlier phone text")).is_none());
        assert!(copy(&hub, text("x")).is_some());

        // A failure after newer phone text was marked changes nothing.
        let older = mark(&hub, "older");
        let newer = mark(&hub, "newer");
        assert!(!hub.written(older, false));
        hub.written(newer, true);
        assert!(copy(&hub, text("newer")).is_none());
    }

    #[tokio::test]
    async fn a_read_overtaken_by_a_phones_text_is_made_again() {
        let hub = ClipboardHub::new(ClipboardSettings::default());
        let now = Instant::now();

        // The read takes the user's earlier copy, then the phone's text is
        // written over it: the second read finds the phone's text, and
        // nothing is shared.
        let clipboard = Arc::new(FocusOnly::holding("earlier"));
        let (during, writing) = (clipboard.clone(), hub.clone());
        *clipboard.during_read.lock().unwrap() = Some(Box::new(move || {
            let marked = writing.mark(Clip::new(text("from the phone")).digest());
            *during.content.lock().unwrap() = Some(text("from the phone"));
            writing.written(marked, true);
        }));
        assert!(hub.read_copy(clipboard.as_ref(), now).await.is_none());

        // The read takes a copy the user just made, and the phone's text
        // cannot be written over it: the copy is read again and shared.
        let clipboard = Arc::new(FocusOnly::holding("just copied"));
        let failing = hub.clone();
        *clipboard.during_read.lock().unwrap() = Some(Box::new(move || {
            let marked = failing.mark(Clip::new(text("from the phone")).digest());
            failing.written(marked, false);
        }));
        let copy = hub.read_copy(clipboard.as_ref(), now).await.unwrap();
        assert_eq!(copy.text().text, "just copied");
    }

    #[test]
    fn copies_the_user_should_know_about_are_reported() {
        let hub = ClipboardHub::new(ClipboardSettings::default());
        let mut events = hub.subscribe();
        let long = "a".repeat(MAX_TEXT_BYTES + 1);
        assert!(copy(&hub, text(&long)).is_none());
        assert!(copy(&hub, secret("hunter2")).is_none());
        // Reported once per copy.
        assert!(copy(&hub, secret("hunter2")).is_none());
        assert!(copy(&hub, text("")).is_none());
        assert_eq!(
            not_shared(&mut events),
            [
                NotShared::TooLarge {
                    bytes: MAX_TEXT_BYTES + 1
                },
                NotShared::Secret
            ]
        );
    }

    #[test]
    fn a_copy_kept_here_stays_here_when_the_settings_change() {
        let hub = ClipboardHub::new(ClipboardSettings::default());
        let mut events = hub.subscribe();
        hub.set_settings(ClipboardSettings {
            sharing: false,
            ..ClipboardSettings::default()
        });
        // Copied while sharing is off, then sharing goes back on: the copy
        // is not shared when the clipboard reports it again.
        assert!(copy(&hub, text("private note")).is_none());
        hub.set_settings(ClipboardSettings::default());
        assert!(copy(&hub, text("private note")).is_none());

        // A secret copied before "Share passwords" went on, likewise.
        assert!(copy(&hub, secret("hunter2")).is_none());
        hub.set_settings(ClipboardSettings {
            share_passwords: true,
            ..ClipboardSettings::default()
        });
        assert!(copy(&hub, secret("hunter2")).is_none());
        assert_eq!(not_shared(&mut events), [NotShared::Secret]);
        // A new one goes.
        assert!(copy(&hub, secret("new secret")).unwrap().is_sensitive());
    }

    #[test]
    fn nothing_is_shared_or_kept_while_sharing_is_off() {
        let hub = ClipboardHub::new(ClipboardSettings::default());
        let mut events = hub.subscribe();
        let now = Instant::now();
        copy(&hub, text("a")).unwrap();
        hold(&hub, "p", "phone-1");
        hub.set_settings(ClipboardSettings {
            sharing: false,
            ..ClipboardSettings::default()
        });
        assert!(hub.recent(now).is_none());
        assert!(hub.held().is_none());
        assert!(copy(&hub, text("b")).is_none());
        assert_eq!(hub.send(Some(text("c")), now).unwrap_err(), NotShared::Off);
        assert!(not_shared(&mut events).is_empty());
    }

    #[test]
    fn the_latest_copy_is_recent_for_a_minute_while_the_clipboard_holds_it() {
        let hub = ClipboardHub::new(ClipboardSettings::default());
        let start = Instant::now();
        assert!(hub.recent(start).is_none());
        hub.copied(Some(text("a")), start, hub.generation())
            .unwrap();
        let later = start + RECENT_COPY - Duration::from_millis(1);
        assert_eq!(hub.recent(later).unwrap().text().text, "a");
        assert!(hub.recent(start + RECENT_COPY).is_none());

        // The clipboard moves on to something that stays here: nothing is
        // recent any more.
        for moved_on in [Some(secret("hunter2")), None] {
            hub.copied(Some(text("b")), start, hub.generation())
                .unwrap();
            hub.copied(moved_on, start, hub.generation());
            assert!(hub.recent(start).is_none());
        }
        // Nor after a phone's text went on it.
        hub.copied(Some(text("c")), start, hub.generation())
            .unwrap();
        put(&hub, "p");
        assert!(hub.recent(start).is_none());
    }

    #[test]
    fn sending_takes_any_text_within_the_limits_and_counts_from_now() {
        let hub = ClipboardHub::new(ClipboardSettings::default());
        let start = Instant::now();
        // Even the text a phone put there: the user asked.
        put(&hub, "p");
        assert_eq!(hub.send(Some(text("p")), start).unwrap().text().text, "p");
        assert_eq!(hub.send(None, start).unwrap_err(), NotShared::NoText);
        assert_eq!(
            hub.send(Some(text("")), start).unwrap_err(),
            NotShared::NoText
        );
        assert_eq!(
            hub.send(Some(secret("s")), start).unwrap_err(),
            NotShared::Secret
        );
        // Sent again much later, it is recent from the second send.
        let later = start + RECENT_COPY * 2;
        hub.send(Some(text("p")), later).unwrap();
        assert!(hub.recent(later + RECENT_COPY / 2).is_some());
        // The copy the user sent is no news to the watcher.
        hub.send(Some(text("q")), later).unwrap();
        assert!(copy(&hub, text("q")).is_none());
    }

    #[tokio::test]
    async fn sending_the_clipboard_reports_why_it_did_not_go() {
        let hub = ClipboardHub::new(ClipboardSettings::default());
        let mut events = hub.subscribe();
        let clipboard = FocusOnly::default();
        let now = Instant::now();
        assert_eq!(
            hub.send_clipboard(&clipboard, now).await.unwrap_err(),
            NotShared::NoText
        );
        clipboard.write(&text("hello")).await.unwrap();
        let sent = hub.send_clipboard(&clipboard, now).await.unwrap();
        assert_eq!(sent.text().text, "hello");
        clipboard.broken.store(true, Ordering::Relaxed);
        assert_eq!(
            hub.send_clipboard(&clipboard, now).await.unwrap_err(),
            NotShared::NoText
        );
        assert_eq!(
            not_shared(&mut events),
            [NotShared::NoText, NotShared::NoText]
        );
    }

    #[tokio::test]
    async fn focus_puts_the_newest_held_text_on_the_clipboard() {
        let hub = ClipboardHub::new(ClipboardSettings::default());
        let mut events = hub.subscribe();
        let clipboard = FocusOnly::holding("mine");
        hold(&hub, "older", "phone-1");
        hold(&hub, "newer", "phone-2");
        let now = Instant::now();
        // The window lost focus again before the clipboard was used.
        assert!(!hub.is_focused());
        assert!(hub.focused(&clipboard, now).await.is_none());
        assert_eq!(clipboard.text().unwrap(), "mine");

        hub.set_focused(true);
        assert!(hub.focused(&clipboard, now).await.is_none());
        assert_eq!(clipboard.text().unwrap(), "newer");
        assert!(hub.held().is_none());
        assert_eq!(
            events.try_recv().unwrap(),
            ClipboardEvent::Released {
                device_id: "phone-2".into()
            }
        );
        // Focused again: the clipboard holds the phone's text, which is
        // not shared on.
        assert!(hub.focused(&clipboard, now).await.is_none());

        // Then the user copies something while the window is away.
        clipboard.write(&text("copied here")).await.unwrap();
        let clip = hub.focused(&clipboard, now).await.unwrap();
        assert_eq!(clip.text().text, "copied here");
    }

    #[tokio::test]
    async fn held_text_wins_over_whatever_the_clipboard_held_and_that_is_not_shared() {
        let hub = ClipboardHub::new(ClipboardSettings::default());
        let clipboard = FocusOnly::holding("before");
        hub.set_focused(true);
        assert!(hub.focused(&clipboard, Instant::now()).await.is_some());
        hub.set_focused(false);

        // The user copies something in another app, and later the phone's
        // text arrives: the desktop cannot tell which came first.
        clipboard.write(&text("copied elsewhere")).await.unwrap();
        hold(&hub, "from the phone", "phone-1");
        hub.set_focused(true);
        assert!(hub.focused(&clipboard, Instant::now()).await.is_none());
        assert_eq!(clipboard.text().unwrap(), "from the phone");
        // Focused again: nothing new to share.
        assert!(hub.focused(&clipboard, Instant::now()).await.is_none());
    }

    #[tokio::test]
    async fn held_text_that_cannot_be_written_waits_on_unless_newer_text_came() {
        let hub = ClipboardHub::new(ClipboardSettings::default());
        let broken = FocusOnly::default();
        broken.broken.store(true, Ordering::Relaxed);
        hold(&hub, "p", "phone-1");
        hub.set_focused(true);
        assert!(hub.focused(&broken, Instant::now()).await.is_none());
        assert_eq!(hub.held().unwrap().clip.text().text, "p");
        // Not marked: the user copying the same text later shares it.
        assert!(copy(&hub, text("p")).is_some());

        // Newer phone text is marked while the held text is being written:
        // the held text is not held again over it.
        let (held, marked) = hub.release().unwrap();
        put(&hub, "newer");
        hub.put_back(held, marked);
        assert!(hub.held().is_none());
    }

    #[test]
    fn a_forgotten_phone_takes_its_state_and_held_text_along() {
        let hub = ClipboardHub::new(ClipboardSettings::default());
        hub.set_phone("phone-1", PhoneClipboard::Ready);
        hub.set_phone("phone-2", PhoneClipboard::TurnedOff);
        hold(&hub, "p", "phone-1");
        hub.forget_phone("phone-2");
        assert!(hub.held().is_some());
        hub.forget_phone("phone-1");
        assert!(hub.held().is_none());
        assert!(hub.phones().is_empty());
    }

    #[test]
    fn held_text_stays_out_of_debug_output() {
        let hub = ClipboardHub::new(ClipboardSettings::default());
        hold(&hub, "hunter2", "phone-1");
        let printed = format!("{:?}", hub.held().unwrap());
        assert!(!printed.contains("hunter2"), "{printed}");
    }
}
