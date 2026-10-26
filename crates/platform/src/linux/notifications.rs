//! Desktop notifications through the `org.freedesktop.Notifications` D-Bus
//! service, which every major Linux desktop provides.
//!
//! The service broadcasts `ActionInvoked` and `NotificationClosed` for every
//! app's notifications, so the sink remembers the ids it showed and ignores
//! the rest. KDE Plasma also offers a reply field inside the notification
//! (the `inline-reply` capability), answered with `NotificationReplied`;
//! elsewhere the caller shows a reply action instead. Some services read
//! the body as markup (`body-markup`), so the phone's text is escaped there.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use futures_util::StreamExt;
use tokio::sync::{Mutex, OnceCell};
use zbus::Connection;
use zbus::zvariant::Value;

use crate::{
    CloseReason, DesktopNotification, NotificationCapabilities, NotificationId,
    NotificationResponse, NotificationSink, PlatformError, Urgency,
};

const SERVICE: &str = "desktop notifications";
/// The desktop file the notifications belong to, so the desktop shows the
/// app's name and icon and groups them.
const DESKTOP_ENTRY: &str = "dev.link2linux.desktop";
/// The application name every pop-up is sent under. A phone app chooses
/// its own label, so sending that instead would let it pass for one of the
/// desktop's apps in the notification service's rules and history; the
/// phone app's name is in the summary.
const APP_NAME: &str = "link2linux";
/// The action key KDE Plasma reserves for its reply field.
const INLINE_REPLY_KEY: &str = "inline-reply";
const CALL_TIMEOUT: Duration = Duration::from_secs(5);

#[zbus::proxy(
    interface = "org.freedesktop.Notifications",
    default_service = "org.freedesktop.Notifications",
    default_path = "/org/freedesktop/Notifications"
)]
trait Notifications {
    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        app_name: &str,
        replaces_id: u32,
        app_icon: &str,
        summary: &str,
        body: &str,
        actions: &[&str],
        hints: HashMap<&str, Value<'_>>,
        expire_timeout: i32,
    ) -> zbus::Result<u32>;

    fn close_notification(&self, id: u32) -> zbus::Result<()>;

    fn get_capabilities(&self) -> zbus::Result<Vec<String>>;

    #[zbus(signal)]
    fn action_invoked(&self, id: u32, action_key: String) -> zbus::Result<()>;

    #[zbus(signal)]
    fn notification_closed(&self, id: u32, reason: u32) -> zbus::Result<()>;

    #[zbus(signal)]
    fn notification_replied(&self, id: u32, text: String) -> zbus::Result<()>;
}

fn unavailable(reason: impl Into<String>) -> PlatformError {
    PlatformError::Unavailable {
        service: SERVICE,
        reason: reason.into(),
    }
}

/// The signal streams, opened before the first notification is shown so no
/// answer to it can be missed.
struct Signals {
    actions: ActionInvokedStream,
    closed: NotificationClosedStream,
    replied: NotificationRepliedStream,
}

struct Live {
    proxy: NotificationsProxy<'static>,
    capabilities: Vec<String>,
    signals: Mutex<Signals>,
    /// The notifications this sink shows, with the action key of their
    /// reply field, if any.
    shown: std::sync::Mutex<HashMap<u32, Option<String>>>,
}

impl Live {
    fn supports(&self, capability: &str) -> bool {
        self.capabilities.iter().any(|c| c == capability)
    }
}

/// Desktop notifications through `org.freedesktop.Notifications`.
pub struct DbusNotifications {
    /// Where to reach the service; the session bus unless given.
    connection: Option<Connection>,
    live: OnceCell<Live>,
}

impl std::fmt::Debug for DbusNotifications {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DbusNotifications").finish_non_exhaustive()
    }
}

impl Default for DbusNotifications {
    fn default() -> Self {
        Self::new()
    }
}

impl DbusNotifications {
    /// Notifications on the session bus, which is opened on first use.
    #[must_use]
    pub fn new() -> Self {
        Self {
            connection: None,
            live: OnceCell::new(),
        }
    }

    /// Notifications on `connection`, for tests with a mock service.
    #[must_use]
    pub fn with_connection(connection: Connection) -> Self {
        Self {
            connection: Some(connection),
            live: OnceCell::new(),
        }
    }

    async fn live(&self) -> Result<&Live, PlatformError> {
        self.live
            .get_or_try_init(|| async {
                // Subscribing asks the bus who owns the service, so the whole
                // start is bounded: a bus or service that never answers must
                // not hang the caller.
                tokio::time::timeout(CALL_TIMEOUT, self.connect())
                    .await
                    .map_err(|_| unavailable("the notification service did not answer"))?
            })
            .await
    }

    async fn connect(&self) -> Result<Live, PlatformError> {
        let connection = match &self.connection {
            Some(connection) => connection.clone(),
            None => Connection::session()
                .await
                .map_err(|e| unavailable(format!("no session D-Bus: {e}")))?,
        };
        let proxy = NotificationsProxy::new(&connection)
            .await
            .map_err(|e| unavailable(e.to_string()))?;
        let signals = Signals {
            actions: proxy.receive_action_invoked().await.map_err(dbus)?,
            closed: proxy.receive_notification_closed().await.map_err(dbus)?,
            replied: proxy.receive_notification_replied().await.map_err(dbus)?,
        };
        let capabilities = proxy
            .get_capabilities()
            .await
            .map_err(|e| unavailable(format!("no notification service: {e}")))?;
        Ok(Live {
            proxy,
            capabilities,
            signals: Mutex::new(signals),
            shown: std::sync::Mutex::new(HashMap::new()),
        })
    }
}

fn dbus(error: zbus::Error) -> PlatformError {
    unavailable(error.to_string())
}

/// Escapes the characters that body markup gives a meaning to.
fn escape_markup(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            c => out.push(c),
        }
    }
    out
}

/// A `file://` URI for an absolute path, with everything but unreserved
/// characters and `/` percent-encoded.
fn file_uri(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut uri = String::from("file://");
    for &byte in path.as_os_str().as_bytes() {
        if byte.is_ascii_alphanumeric() || b"/-._~".contains(&byte) {
            uri.push(char::from(byte));
        } else {
            uri.push_str(&format!("%{byte:02X}"));
        }
    }
    uri
}

impl NotificationSink for DbusNotifications {
    async fn show(
        &self,
        notification: &DesktopNotification,
        replaces: Option<NotificationId>,
    ) -> Result<NotificationId, PlatformError> {
        let live = self.live().await?;
        let body = if live.supports("body-markup") {
            escape_markup(&notification.body)
        } else {
            notification.body.clone()
        };
        let mut actions: Vec<&str> = Vec::new();
        for action in &notification.actions {
            actions.extend([action.key.as_str(), action.label.as_str()]);
        }
        let inline_reply = notification
            .inline_reply
            .as_ref()
            .filter(|_| live.supports("inline-reply"));
        let mut hints: HashMap<&str, Value<'_>> = HashMap::new();
        let urgency: u8 = match notification.urgency {
            Urgency::Low => 0,
            Urgency::Normal => 1,
            Urgency::Critical => 2,
        };
        hints.insert("urgency", Value::U8(urgency));
        hints.insert("desktop-entry", Value::from(DESKTOP_ENTRY));
        if let Some(category) = &notification.category {
            hints.insert("category", Value::from(category.as_str()));
        }
        if let Some(path) = &notification.image_path {
            hints.insert("image-path", Value::from(file_uri(path)));
        }
        if notification.quiet {
            hints.insert("suppress-sound", Value::Bool(true));
        }
        if let Some(reply) = inline_reply {
            actions.extend([INLINE_REPLY_KEY, "Reply"]);
            hints.insert(
                "x-kde-reply-placeholder-text",
                Value::from(reply.placeholder.as_str()),
            );
        }

        let id = live
            .proxy
            .notify(
                APP_NAME,
                replaces.map_or(0, |id| id.0),
                "",
                &notification.summary,
                &body,
                &actions,
                hints,
                -1,
            )
            .await
            .map_err(dbus)?;
        live.shown
            .lock()
            .expect("not poisoned")
            .insert(id, inline_reply.map(|reply| reply.action_key.clone()));
        Ok(NotificationId(id))
    }

    async fn close(&self, id: NotificationId) -> Result<(), PlatformError> {
        let live = self.live().await?;
        // The id stays known until NotificationClosed reports it.
        live.proxy.close_notification(id.0).await.map_err(dbus)
    }

    async fn capabilities(&self) -> Result<NotificationCapabilities, PlatformError> {
        let live = self.live().await?;
        Ok(NotificationCapabilities {
            actions: live.supports("actions"),
            inline_reply: live.supports("inline-reply"),
            images: live.supports("icon-static") || live.supports("body-images"),
            persistence: live.supports("persistence"),
        })
    }

    async fn next_response(&self) -> Result<NotificationResponse, PlatformError> {
        let live = self.live().await?;
        let mut signals = live.signals.lock().await;
        let Signals {
            actions,
            closed,
            replied,
        } = &mut *signals;
        loop {
            let response = tokio::select! {
                signal = actions.next() => {
                    let signal = signal.ok_or_else(gone)?;
                    let args = signal.args().map_err(dbus)?;
                    let id = args.id;
                    // KDE answers its reply field with NotificationReplied.
                    if args.action_key == INLINE_REPLY_KEY {
                        continue;
                    }
                    is_ours(live, id).then_some(NotificationResponse::Action {
                        id: NotificationId(id),
                        key: args.action_key,
                    })
                }
                signal = replied.next() => {
                    let signal = signal.ok_or_else(gone)?;
                    let args = signal.args().map_err(dbus)?;
                    let action_key = live.shown.lock().expect("not poisoned").get(&args.id).cloned();
                    match action_key {
                        Some(Some(action_key)) => Some(NotificationResponse::Replied {
                            id: NotificationId(args.id),
                            action_key,
                            text: args.text,
                        }),
                        _ => None,
                    }
                }
                signal = closed.next() => {
                    let signal = signal.ok_or_else(gone)?;
                    let args = signal.args().map_err(dbus)?;
                    let ours = live.shown.lock().expect("not poisoned").remove(&args.id).is_some();
                    let reason = match args.reason {
                        1 => CloseReason::Expired,
                        2 => CloseReason::Dismissed,
                        3 => CloseReason::ClosedByApp,
                        _ => CloseReason::Unknown,
                    };
                    ours.then_some(NotificationResponse::Closed { id: NotificationId(args.id), reason })
                }
            };
            if let Some(response) = response {
                return Ok(response);
            }
        }
    }
}

fn is_ours(live: &Live, id: u32) -> bool {
    live.shown.lock().expect("not poisoned").contains_key(&id)
}

fn gone() -> PlatformError {
    unavailable("the notification service went away")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DesktopAction, InlineReply, Visibility};
    use std::sync::{Arc, Mutex as StdMutex};
    use tokio::net::UnixStream;
    use zbus::connection::Builder;
    use zbus::object_server::SignalEmitter;
    use zbus::zvariant::OwnedValue;

    const PATH: &str = "/org/freedesktop/Notifications";

    #[derive(Debug, Default)]
    struct Shown {
        app_name: String,
        replaces: u32,
        summary: String,
        body: String,
        actions: Vec<String>,
        hints: HashMap<String, OwnedValue>,
    }

    struct MockService {
        capabilities: Vec<String>,
        shown: Arc<StdMutex<Vec<Shown>>>,
        next_id: u32,
    }

    #[zbus::interface(name = "org.freedesktop.Notifications")]
    impl MockService {
        #[allow(clippy::too_many_arguments)]
        fn notify(
            &mut self,
            app_name: String,
            replaces_id: u32,
            _app_icon: String,
            summary: String,
            body: String,
            actions: Vec<String>,
            hints: HashMap<String, OwnedValue>,
            _expire_timeout: i32,
        ) -> u32 {
            self.shown.lock().unwrap().push(Shown {
                app_name,
                replaces: replaces_id,
                summary,
                body,
                actions,
                hints,
            });
            self.next_id += 1;
            if replaces_id == 0 {
                self.next_id
            } else {
                replaces_id
            }
        }

        async fn close_notification(
            &self,
            id: u32,
            #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
        ) -> zbus::fdo::Result<()> {
            Self::notification_closed(&emitter, id, 3).await?;
            Ok(())
        }

        fn get_capabilities(&self) -> Vec<String> {
            self.capabilities.clone()
        }

        #[zbus(signal)]
        async fn action_invoked(
            emitter: &SignalEmitter<'_>,
            id: u32,
            key: &str,
        ) -> zbus::Result<()>;

        #[zbus(signal)]
        async fn notification_closed(
            emitter: &SignalEmitter<'_>,
            id: u32,
            reason: u32,
        ) -> zbus::Result<()>;

        #[zbus(signal)]
        async fn notification_replied(
            emitter: &SignalEmitter<'_>,
            id: u32,
            text: &str,
        ) -> zbus::Result<()>;
    }

    struct Mock {
        sink: DbusNotifications,
        server: Connection,
        shown: Arc<StdMutex<Vec<Shown>>>,
    }

    impl Mock {
        async fn emitter(&self) -> SignalEmitter<'static> {
            SignalEmitter::new(&self.server, PATH).unwrap().into_owned()
        }
    }

    async fn mock(capabilities: &[&str]) -> Mock {
        let shown = Arc::new(StdMutex::new(Vec::new()));
        let (a, b) = UnixStream::pair().unwrap();
        let server = Builder::unix_stream(a)
            .server(zbus::Guid::generate())
            .unwrap()
            .p2p()
            .serve_at(
                PATH,
                MockService {
                    capabilities: capabilities.iter().map(|c| (*c).to_owned()).collect(),
                    shown: shown.clone(),
                    next_id: 0,
                },
            )
            .unwrap()
            .build();
        let client = Builder::unix_stream(b).p2p().build();
        let (server, client) = tokio::try_join!(server, client).unwrap();
        Mock {
            sink: DbusNotifications::with_connection(client),
            server,
            shown,
        }
    }

    fn message() -> DesktopNotification {
        DesktopNotification {
            app_name: "Messages".into(),
            summary: "Ana".into(),
            body: "See you at <b>8</b> & bring snacks".into(),
            actions: vec![DesktopAction {
                key: "mark-read".into(),
                label: "Mark as read".into(),
            }],
            urgency: Urgency::Critical,
            category: Some("im.received".into()),
            image_path: Some("/tmp/l2l cache/ana.png".into()),
            inline_reply: Some(InlineReply {
                action_key: "reply".into(),
                placeholder: "Reply to Ana".into(),
            }),
            quiet: true,
            visibility: Visibility::Private,
        }
    }

    fn hint<T: TryFrom<OwnedValue>>(shown: &Shown, key: &str) -> Option<T> {
        shown
            .hints
            .get(key)
            .and_then(|v| T::try_from(v.clone()).ok())
    }

    #[tokio::test]
    async fn shows_actions_hints_and_an_escaped_body() {
        let m = mock(&["actions", "body-markup", "persistence"]).await;
        let id = m.sink.show(&message(), None).await.unwrap();
        assert_eq!(id, NotificationId(1));
        let shown = m.shown.lock().unwrap();
        let shown = &shown[0];
        assert_eq!(
            shown.app_name, "link2linux",
            "never the phone app's own label"
        );
        assert_eq!(shown.summary, "Ana");
        assert_eq!(
            shown.body,
            "See you at &lt;b&gt;8&lt;/b&gt; &amp; bring snacks"
        );
        // No reply field: the service lacks inline-reply.
        assert_eq!(shown.actions, ["mark-read", "Mark as read"]);
        assert_eq!(hint::<u8>(shown, "urgency"), Some(2));
        assert_eq!(
            hint::<String>(shown, "category").as_deref(),
            Some("im.received")
        );
        assert_eq!(
            hint::<String>(shown, "desktop-entry").as_deref(),
            Some(DESKTOP_ENTRY)
        );
        assert_eq!(
            hint::<String>(shown, "image-path").as_deref(),
            Some("file:///tmp/l2l%20cache/ana.png")
        );
        assert_eq!(hint::<bool>(shown, "suppress-sound"), Some(true));
        assert!(!shown.hints.contains_key("x-kde-reply-placeholder-text"));
    }

    #[tokio::test]
    async fn the_body_stays_plain_without_markup() {
        let m = mock(&["actions"]).await;
        m.sink.show(&message(), None).await.unwrap();
        assert_eq!(m.shown.lock().unwrap()[0].body, message().body);
    }

    #[tokio::test]
    async fn offers_the_reply_field_where_the_service_has_one() {
        let m = mock(&["actions", "inline-reply"]).await;
        let id = m
            .sink
            .show(&message(), Some(NotificationId(7)))
            .await
            .unwrap();
        assert_eq!(id, NotificationId(7), "replaces keeps the id");
        {
            let shown = m.shown.lock().unwrap();
            assert_eq!(shown[0].replaces, 7);
            assert_eq!(
                shown[0].actions,
                ["mark-read", "Mark as read", INLINE_REPLY_KEY, "Reply"]
            );
            assert_eq!(
                hint::<String>(&shown[0], "x-kde-reply-placeholder-text").as_deref(),
                Some("Reply to Ana")
            );
        }
        let emitter = m.emitter().await;
        MockService::notification_replied(&emitter, 7, "On my way")
            .await
            .unwrap();
        assert_eq!(
            m.sink.next_response().await.unwrap(),
            NotificationResponse::Replied {
                id: NotificationId(7),
                action_key: "reply".into(),
                text: "On my way".into(),
            }
        );
    }

    #[tokio::test]
    async fn reports_actions_and_why_notifications_closed() {
        let m = mock(&["actions"]).await;
        let id = m.sink.show(&message(), None).await.unwrap();
        let second = m.sink.show(&message(), None).await.unwrap();
        let emitter = m.emitter().await;
        // Another app's notification is ignored.
        MockService::action_invoked(&emitter, 99, "open")
            .await
            .unwrap();
        MockService::action_invoked(&emitter, id.0, "mark-read")
            .await
            .unwrap();
        assert_eq!(
            m.sink.next_response().await.unwrap(),
            NotificationResponse::Action {
                id,
                key: "mark-read".into()
            }
        );
        MockService::notification_closed(&emitter, id.0, 2)
            .await
            .unwrap();
        assert_eq!(
            m.sink.next_response().await.unwrap(),
            NotificationResponse::Closed {
                id,
                reason: CloseReason::Dismissed
            }
        );
        m.sink.close(second).await.unwrap();
        assert_eq!(
            m.sink.next_response().await.unwrap(),
            NotificationResponse::Closed {
                id: second,
                reason: CloseReason::ClosedByApp
            }
        );
        // Once closed, its signals are no longer ours.
        MockService::notification_closed(&emitter, second.0, 2)
            .await
            .unwrap();
        MockService::notification_closed(&emitter, 99, 1)
            .await
            .unwrap();
        let third = m.sink.show(&message(), None).await.unwrap();
        MockService::notification_closed(&emitter, third.0, 1)
            .await
            .unwrap();
        assert_eq!(
            m.sink.next_response().await.unwrap(),
            NotificationResponse::Closed {
                id: third,
                reason: CloseReason::Expired
            }
        );
    }

    #[tokio::test]
    async fn reads_the_capabilities() {
        let m = mock(&["actions", "inline-reply", "icon-static", "persistence"]).await;
        assert_eq!(
            m.sink.capabilities().await.unwrap(),
            NotificationCapabilities {
                actions: true,
                inline_reply: true,
                images: true,
                persistence: true,
            }
        );
        let bare = mock(&[]).await;
        assert_eq!(
            bare.sink.capabilities().await.unwrap(),
            NotificationCapabilities::default()
        );
    }

    #[tokio::test]
    async fn no_service_is_unavailable() {
        // A peer that serves nothing at the notifications path.
        let (a, b) = UnixStream::pair().unwrap();
        let server = Builder::unix_stream(a)
            .server(zbus::Guid::generate())
            .unwrap()
            .p2p()
            .build();
        let client = Builder::unix_stream(b).p2p().build();
        let (_server, client) = tokio::try_join!(server, client).unwrap();
        let sink = DbusNotifications::with_connection(client);
        assert!(matches!(
            sink.capabilities().await,
            Err(PlatformError::Unavailable { .. })
        ));
    }

    #[test]
    fn escapes_markup_and_encodes_file_uris() {
        assert_eq!(escape_markup("a<b>&c"), "a&lt;b&gt;&amp;c");
        assert_eq!(escape_markup("plain"), "plain");
        assert_eq!(
            file_uri(Path::new("/a b/ü.png")),
            "file:///a%20b/%C3%BC.png"
        );
    }

    /// Needs a desktop session with a notification service; shows one
    /// notification for a moment.
    #[tokio::test]
    #[ignore = "needs a desktop notification service"]
    async fn shows_a_notification_on_the_real_desktop() {
        let sink = DbusNotifications::new();
        println!("{:?}", sink.capabilities().await.unwrap());
        let id = sink
            .show(
                &DesktopNotification {
                    app_name: "link2linux".into(),
                    summary: "link2linux test".into(),
                    body: "If you can read this, notifications work.".into(),
                    ..DesktopNotification::default()
                },
                None,
            )
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_secs(2)).await;
        sink.close(id).await.unwrap();
    }
}
