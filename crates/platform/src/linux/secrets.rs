//! The Secret Service keyring (`org.freedesktop.secrets`), which GNOME
//! Keyring, KWallet and KeePassXC provide.
//!
//! Every call opens its own session with the service, so a keyring that
//! starts or unlocks later is picked up, and gives up after a timeout: a
//! keyring that hangs must not hang the app.

use std::future::Future;
use std::time::Duration;

use oo7::dbus::{Collection, Service};
use zeroize::Zeroizing;

use crate::{PlatformError, SecretStore};

/// The service named in errors.
const SERVICE: &str = "Secret Service";

/// How long one call may take before the keyring counts as unavailable.
/// Starting the service on demand fits in it.
const CALL_TIMEOUT: Duration = Duration::from_secs(10);

/// How long the user has to answer the keyring's prompt to unlock it (or
/// to create it, on a first login). The prompt itself stays on screen; an
/// answer that comes later is not used.
const PROMPT_TIMEOUT: Duration = Duration::from_secs(60);

/// Secrets in the Secret Service's default collection (the login keyring),
/// found by two attributes: `application`, this app's id, and `name`.
///
/// The session with the service is always encrypted. Any program of the
/// same user can watch the session bus, so a service that offers only
/// plain sessions counts as unavailable rather than seeing secrets in the
/// clear.
#[derive(Debug, Clone)]
pub struct SecretServiceStore {
    application: String,
}

impl SecretServiceStore {
    /// A store for the app with id `application`, such as
    /// `dev.link2linux.desktop`.
    #[must_use]
    pub fn new(application: impl Into<String>) -> Self {
        Self {
            application: application.into(),
        }
    }

    fn attributes<'a>(&'a self, name: &'a str) -> [(&'a str, &'a str); 2] {
        [("application", &self.application), ("name", name)]
    }
}

impl SecretStore for SecretServiceStore {
    async fn read(&self, name: &str) -> Result<Option<Zeroizing<Vec<u8>>>, PlatformError> {
        let keyring = Keyring::open().await?;
        let items = within(
            CALL_TIMEOUT,
            keyring.collection.search_items(&self.attributes(name)),
        )
        .await?;
        let Some(item) = items.first() else {
            return Ok(None);
        };
        if within(CALL_TIMEOUT, item.is_locked()).await? {
            within(PROMPT_TIMEOUT, item.unlock(None)).await?;
        }
        let secret = within(CALL_TIMEOUT, item.secret()).await?;
        Ok(Some(Zeroizing::new(secret.as_bytes().to_vec())))
    }

    async fn write(&self, name: &str, label: &str, secret: &[u8]) -> Result<(), PlatformError> {
        let keyring = Keyring::open().await?;
        within(
            CALL_TIMEOUT,
            keyring.collection.create_item(
                label,
                &self.attributes(name),
                oo7::Secret::blob(secret),
                true,
                None,
            ),
        )
        .await?;
        Ok(())
    }
}

/// The unlocked default collection, and the service whose encrypted
/// session it uses. Dropping the service closes the session.
struct Keyring {
    _service: Service,
    collection: Collection,
}

impl Keyring {
    async fn open() -> Result<Self, PlatformError> {
        let service = within(CALL_TIMEOUT, Service::encrypted()).await?;
        let collection = within(PROMPT_TIMEOUT, service.default_collection()).await?;
        if within(CALL_TIMEOUT, collection.is_locked()).await? {
            within(PROMPT_TIMEOUT, collection.unlock(None)).await?;
        }
        Ok(Self {
            _service: service,
            collection,
        })
    }
}

/// Runs a call to the service, which counts as unavailable when the call
/// fails or takes longer than `limit`.
async fn within<T>(
    limit: Duration,
    call: impl Future<Output = Result<T, oo7::dbus::Error>>,
) -> Result<T, PlatformError> {
    match tokio::time::timeout(limit, call).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err(unavailable(error.to_string())),
        Err(_) => Err(unavailable(format!(
            "no answer within {} seconds",
            limit.as_secs()
        ))),
    }
}

fn unavailable(reason: String) -> PlatformError {
    PlatformError::Unavailable {
        service: SERVICE,
        reason,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn items_are_found_by_app_and_name() {
        let store = SecretServiceStore::new("dev.link2linux.desktop");
        assert_eq!(
            store.attributes("device-key"),
            [
                ("application", "dev.link2linux.desktop"),
                ("name", "device-key")
            ]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_service_that_never_answers_is_unavailable() {
        let never = std::future::pending::<Result<(), oo7::dbus::Error>>();
        let result = within(CALL_TIMEOUT, never).await;
        assert!(
            matches!(
                &result,
                Err(PlatformError::Unavailable { service: SERVICE, reason })
                    if reason.contains("10 seconds")
            ),
            "{result:?}"
        );
    }
}
