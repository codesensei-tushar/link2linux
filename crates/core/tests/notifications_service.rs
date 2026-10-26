//! The notifications service on a desktop session, against a phone each
//! test scripts frame by frame, and a desktop that records its banners.

mod session_common;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use futures_util::{SinkExt, StreamExt};
use link2linux_core::Session;
use link2linux_core::notifications::{
    ImageCache, Mirror, NotificationService, NotificationSettings, ServiceError,
};
use link2linux_core::outbox::{MemoryOutboxStore, Outbox};
use link2linux_core::session::SessionEvent;
use link2linux_platform::{
    CloseReason, DesktopNotification, NotificationCapabilities, NotificationId,
    NotificationResponse, NotificationSink, PlatformError,
};
use link2linux_protocol::v1::{
    Ack, NotificationImportance, NotificationPosted, NotificationRemoved, NotificationSyncBatch,
    PhoneNotification, SyncCursor,
};
use link2linux_protocol::{Envelope, Payload};
use session_common::{
    RawPeer, all_ready, answer, connected_to, envelope, fast, notifying_phone_hello,
};
use tokio::io::DuplexStream;

/// What the desktop was asked to do.
#[derive(Debug, Default)]
struct Desktop {
    shown: Vec<(DesktopNotification, Option<NotificationId>)>,
    closed: Vec<NotificationId>,
}

/// A notification service that records instead of showing.
struct FakeSink {
    log: Arc<Mutex<Desktop>>,
}

impl NotificationSink for FakeSink {
    async fn show(
        &self,
        notification: &DesktopNotification,
        replaces: Option<NotificationId>,
    ) -> Result<NotificationId, PlatformError> {
        let mut log = self.log.lock().unwrap();
        log.shown.push((notification.clone(), replaces));
        Ok(replaces.unwrap_or(NotificationId(u32::try_from(log.shown.len()).unwrap())))
    }

    async fn close(&self, id: NotificationId) -> Result<(), PlatformError> {
        self.log.lock().unwrap().closed.push(id);
        Ok(())
    }

    async fn capabilities(&self) -> Result<NotificationCapabilities, PlatformError> {
        Ok(NotificationCapabilities {
            actions: true,
            ..NotificationCapabilities::default()
        })
    }

    async fn next_response(&self) -> Result<NotificationResponse, PlatformError> {
        std::future::pending().await
    }
}

/// A directory for the image cache, removed when dropped.
struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Setup {
    session: Session<DuplexStream>,
    phone: RawPeer,
    service: NotificationService<FakeSink, MemoryOutboxStore>,
    desktop: Arc<Mutex<Desktop>>,
    _images: TempDir,
}

/// A session whose phone reports notifications ready, and a service that
/// has sent its sync request (seq 4), read by the phone.
async fn synced_setup(batch: Vec<PhoneNotification>) -> Setup {
    let (mut session, mut phone) = connected_to(notifying_phone_hello(), fast()).await;
    let images = TempDir(std::env::temp_dir().join(format!(
        "l2l-service-{}-{}",
        std::process::id(),
        rand()
    )));
    let desktop = Arc::new(Mutex::new(Desktop::default()));
    let mut service = NotificationService::new(
        FakeSink {
            log: desktop.clone(),
        },
        Outbox::new(MemoryOutboxStore::default()),
        ImageCache::new(&images.0, 1 << 20).unwrap(),
        NotificationSettings::default(),
        Mirror::default(),
    );
    service.session_started(&mut session).await.unwrap();

    phone
        .send(&envelope(2, all_ready(&["notifications"])))
        .await
        .unwrap();
    let status = next(&mut session).await;
    assert!(matches!(
        service.handle_event(&mut session, status).await.unwrap(),
        Some(SessionEvent::FeatureStatus(_))
    ));
    let sync = phone.next().await.unwrap().unwrap();
    assert!(sync.expects_reply);
    assert!(
        matches!(&sync.payload, Some(Payload::NotificationSyncRequest(r)) if r.since.is_none())
    );

    let snapshot = NotificationSyncBatch {
        reset: true,
        notifications: batch,
        cursor: Some(SyncCursor {
            epoch: 5,
            revision: 1,
        }),
        ..NotificationSyncBatch::default()
    };
    phone
        .send(&answer(
            3,
            sync.seq,
            Payload::NotificationSyncBatch(snapshot),
        ))
        .await
        .unwrap();
    let reply = next(&mut session).await;
    assert!(
        service
            .handle_event(&mut session, reply)
            .await
            .unwrap()
            .is_none()
    );
    Setup {
        session,
        phone,
        service,
        desktop,
        _images: images,
    }
}

fn rand() -> u64 {
    use std::hash::{BuildHasher, RandomState};
    RandomState::new().hash_one(std::time::SystemTime::now())
}

async fn next(session: &mut Session<DuplexStream>) -> SessionEvent {
    session.next_event().await.unwrap().expect("an event")
}

fn note(key: &str, revision: u64) -> PhoneNotification {
    PhoneNotification {
        key: key.into(),
        revision,
        app_name: "Chat".into(),
        title: format!("from {key}"),
        importance: NotificationImportance::High.into(),
        dismissible: true,
        ..PhoneNotification::default()
    }
}

fn posted(notification: PhoneNotification, alert: bool) -> Payload {
    Payload::NotificationPosted(NotificationPosted {
        notification: Some(notification),
        alert,
    })
}

#[tokio::test]
async fn syncs_then_shows_alerting_notifications_and_closes_removed_ones() {
    let mut s = synced_setup(vec![note("old", 1)]).await;
    assert_eq!(s.service.notifications().count(), 1);
    assert!(
        s.desktop.lock().unwrap().shown.is_empty(),
        "synced notifications do not alert"
    );

    s.phone
        .send(&envelope(4, posted(note("new", 2), true)))
        .await
        .unwrap();
    let event = next(&mut s.session).await;
    assert!(
        s.service
            .handle_event(&mut s.session, event)
            .await
            .unwrap()
            .is_none()
    );
    let id = {
        let desktop = s.desktop.lock().unwrap();
        assert_eq!(desktop.shown.len(), 1);
        assert_eq!(desktop.shown[0].0.summary, "from new");
        NotificationId(1)
    };

    let removed = NotificationRemoved {
        key: "new".into(),
        revision: 3,
        ..NotificationRemoved::default()
    };
    s.phone
        .send(&envelope(5, Payload::NotificationRemoved(removed)))
        .await
        .unwrap();
    let event = next(&mut s.session).await;
    s.service.handle_event(&mut s.session, event).await.unwrap();
    assert_eq!(s.desktop.lock().unwrap().closed, [id]);
    assert_eq!(s.service.notifications().count(), 1);
    assert_eq!(s.service.cursor().revision, 3);
}

#[tokio::test]
async fn the_users_dismissal_goes_to_the_phone_until_it_answers() {
    let mut s = synced_setup(vec![]).await;
    s.phone
        .send(&envelope(4, posted(note("chat", 2), true)))
        .await
        .unwrap();
    let event = next(&mut s.session).await;
    s.service.handle_event(&mut s.session, event).await.unwrap();

    // A banner the desktop closed itself asks nothing of the phone.
    let closed_by_app = NotificationResponse::Closed {
        id: NotificationId(1),
        reason: CloseReason::ClosedByApp,
    };
    assert!(
        s.service
            .handle_response(Some(&mut s.session), closed_by_app)
            .await
            .unwrap()
            .is_none()
    );
    assert!(s.service.pending().is_empty());

    s.phone
        .send(&envelope(5, posted(note("chat", 3), true)))
        .await
        .unwrap();
    let event = next(&mut s.session).await;
    s.service.handle_event(&mut s.session, event).await.unwrap();
    let dismissed = NotificationResponse::Closed {
        id: NotificationId(2),
        reason: CloseReason::Dismissed,
    };
    s.service
        .handle_response(Some(&mut s.session), dismissed)
        .await
        .unwrap();
    assert_eq!(
        s.service.pending().len(),
        1,
        "pending until the phone answers"
    );

    let request: Envelope = s.phone.next().await.unwrap().unwrap();
    let Some(Payload::NotificationDismissRequest(dismiss)) = &request.payload else {
        panic!("expected a dismiss request, got {request:?}");
    };
    assert_eq!(dismiss.key, "chat");
    assert_eq!(dismiss.idempotency_key.len(), 16);
    s.phone
        .send(&answer(6, request.seq, Payload::Ack(Ack {})))
        .await
        .unwrap();
    let reply = next(&mut s.session).await;
    assert!(
        s.service
            .handle_event(&mut s.session, reply)
            .await
            .unwrap()
            .is_none()
    );
    assert!(s.service.pending().is_empty());
}

#[tokio::test]
async fn live_events_before_the_sync_are_a_rule_break() {
    let (mut session, mut phone) = connected_to(notifying_phone_hello(), fast()).await;
    let images = TempDir(std::env::temp_dir().join(format!("l2l-service-early-{}", rand())));
    let mut service = NotificationService::new(
        FakeSink {
            log: Arc::default(),
        },
        Outbox::new(MemoryOutboxStore::default()),
        ImageCache::new(&images.0, 1 << 20).unwrap(),
        NotificationSettings::default(),
        Mirror::default(),
    );
    phone
        .send(&envelope(2, posted(note("early", 1), true)))
        .await
        .unwrap();
    let event = next(&mut session).await;
    assert!(matches!(
        service.handle_event(&mut session, event).await,
        Err(ServiceError::Mirror(_))
    ));
}
