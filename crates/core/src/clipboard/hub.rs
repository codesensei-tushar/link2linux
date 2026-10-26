//! What the clipboard services of all paired phones share.

use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use link2linux_platform::{Clipboard, ClipboardText};
use tokio::sync::broadcast;
use tokio::time::Instant;

use super::{Clip, ClipboardEvent, ClipboardSettings, Digest, NotShared, RECENT_COPY, check};

/// Text a phone pushed that waits for the window to gain focus, on a
/// desktop that lets the app use its clipboard only then.
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

#[derive(Default)]
struct State {
    settings: ClipboardSettings,
    /// The app's window has focus, so a clipboard that works only then
    /// works now.
    focused: bool,
    /// What the clipboard held when the desktop last looked.
    seen: Option<Digest>,
    /// The text a phone put on the clipboard, until the clipboard holds
    /// something else. It is not shared on by itself: not back to that
    /// phone, and not to the others.
    from_phone: Option<Digest>,
    /// The latest copy made on this desktop that may be shared, and when
    /// it was made.
    latest: Option<(Arc<Clip>, Instant)>,
    held: Option<Held>,
}

/// The desktop-wide part of the clipboard feature, shared by the service
/// of every paired phone. Each copy of the handle shares the same state.
#[derive(Clone)]
pub struct ClipboardHub {
    state: Arc<Mutex<State>>,
    events: broadcast::Sender<ClipboardEvent>,
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
        }
    }

    /// The user's settings.
    #[must_use]
    pub fn settings(&self) -> ClipboardSettings {
        self.lock().settings
    }

    /// Changes the user's settings. Turning sharing off drops what waits:
    /// the latest copy and the text held for the window. Each phone's
    /// service tells its phone with
    /// [`ClipboardService::sharing_changed`](super::ClipboardService::sharing_changed).
    pub fn set_settings(&self, settings: ClipboardSettings) {
        let mut state = self.lock();
        state.settings = settings;
        // A copy refused under the old settings may pass the new ones.
        state.seen = None;
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

    /// The clipboard holds `text` now, as seen by watching it or when the
    /// window gained focus. Returns the copy to share with every phone, or
    /// `None` when nothing new is to be shared: the same text as before,
    /// the text a phone put there, or one the limits or the settings keep
    /// here. Too long text and secrets are reported as
    /// [`ClipboardEvent::NotShared`].
    pub fn copied(&self, text: ClipboardText, now: Instant) -> Option<Arc<Clip>> {
        let clip = Clip::new(text);
        let mut state = self.lock();
        if state.seen.replace(clip.digest()) == Some(clip.digest()) {
            return None;
        }
        if state.from_phone == Some(clip.digest()) {
            return None;
        }
        state.from_phone = None;
        match check(&clip, state.settings) {
            Ok(()) => Some(state.latest(clip, now)),
            Err(reason) => {
                drop(state);
                if matches!(reason, NotShared::TooLarge { .. } | NotShared::Secret) {
                    tracing::info!(bytes = clip.len(), ?reason, "a copy is not shared");
                    self.emit(ClipboardEvent::NotShared(reason));
                }
                None
            }
        }
    }

    /// The user asked to send `text` to every phone. Returns the copy to
    /// send, or why it cannot go.
    ///
    /// # Errors
    ///
    /// Fails when the clipboard holds no text, when the limits or the
    /// settings keep it here, and while sharing is off.
    pub fn send(&self, text: Option<ClipboardText>, now: Instant) -> Result<Arc<Clip>, NotShared> {
        let clip = Clip::new(text.ok_or(NotShared::NoText)?);
        let mut state = self.lock();
        check(&clip, state.settings)?;
        state.seen = Some(clip.digest());
        Ok(state.latest(clip, now))
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
    /// clipboard only then: puts the text that waits on the clipboard, or
    /// else reads the clipboard as a copy the user may have made while the
    /// window was in the background, which is returned as for
    /// [`Self::copied`]. Call it after [`Self::set_focused`] said so; it
    /// does nothing once the window has lost focus again.
    pub async fn focused<C: Clipboard>(&self, clipboard: &C, now: Instant) -> Option<Arc<Clip>> {
        if !self.is_focused() {
            return None;
        }
        if let Some(held) = self.release() {
            match clipboard.write(held.clip.text()).await {
                Ok(()) => self.emit(ClipboardEvent::Released {
                    device_id: held.device_id,
                }),
                Err(error) => {
                    tracing::warn!(%error, "could not put the phone's text on the clipboard");
                    self.unmark(held.clip.digest());
                    self.hold_back(held);
                }
            }
            return None;
        }
        match clipboard.read().await {
            Ok(Some(text)) => self.copied(text, now),
            Ok(None) => None,
            Err(error) => {
                tracing::info!(%error, "could not read the clipboard");
                None
            }
        }
    }

    /// The latest copy, if the user made it within [`RECENT_COPY`] before
    /// `now`.
    pub(super) fn recent(&self, now: Instant) -> Option<Arc<Clip>> {
        let state = self.lock();
        let (clip, at) = state.latest.as_ref()?;
        (now.saturating_duration_since(*at) < RECENT_COPY).then(|| clip.clone())
    }

    /// A phone's text is about to go on the clipboard: when the clipboard
    /// reports it, it is not shared on.
    pub(super) fn mark(&self, digest: Digest) {
        let mut state = self.lock();
        state.from_phone = Some(digest);
        state.seen = Some(digest);
    }

    /// The phone's text did not go on the clipboard after all.
    pub(super) fn unmark(&self, digest: Digest) {
        let mut state = self.lock();
        if state.from_phone == Some(digest) {
            state.from_phone = None;
            state.seen = None;
        }
    }

    /// Keeps a phone's text for the window, replacing older text.
    pub(super) fn hold(&self, held: Held) {
        self.lock().held = Some(held);
    }

    /// A phone's text went on the clipboard: older text that waited for
    /// the window must not replace it.
    pub(super) fn discard_held(&self) {
        self.lock().held = None;
    }

    /// Takes the text that waits, to put it on the clipboard now.
    fn release(&self) -> Option<Held> {
        let held = self.lock().held.take()?;
        self.mark(held.clip.digest());
        Some(held)
    }

    /// Holds `held` again after it could not go on the clipboard, unless
    /// newer text arrived meanwhile.
    fn hold_back(&self, held: Held) {
        self.lock().held.get_or_insert(held);
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        // The state stays consistent between statements.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl State {
    /// Makes `clip` the latest copy. The same text copied again keeps the
    /// time of its first copy, so that it does not count as recent forever.
    fn latest(&mut self, clip: Clip, now: Instant) -> Arc<Clip> {
        match &self.latest {
            Some((latest, _)) if latest.digest() == clip.digest() => latest.clone(),
            _ => {
                let clip = Arc::new(clip);
                self.latest = Some((clip.clone(), now));
                clip
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex as StdMutex;

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

    fn held(text: &str, device_id: &str) -> Held {
        Held {
            clip: Clip::new(self::text(text)),
            device_id: device_id.to_owned(),
        }
    }

    /// A clipboard that works only while the window has focus.
    #[derive(Default)]
    struct FocusOnly {
        content: StdMutex<Option<ClipboardText>>,
        broken: bool,
    }

    impl Clipboard for FocusOnly {
        fn access(&self) -> ClipboardAccess {
            ClipboardAccess::FocusOnly
        }

        async fn read(&self) -> Result<Option<ClipboardText>, PlatformError> {
            if self.broken {
                return Err(PlatformError::Unsupported("clipboard"));
            }
            Ok(self.content.lock().unwrap().clone())
        }

        async fn write(&self, text: &ClipboardText) -> Result<(), PlatformError> {
            if self.broken {
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
        let now = Instant::now();
        let clip = hub.copied(text("a"), now).unwrap();
        assert_eq!(clip.text().text, "a");
        // The same content reported again, as when a clipboard manager
        // takes the copy over, is nothing new.
        assert!(hub.copied(text("a"), now).is_none());
        assert!(hub.copied(text("b"), now).is_some());
        assert!(hub.copied(text("a"), now).is_some());
    }

    #[test]
    fn a_phones_text_is_not_shared_on_until_something_else_is_copied() {
        let hub = ClipboardHub::new(ClipboardSettings::default());
        let now = Instant::now();
        let from_phone = Clip::new(text("from the phone"));
        hub.mark(from_phone.digest());
        assert!(hub.copied(text("from the phone"), now).is_none());
        // Reported again: still the phone's.
        hub.set_settings(ClipboardSettings::default());
        assert!(hub.copied(text("from the phone"), now).is_none());

        // The user copies something else, then the phone's text again: a
        // copy of their own.
        assert!(hub.copied(text("mine"), now).is_some());
        assert!(hub.copied(text("from the phone"), now).is_some());
    }

    #[test]
    fn a_phones_text_that_did_not_go_on_the_clipboard_is_forgotten() {
        let hub = ClipboardHub::new(ClipboardSettings::default());
        let clip = Clip::new(text("x"));
        hub.mark(clip.digest());
        hub.unmark(clip.digest());
        assert!(hub.copied(text("x"), Instant::now()).is_some());
    }

    #[test]
    fn copies_the_user_should_know_about_are_reported() {
        let hub = ClipboardHub::new(ClipboardSettings::default());
        let mut events = hub.subscribe();
        let now = Instant::now();
        let long = "a".repeat(MAX_TEXT_BYTES + 1);
        assert!(hub.copied(text(&long), now).is_none());
        assert!(hub.copied(secret("hunter2"), now).is_none());
        // Reported once per copy.
        assert!(hub.copied(secret("hunter2"), now).is_none());
        assert!(hub.copied(text(""), now).is_none());
        assert_eq!(
            not_shared(&mut events),
            [
                NotShared::TooLarge {
                    bytes: MAX_TEXT_BYTES + 1
                },
                NotShared::Secret
            ]
        );

        // With "Share passwords" on, secrets go.
        hub.set_settings(ClipboardSettings {
            share_passwords: true,
            ..ClipboardSettings::default()
        });
        let clip = hub.copied(secret("hunter2"), now).unwrap();
        assert!(clip.is_sensitive());
    }

    #[test]
    fn nothing_is_shared_or_kept_while_sharing_is_off() {
        let hub = ClipboardHub::new(ClipboardSettings::default());
        let mut events = hub.subscribe();
        let now = Instant::now();
        hub.copied(text("a"), now).unwrap();
        hub.hold(held("p", "phone-1"));
        hub.set_settings(ClipboardSettings {
            sharing: false,
            ..ClipboardSettings::default()
        });
        assert!(hub.recent(now).is_none());
        assert!(hub.held().is_none());
        assert!(hub.copied(text("b"), now).is_none());
        assert_eq!(hub.send(Some(text("c")), now).unwrap_err(), NotShared::Off);
        assert!(not_shared(&mut events).is_empty());
    }

    #[test]
    fn the_latest_copy_is_recent_for_a_minute_from_its_first_copy() {
        let hub = ClipboardHub::new(ClipboardSettings::default());
        let start = Instant::now();
        assert!(hub.recent(start).is_none());
        hub.copied(text("a"), start).unwrap();
        hub.copied(text("b"), start).unwrap();
        // Copied again later: still as old as its first copy.
        hub.copied(text("b"), start + RECENT_COPY / 2);
        let later = start + RECENT_COPY - Duration::from_millis(1);
        assert_eq!(hub.recent(later).unwrap().text().text, "b");
        assert!(hub.recent(start + RECENT_COPY).is_none());
        // A phone's text never becomes the latest copy.
        let from_phone = Clip::new(text("p"));
        hub.mark(from_phone.digest());
        assert!(hub.copied(text("p"), start).is_none());
        assert_eq!(hub.recent(start).unwrap().text().text, "b");
    }

    #[test]
    fn sending_takes_any_text_within_the_limits() {
        let hub = ClipboardHub::new(ClipboardSettings::default());
        let now = Instant::now();
        // Even the text a phone put there: the user asked.
        let from_phone = Clip::new(text("p"));
        hub.mark(from_phone.digest());
        assert_eq!(hub.send(Some(text("p")), now).unwrap().text().text, "p");
        assert_eq!(hub.send(None, now).unwrap_err(), NotShared::NoText);
        assert_eq!(
            hub.send(Some(text("")), now).unwrap_err(),
            NotShared::NoText
        );
        assert_eq!(
            hub.send(Some(secret("s")), now).unwrap_err(),
            NotShared::Secret
        );
        assert_eq!(hub.recent(now).unwrap().text().text, "p");
        // The copy the user sent is no news to the watcher.
        hub.send(Some(text("q")), now).unwrap();
        assert!(hub.copied(text("q"), now).is_none());
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
        let broken = FocusOnly {
            broken: true,
            ..FocusOnly::default()
        };
        assert_eq!(
            hub.send_clipboard(&broken, now).await.unwrap_err(),
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
        let clipboard = FocusOnly::default();
        clipboard.write(&text("mine")).await.unwrap();
        hub.hold(held("older", "phone-1"));
        hub.hold(held("newer", "phone-2"));
        let now = Instant::now();
        // The window lost focus again before the clipboard was used.
        assert!(!hub.is_focused());
        assert!(hub.focused(&clipboard, now).await.is_none());
        assert_eq!(clipboard.read().await.unwrap().unwrap().text, "mine");

        hub.set_focused(true);
        assert!(hub.focused(&clipboard, now).await.is_none());
        assert_eq!(clipboard.read().await.unwrap().unwrap().text, "newer");
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
    async fn held_text_that_cannot_be_written_waits_on() {
        let hub = ClipboardHub::new(ClipboardSettings::default());
        let broken = FocusOnly {
            broken: true,
            ..FocusOnly::default()
        };
        hub.hold(held("p", "phone-1"));
        hub.set_focused(true);
        assert!(hub.focused(&broken, Instant::now()).await.is_none());
        assert_eq!(hub.held().unwrap().clip.text().text, "p");
        // Not marked: the user copying the same text later shares it.
        assert!(hub.copied(text("p"), Instant::now()).is_some());
    }

    #[test]
    fn held_text_stays_out_of_debug_output() {
        let printed = format!("{:?}", held("hunter2", "phone-1"));
        assert!(!printed.contains("hunter2"), "{printed}");
    }
}
