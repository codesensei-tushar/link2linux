//! The core's notifications service behind [`NotificationsDriver`].

use std::path::PathBuf;
use std::sync::Arc;

use link2linux_core::device::DeviceInfo;
use link2linux_core::notifications::{
    Answer, Change, ImageCache, Mirror, NotificationService, NotificationSettings, ServiceError,
};
use link2linux_core::outbox::{MemoryOutboxStore, Outbox};
use link2linux_core::session::SessionEvent;
use link2linux_platform::NotificationResponse;

use super::{
    BoxFuture, DriverError, DriverFactory, NotificationsDriver, PhoneSession, Presenter, SharedSink,
};

/// The most image bytes kept on disk for one phone.
const IMAGE_CACHE_BYTES: u64 = 64 * 1024 * 1024;

/// Makes a [`ServiceDriver`] for each phone, keeping its images under
/// `images_dir`.
pub struct ServiceFactory {
    presenter: Arc<dyn Presenter>,
    images_dir: PathBuf,
}

impl ServiceFactory {
    /// A factory whose services show banners through `presenter`.
    #[must_use]
    pub fn new(presenter: Arc<dyn Presenter>, images_dir: PathBuf) -> Self {
        Self {
            presenter,
            images_dir,
        }
    }
}

impl ServiceFactory {
    /// Where the pictures of phone `device_id` are kept. Device ids come
    /// from the phone, so only what is safe in a file name is kept.
    fn dir_of(&self, device_id: &str) -> PathBuf {
        let name: String = device_id
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        self.images_dir.join(name)
    }
}

impl DriverFactory for ServiceFactory {
    fn create(&self, device: &DeviceInfo) -> Box<dyn NotificationsDriver> {
        let dir = self.dir_of(&device.id);
        let images = ImageCache::new(&dir, IMAGE_CACHE_BYTES).unwrap_or_else(|error| {
            tracing::warn!(%error, dir = %dir.display(), "no image cache on disk; using a temporary one");
            ImageCache::new(std::env::temp_dir().join("link2linux-images"), IMAGE_CACHE_BYTES)
                .expect("the temporary directory is writable")
        });
        Box::new(ServiceDriver::new(self.presenter.clone(), images))
    }

    fn image_path(&self, device_id: &str, sha256: &[u8]) -> Option<PathBuf> {
        ImageCache::new(self.dir_of(device_id), IMAGE_CACHE_BYTES)
            .ok()?
            .get(sha256)
    }
}

/// One phone's [`NotificationService`], with the memory-only outbox and
/// mirror of this version.
pub struct ServiceDriver {
    service: NotificationService<SharedSink, MemoryOutboxStore>,
}

impl ServiceDriver {
    /// A service that shows banners through `presenter` and keeps images in
    /// `images`.
    #[must_use]
    pub fn new(presenter: Arc<dyn Presenter>, images: ImageCache) -> Self {
        Self {
            service: NotificationService::new(
                SharedSink(presenter),
                Outbox::new(MemoryOutboxStore::default()),
                images,
                NotificationSettings::default(),
                Mirror::default(),
            ),
        }
    }
}

fn failed(error: ServiceError) -> DriverError {
    match error {
        ServiceError::Mirror(_) | ServiceError::UnexpectedReply => {
            DriverError::PhoneBrokeRules(error.to_string())
        }
        other => DriverError::Failed(other.to_string()),
    }
}

impl NotificationsDriver for ServiceDriver {
    fn session_started<'a>(
        &'a mut self,
        session: &'a mut PhoneSession,
    ) -> BoxFuture<'a, Result<(), DriverError>> {
        Box::pin(async move { self.service.session_started(session).await.map_err(failed) })
    }

    fn handle_event<'a>(
        &'a mut self,
        session: &'a mut PhoneSession,
        event: SessionEvent,
    ) -> BoxFuture<'a, Result<Option<SessionEvent>, DriverError>> {
        Box::pin(async move {
            self.service
                .handle_event(session, event)
                .await
                .map_err(failed)
        })
    }

    fn handle_response<'a>(
        &'a mut self,
        session: Option<&'a mut PhoneSession>,
        response: NotificationResponse,
    ) -> BoxFuture<'a, Result<Option<Answer>, DriverError>> {
        Box::pin(async move {
            self.service
                .handle_response(session, response)
                .await
                .map_err(failed)
        })
    }

    fn session_ended(&mut self) {
        self.service.session_ended();
    }

    fn subscribe(&self) -> tokio::sync::broadcast::Receiver<Change> {
        self.service.subscribe()
    }

    fn dismiss<'a>(
        &'a mut self,
        session: Option<&'a mut PhoneSession>,
        key: &'a str,
    ) -> BoxFuture<'a, Result<(), DriverError>> {
        Box::pin(async move { self.service.dismiss(session, key).await.map_err(failed) })
    }

    fn run_action<'a>(
        &'a mut self,
        session: Option<&'a mut PhoneSession>,
        key: &'a str,
        action_id: &'a str,
        reply: Option<&'a str>,
    ) -> BoxFuture<'a, Result<(), DriverError>> {
        Box::pin(async move {
            self.service
                .run_action(session, key, action_id, reply)
                .await
                .map_err(failed)
        })
    }
}
