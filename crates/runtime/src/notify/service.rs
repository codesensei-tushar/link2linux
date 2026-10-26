//! The core's notifications service behind [`NotificationsDriver`].

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use link2linux_core::device::DeviceInfo;
use link2linux_core::notifications::{
    Answer, CachedImage, Change, ImageCache, Mirror, NotificationService, NotificationSettings,
    ServiceError,
};
use link2linux_core::outbox::{MemoryOutboxStore, Outbox};
use link2linux_core::session::SessionEvent;
use link2linux_platform::NotificationResponse;

use super::{
    BoxFuture, DriverError, DriverFactory, NotificationsDriver, PhoneSession, Presenter, SharedSink,
};

/// The most image bytes kept for one phone.
const IMAGE_CACHE_BYTES: u64 = 64 * 1024 * 1024;

/// Makes a [`ServiceDriver`] for each phone, keeping its images as files
/// under `images_dir`, or in memory without one.
pub struct ServiceFactory {
    presenter: Arc<dyn Presenter>,
    images_dir: Option<PathBuf>,
    /// Each phone's images, shared with its service.
    caches: Mutex<HashMap<String, ImageCache>>,
}

impl ServiceFactory {
    /// A factory whose services show banners through `presenter`, and keep
    /// pictures in `images_dir`, or only in memory when it is `None`.
    #[must_use]
    pub fn new(presenter: Arc<dyn Presenter>, images_dir: Option<PathBuf>) -> Self {
        Self {
            presenter,
            images_dir,
            caches: Mutex::default(),
        }
    }

    /// The image cache of phone `device_id`, made on first use.
    fn cache_of(&self, device_id: &str) -> ImageCache {
        let mut caches = crate::link::lock(&self.caches);
        let cache = caches.entry(device_id.to_owned()).or_insert_with(|| {
            let Some(images_dir) = &self.images_dir else {
                return ImageCache::in_memory(IMAGE_CACHE_BYTES);
            };
            let dir = dir_of(images_dir, device_id);
            ImageCache::new(&dir, IMAGE_CACHE_BYTES).unwrap_or_else(|error| {
                tracing::warn!(%error, dir = %dir.display(), "keeping a phone's images in memory");
                ImageCache::in_memory(IMAGE_CACHE_BYTES)
            })
        });
        cache.clone()
    }
}

/// Where the pictures of phone `device_id` are kept. Device ids come from
/// the phone, so only what is safe in a file name is kept.
fn dir_of(images_dir: &std::path::Path, device_id: &str) -> PathBuf {
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
    images_dir.join(name)
}

impl DriverFactory for ServiceFactory {
    fn create(&self, device: &DeviceInfo) -> Box<dyn NotificationsDriver> {
        let images = self.cache_of(&device.id);
        Box::new(ServiceDriver::new(self.presenter.clone(), images))
    }

    fn image(&self, device_id: &str, sha256: &[u8]) -> Option<CachedImage> {
        let cache = crate::link::lock(&self.caches).get(device_id).cloned()?;
        cache.read(sha256)
    }

    fn forget(&self, device_id: &str) {
        // Every paired phone got its cache when its driver was made.
        let cache = crate::link::lock(&self.caches).remove(device_id);
        if let Some(Err(error)) = cache.map(|cache| cache.clear()) {
            tracing::warn!(%error, "could not delete an unpaired phone's images");
        }
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
