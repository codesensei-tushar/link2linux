//! What the desktop's pop-ups show of a phone's notifications.
//!
//! A pop-up is not private to this app: what it holds travels over the
//! session bus, which other programs of the user can watch, and the
//! desktop keeps it in its notification history. So by default a pop-up
//! names only the phone's app, and the content stays in the link2linux
//! window, which a click on the pop-up opens. The user can let pop-ups show
//! the text; even then a locked screen gets pop-ups without it, and none
//! at all for what the phone's app marked secret.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

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
    open_window: Box<dyn Fn() + Send + Sync>,
}

impl PrivatePopups {
    /// Pop-ups through `inner`, with the text when `show_text` is on.
    /// `open_window` brings up the app window when the user clicks one.
    pub fn new(
        inner: Arc<dyn Presenter>,
        show_text: bool,
        open_window: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        Self {
            inner,
            show_text: AtomicBool::new(show_text),
            locked: AtomicBool::new(false),
            open_window: Box::new(open_window),
        }
    }

    /// The user's setting: whether pop-ups show the notification's text.
    pub fn set_show_text(&self, show_text: bool) {
        self.show_text.store(show_text, Ordering::Relaxed);
    }

    /// Whether the desktop's screen is locked.
    pub fn set_locked(&self, locked: bool) {
        self.locked.store(locked, Ordering::Relaxed);
    }

    /// The pop-up for `notification` as things stand, or `None` when it
    /// must not appear at all.
    fn popup(&self, notification: &DesktopNotification) -> Option<DesktopNotification> {
        let locked = self.locked.load(Ordering::Relaxed);
        if locked && notification.visibility == Visibility::Secret {
            return None;
        }
        let mut popup = if self.show_text.load(Ordering::Relaxed) && !locked {
            notification.clone()
        } else {
            DesktopNotification {
                app_name: notification.app_name.clone(),
                summary: notification.app_name.clone(),
                body: NEUTRAL_TEXT.to_owned(),
                urgency: notification.urgency,
                category: notification.category.clone(),
                quiet: notification.quiet,
                visibility: notification.visibility,
                ..DesktopNotification::default()
            }
        };
        // Clicking the pop-up itself reports this action.
        if !popup.actions.iter().any(|action| action.key == OPEN_KEY) {
            popup.actions.insert(
                0,
                DesktopAction {
                    key: OPEN_KEY.to_owned(),
                    label: "Open".to_owned(),
                },
            );
        }
        Some(popup)
    }
}

impl Presenter for PrivatePopups {
    fn show<'a>(
        &'a self,
        notification: &'a DesktopNotification,
        replaces: Option<NotificationId>,
    ) -> BoxFuture<'a, Result<NotificationId, PlatformError>> {
        Box::pin(async move {
            let replaces = replaces.filter(|id| *id != NOT_SHOWN);
            match self.popup(notification) {
                Some(popup) => self.inner.show(&popup, replaces).await,
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

    fn close(&self, id: NotificationId) -> BoxFuture<'_, Result<(), PlatformError>> {
        if id == NOT_SHOWN {
            return Box::pin(async { Ok(()) });
        }
        self.inner.close(id)
    }

    fn capabilities(&self) -> BoxFuture<'_, Result<NotificationCapabilities, PlatformError>> {
        self.inner.capabilities()
    }

    fn next_response(&self) -> BoxFuture<'_, Result<NotificationResponse, PlatformError>> {
        Box::pin(async move {
            let response = self.inner.next_response().await?;
            if let NotificationResponse::Action { key, .. } = &response
                && key == OPEN_KEY
            {
                (self.open_window)();
            }
            Ok(response)
        })
    }
}

#[cfg(test)]
mod tests {
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
        responses: tokio::sync::Mutex<mpsc::UnboundedReceiver<NotificationResponse>>,
    }

    impl NotificationSink for Desktop {
        async fn show(
            &self,
            notification: &DesktopNotification,
            replaces: Option<NotificationId>,
        ) -> Result<NotificationId, PlatformError> {
            let mut shown = self.shown.lock().unwrap();
            shown.push((notification.clone(), replaces));
            Ok(NotificationId(u32::try_from(shown.len()).unwrap()))
        }

        async fn close(&self, id: NotificationId) -> Result<(), PlatformError> {
            self.closed.lock().unwrap().push(id);
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
        rig.popups
            .show(&message(Visibility::Private), None)
            .await
            .unwrap();

        let mut expected = message(Visibility::Private);
        expected.actions.insert(0, open_action());
        assert_eq!(rig.last_shown(), expected);

        rig.popups.set_locked(true);
        rig.popups
            .show(&message(Visibility::Public), None)
            .await
            .unwrap();
        assert_eq!(rig.last_shown().body, NEUTRAL_TEXT);
    }

    #[tokio::test]
    async fn secret_notifications_do_not_appear_on_a_locked_screen() {
        let rig = rig(true);
        let first = rig
            .popups
            .show(&message(Visibility::Secret), None)
            .await
            .unwrap();
        assert_ne!(first, NOT_SHOWN);

        rig.popups.set_locked(true);
        let id = rig
            .popups
            .show(&message(Visibility::Secret), Some(first))
            .await
            .unwrap();
        assert_eq!(id, NOT_SHOWN);
        // The earlier pop-up is taken down, and nothing new appears.
        assert_eq!(*rig.desktop.closed.lock().unwrap(), [first]);
        assert_eq!(rig.desktop.shown.lock().unwrap().len(), 1);
        rig.popups.close(id).await.unwrap();
        assert_eq!(rig.desktop.closed.lock().unwrap().len(), 1);

        // Once unlocked, an update is a new pop-up rather than a replacement.
        rig.popups.set_locked(false);
        rig.popups
            .show(&message(Visibility::Secret), Some(id))
            .await
            .unwrap();
        assert_eq!(rig.desktop.shown.lock().unwrap().last().unwrap().1, None);
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
}
