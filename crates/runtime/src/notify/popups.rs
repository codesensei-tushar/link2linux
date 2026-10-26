//! What the desktop's pop-ups show of a phone's notifications.
//!
//! A pop-up is not private to this app: what it holds travels over the
//! session bus, which other programs of the user can watch, and the
//! desktop keeps it in its notification history. So by default a pop-up
//! names only the phone's app, and the content stays in the link2linux
//! window, which a click on the pop-up opens. The user can let pop-ups show
//! the text; even then a locked screen gets pop-ups without it, and none
//! at all for what the phone's app marked secret. The screen counts as
//! locked whenever the app cannot tell, and locking it takes the text off
//! the pop-ups still on screen.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use link2linux_core::notifications::present::OPEN_KEY;
use link2linux_platform::{
    DesktopAction, DesktopNotification, NotificationCapabilities, NotificationId,
    NotificationResponse, PlatformError, Visibility,
};

use super::{BoxFuture, Presenter};

/// The line a pop-up shows instead of the notification's text.
pub const NEUTRAL_TEXT: &str = "New notification";

/// Stands for a pop-up that was not shown. Notification services never
/// hand out 0, so it cannot be confused with a real one.
const NOT_SHOWN: NotificationId = NotificationId(0);

/// A [`Presenter`] that decides what each pop-up shows before the desktop
/// gets it.
pub struct PrivatePopups {
    inner: Arc<dyn Presenter>,
    show_text: AtomicBool,
    locked: AtomicBool,
    /// The pop-ups on screen that change once the screen locks, each with
    /// the form it takes then: without the text, or `None` to go.
    until_locked: Mutex<HashMap<NotificationId, Option<DesktopNotification>>>,
    open_window: Box<dyn Fn() + Send + Sync>,
}

impl PrivatePopups {
    /// Pop-ups through `inner`, with the text when `show_text` is on.
    /// `open_window` brings up the app window when the user clicks one.
    /// Until [`Self::set_locked`] says otherwise, the screen counts as
    /// locked.
    pub fn new(
        inner: Arc<dyn Presenter>,
        show_text: bool,
        open_window: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        Self {
            inner,
            show_text: AtomicBool::new(show_text),
            locked: AtomicBool::new(true),
            until_locked: Mutex::default(),
            open_window: Box::new(open_window),
        }
    }

    /// The user's setting: whether pop-ups show the notification's text.
    pub fn set_show_text(&self, show_text: bool) {
        self.show_text.store(show_text, Ordering::Relaxed);
    }

    /// Whether the desktop's screen is locked: pass `true` also when that
    /// is unknown. Locking takes the text off the pop-ups still on screen,
    /// and takes down those that must not appear on a locked screen.
    pub async fn set_locked(&self, locked: bool) {
        let was_locked = self.locked.swap(locked, Ordering::Relaxed);
        if !locked || was_locked {
            return;
        }
        let on_screen: Vec<_> = self.on_screen().drain().collect();
        for (id, when_locked) in on_screen {
            self.hide_text(id, when_locked).await;
        }
    }

    /// Shows pop-up `id` again in its form for a locked screen, or takes it
    /// down when it has none.
    async fn hide_text(&self, id: NotificationId, when_locked: Option<DesktopNotification>) {
        // Replacing keeps the id: the notification service answers a
        // replacement with the id it replaced. Another id is a new pop-up,
        // as for an id the service no longer shows after it restarted, and
        // nothing would ever take that one down.
        let hidden = match when_locked {
            Some(popup) => match self.inner.show(&popup, Some(id)).await {
                Ok(shown) if shown != id => self.inner.close(shown).await,
                shown => shown.map(drop),
            },
            None => self.inner.close(id).await,
        };
        if let Err(error) = hidden {
            tracing::warn!(%error, "taking the text off a pop-up failed");
        }
    }

    fn on_screen(
        &self,
    ) -> std::sync::MutexGuard<'_, HashMap<NotificationId, Option<DesktopNotification>>> {
        self.until_locked
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The pop-up for `notification` as things stand, and whether it shows
    /// the text, or `None` when it must not appear at all.
    fn popup(&self, notification: &DesktopNotification) -> Option<(DesktopNotification, bool)> {
        let locked = self.locked.load(Ordering::Relaxed);
        if locked && notification.visibility == Visibility::Secret {
            return None;
        }
        if self.show_text.load(Ordering::Relaxed) && !locked {
            Some((with_open(notification.clone()), true))
        } else {
            Some((neutral(notification), false))
        }
    }
}

/// `notification` without its text: only the app's name.
fn neutral(notification: &DesktopNotification) -> DesktopNotification {
    with_open(DesktopNotification {
        app_name: notification.app_name.clone(),
        summary: notification.app_name.clone(),
        body: NEUTRAL_TEXT.to_owned(),
        urgency: notification.urgency,
        category: notification.category.clone(),
        quiet: notification.quiet,
        visibility: notification.visibility,
        ..DesktopNotification::default()
    })
}

/// `popup` with the action that clicking the pop-up itself reports.
fn with_open(mut popup: DesktopNotification) -> DesktopNotification {
    if !popup.actions.iter().any(|action| action.key == OPEN_KEY) {
        popup.actions.insert(
            0,
            DesktopAction {
                key: OPEN_KEY.to_owned(),
                label: "Open".to_owned(),
            },
        );
    }
    popup
}

impl Presenter for PrivatePopups {
    fn show<'a>(
        &'a self,
        notification: &'a DesktopNotification,
        replaces: Option<NotificationId>,
    ) -> BoxFuture<'a, Result<NotificationId, PlatformError>> {
        Box::pin(async move {
            let replaces = replaces.filter(|id| *id != NOT_SHOWN);
            if let Some(old) = replaces {
                self.on_screen().remove(&old);
            }
            match self.popup(notification) {
                Some((popup, with_text)) => {
                    let id = self.inner.show(&popup, replaces).await?;
                    let when_locked = (notification.visibility != Visibility::Secret)
                        .then(|| neutral(notification));
                    // Without the text and not secret, it stays as it is.
                    if with_text || when_locked.is_none() {
                        self.on_screen().insert(id, when_locked.clone());
                        // The screen may have locked while this was shown.
                        if self.locked.load(Ordering::Relaxed)
                            && self.on_screen().remove(&id).is_some()
                        {
                            self.hide_text(id, when_locked).await;
                        }
                    }
                    Ok(id)
                }
                None => {
                    // An older pop-up of the same notification goes too.
                    if let Some(id) = replaces {
                        self.inner.close(id).await?;
                    }
                    Ok(NOT_SHOWN)
                }
            }
        })
    }

    fn show_own<'a>(
        &'a self,
        notification: &'a DesktopNotification,
        replaces: Option<NotificationId>,
    ) -> BoxFuture<'a, Result<NotificationId, PlatformError>> {
        // Nothing in it comes from the phone, so it shows as it is, also
        // on a locked screen.
        let popup = with_open(notification.clone());
        Box::pin(async move {
            let replaces = replaces.filter(|id| *id != NOT_SHOWN);
            self.inner.show(&popup, replaces).await
        })
    }

    fn close(&self, id: NotificationId) -> BoxFuture<'_, Result<(), PlatformError>> {
        if id == NOT_SHOWN {
            return Box::pin(async { Ok(()) });
        }
        self.on_screen().remove(&id);
        self.inner.close(id)
    }

    fn capabilities(&self) -> BoxFuture<'_, Result<NotificationCapabilities, PlatformError>> {
        self.inner.capabilities()
    }

    fn next_response(&self) -> BoxFuture<'_, Result<NotificationResponse, PlatformError>> {
        Box::pin(async move {
            let response = self.inner.next_response().await?;
            match &response {
                NotificationResponse::Action { key, .. } if key == OPEN_KEY => (self.open_window)(),
                NotificationResponse::Closed { id, .. } => {
                    self.on_screen().remove(id);
                }
                _ => {}
            }
            Ok(response)
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::sync::Mutex;
    use std::sync::atomic::AtomicUsize;

    use link2linux_platform::{InlineReply, NotificationSink, Urgency};
    use tokio::sync::mpsc;

    use super::*;

    /// Records what reaches the desktop, and answers with what the test
    /// sends.
    struct Desktop {
        shown: Mutex<Vec<(DesktopNotification, Option<NotificationId>)>>,
        closed: Mutex<Vec<NotificationId>>,
        /// The pop-ups on screen. Replacing one keeps its id; replacing
        /// one that is not on screen shows a new one.
        screen: Mutex<HashSet<NotificationId>>,
        responses: tokio::sync::Mutex<mpsc::UnboundedReceiver<NotificationResponse>>,
    }

    impl Desktop {
        /// The notification service restarted: its pop-ups are gone.
        fn restart(&self) {
            self.screen.lock().unwrap().clear();
        }
    }

    impl NotificationSink for Desktop {
        async fn show(
            &self,
            notification: &DesktopNotification,
            replaces: Option<NotificationId>,
        ) -> Result<NotificationId, PlatformError> {
            let mut shown = self.shown.lock().unwrap();
            shown.push((notification.clone(), replaces));
            let mut screen = self.screen.lock().unwrap();
            let id = replaces
                .filter(|id| screen.contains(id))
                .unwrap_or(NotificationId(u32::try_from(shown.len()).unwrap()));
            screen.insert(id);
            Ok(id)
        }

        async fn close(&self, id: NotificationId) -> Result<(), PlatformError> {
            self.closed.lock().unwrap().push(id);
            self.screen.lock().unwrap().remove(&id);
            Ok(())
        }

        async fn capabilities(&self) -> Result<NotificationCapabilities, PlatformError> {
            Ok(NotificationCapabilities::default())
        }

        async fn next_response(&self) -> Result<NotificationResponse, PlatformError> {
            let response = self.responses.lock().await.recv().await;
            response.ok_or(PlatformError::Unsupported("responses"))
        }
    }

    struct Rig {
        desktop: Arc<Desktop>,
        popups: PrivatePopups,
        respond: mpsc::UnboundedSender<NotificationResponse>,
        opened: Arc<AtomicUsize>,
    }

    fn rig(show_text: bool) -> Rig {
        let (respond, responses) = mpsc::unbounded_channel();
        let desktop = Arc::new(Desktop {
            shown: Mutex::default(),
            closed: Mutex::default(),
            screen: Mutex::default(),
            responses: tokio::sync::Mutex::new(responses),
        });
        let opened = Arc::new(AtomicUsize::new(0));
        let count = opened.clone();
        let popups = PrivatePopups::new(desktop.clone(), show_text, move || {
            count.fetch_add(1, Ordering::Relaxed);
        });
        Rig {
            desktop,
            popups,
            respond,
            opened,
        }
    }

    fn message(visibility: Visibility) -> DesktopNotification {
        DesktopNotification {
            app_name: "Chat".into(),
            summary: "Ana".into(),
            body: "The door code is 4512".into(),
            actions: vec![DesktopAction {
                key: "read".into(),
                label: "Mark as read".into(),
            }],
            urgency: Urgency::Normal,
            category: Some("im.received".into()),
            image_path: Some("/run/user/1000/link2linux/images/ana.png".into()),
            inline_reply: Some(InlineReply {
                action_key: "reply".into(),
                placeholder: "Reply".into(),
            }),
            quiet: false,
            visibility,
        }
    }

    fn open_action() -> DesktopAction {
        DesktopAction {
            key: OPEN_KEY.into(),
            label: "Open".into(),
        }
    }

    impl Rig {
        fn last_shown(&self) -> DesktopNotification {
            self.desktop.shown.lock().unwrap().last().unwrap().0.clone()
        }
    }

    #[tokio::test]
    async fn by_default_a_popup_names_only_the_app() {
        let rig = rig(false);
        rig.popups
            .show(&message(Visibility::Public), None)
            .await
            .unwrap();

        let popup = rig.last_shown();
        assert_eq!(popup.app_name, "Chat");
        assert_eq!(popup.summary, "Chat");
        assert_eq!(popup.body, NEUTRAL_TEXT);
        assert_eq!(popup.actions, [open_action()]);
        assert_eq!(popup.image_path, None);
        assert_eq!(popup.inline_reply, None);
        assert_eq!(popup.category.as_deref(), Some("im.received"));
    }

    #[tokio::test]
    async fn the_text_shows_when_the_user_allows_it_and_the_screen_is_unlocked() {
        let rig = rig(false);
        rig.popups.set_show_text(true);
        rig.popups.set_locked(false).await;
        rig.popups
            .show(&message(Visibility::Private), None)
            .await
            .unwrap();

        let mut expected = message(Visibility::Private);
        expected.actions.insert(0, open_action());
        assert_eq!(rig.last_shown(), expected);

        rig.popups.set_locked(true).await;
        rig.popups
            .show(&message(Visibility::Public), None)
            .await
            .unwrap();
        assert_eq!(rig.last_shown().body, NEUTRAL_TEXT);
    }

    #[tokio::test]
    async fn the_screen_counts_as_locked_until_its_state_is_known() {
        let rig = rig(true);
        rig.popups
            .show(&message(Visibility::Public), None)
            .await
            .unwrap();
        assert_eq!(rig.last_shown().body, NEUTRAL_TEXT);
        assert_eq!(
            rig.popups
                .show(&message(Visibility::Secret), None)
                .await
                .unwrap(),
            NOT_SHOWN
        );
    }

    #[tokio::test]
    async fn locking_takes_the_text_off_popups_on_screen() {
        let rig = rig(true);
        rig.popups.set_locked(false).await;
        let private = rig
            .popups
            .show(&message(Visibility::Private), None)
            .await
            .unwrap();
        let secret = rig
            .popups
            .show(&message(Visibility::Secret), None)
            .await
            .unwrap();
        let gone = rig
            .popups
            .show(&message(Visibility::Public), None)
            .await
            .unwrap();
        rig.respond
            .send(NotificationResponse::Closed {
                id: gone,
                reason: link2linux_platform::CloseReason::Expired,
            })
            .unwrap();
        rig.popups.next_response().await.unwrap();
        assert_eq!(rig.desktop.shown.lock().unwrap().len(), 3);

        rig.popups.set_locked(true).await;
        let shown = rig.desktop.shown.lock().unwrap().clone();
        assert_eq!(
            shown.len(),
            4,
            "only the pop-up still on screen is shown again"
        );
        let (again, replaces) = shown.last().unwrap();
        assert_eq!(*replaces, Some(private));
        assert_eq!(again.body, NEUTRAL_TEXT);
        assert_eq!(again.actions, [open_action()]);
        assert_eq!(*rig.desktop.closed.lock().unwrap(), [secret]);

        // Locking again changes nothing.
        rig.popups.set_locked(true).await;
        assert_eq!(rig.desktop.shown.lock().unwrap().len(), 4);
    }

    #[tokio::test]
    async fn secret_notifications_do_not_appear_on_a_locked_screen() {
        let rig = rig(true);
        rig.popups.set_locked(false).await;
        let first = rig
            .popups
            .show(&message(Visibility::Secret), None)
            .await
            .unwrap();
        assert_ne!(first, NOT_SHOWN);

        // Locking takes the earlier pop-up down at once.
        rig.popups.set_locked(true).await;
        assert_eq!(*rig.desktop.closed.lock().unwrap(), [first]);
        let id = rig
            .popups
            .show(&message(Visibility::Secret), Some(first))
            .await
            .unwrap();
        assert_eq!(id, NOT_SHOWN);
        // Nothing new appears.
        assert_eq!(rig.desktop.shown.lock().unwrap().len(), 1);
        let closed = rig.desktop.closed.lock().unwrap().len();
        rig.popups.close(id).await.unwrap();
        assert_eq!(rig.desktop.closed.lock().unwrap().len(), closed);

        // Once unlocked, an update is a new pop-up rather than a replacement.
        rig.popups.set_locked(false).await;
        rig.popups
            .show(&message(Visibility::Secret), Some(id))
            .await
            .unwrap();
        assert_eq!(rig.desktop.shown.lock().unwrap().last().unwrap().1, None);
    }

    #[tokio::test]
    async fn locking_takes_down_a_secret_popup_shown_without_its_text() {
        // The default setting: pop-ups never show the text.
        let rig = rig(false);
        rig.popups.set_locked(false).await;
        let private = rig
            .popups
            .show(&message(Visibility::Private), None)
            .await
            .unwrap();
        let secret = rig
            .popups
            .show(&message(Visibility::Secret), None)
            .await
            .unwrap();
        assert_ne!(secret, NOT_SHOWN);
        assert_eq!(rig.last_shown().body, NEUTRAL_TEXT);

        rig.popups.set_locked(true).await;
        assert_eq!(*rig.desktop.closed.lock().unwrap(), [secret]);
        // The other one names only the app already, so it stays.
        assert_eq!(rig.desktop.shown.lock().unwrap().len(), 2);
        assert_eq!(
            *rig.desktop.screen.lock().unwrap(),
            HashSet::from([private])
        );
    }

    #[tokio::test]
    async fn locking_after_the_service_restarted_leaves_no_popup_behind() {
        let rig = rig(true);
        rig.popups.set_locked(false).await;
        rig.popups
            .show(&message(Visibility::Private), None)
            .await
            .unwrap();
        // The pop-up went with the old service, which never said so.
        rig.desktop.restart();

        rig.popups.set_locked(true).await;
        assert!(
            rig.desktop.screen.lock().unwrap().is_empty(),
            "{:?}",
            rig.desktop.shown.lock().unwrap()
        );
    }

    #[tokio::test]
    async fn clicking_a_popup_opens_the_window() {
        let rig = rig(false);
        let id = rig
            .popups
            .show(&message(Visibility::Public), None)
            .await
            .unwrap();
        for key in ["read", OPEN_KEY] {
            rig.respond
                .send(NotificationResponse::Action {
                    id,
                    key: key.into(),
                })
                .unwrap();
            let response = rig.popups.next_response().await.unwrap();
            assert!(matches!(response, NotificationResponse::Action { key: k, .. } if k == key));
        }
        assert_eq!(rig.opened.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn the_apps_own_popups_show_as_they_are_and_open_the_window() {
        let rig = rig(false);
        let own = DesktopNotification {
            app_name: "link2linux".into(),
            summary: "Clipboard from Pixel".into(),
            body: "Click to put it on this computer's clipboard.".into(),
            ..DesktopNotification::default()
        };
        // Locked, and without the text of notifications: still as it is.
        let id = rig.popups.show_own(&own, None).await.unwrap();
        let shown = rig.desktop.shown.lock().unwrap().clone();
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].0.summary, own.summary);
        assert_eq!(shown[0].0.body, own.body);
        assert_eq!(shown[0].0.actions[0].key, OPEN_KEY);
        // Unlocking leaves it alone.
        rig.popups.set_locked(false).await;
        rig.popups.set_locked(true).await;
        assert_eq!(rig.desktop.shown.lock().unwrap().len(), 1);

        rig.respond
            .send(NotificationResponse::Action {
                id,
                key: OPEN_KEY.into(),
            })
            .unwrap();
        rig.popups.next_response().await.unwrap();
        assert_eq!(rig.opened.load(Ordering::Relaxed), 1);
    }
}
