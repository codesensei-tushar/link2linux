//! Whether the user's session is locked, as systemd-logind reports it in
//! `LockedHint` on the system bus.
//!
//! GNOME and KDE Plasma set the hint while their lock screen is up. A
//! screen locker that does not tell logind leaves it unset, and the session
//! then always counts as unlocked. Callers treat the session as locked
//! while logind does not answer.

use std::time::Duration;

use futures_util::StreamExt;
use zbus::Connection;
use zbus::proxy::PropertyStream;
use zbus::zvariant::OwnedObjectPath;

use crate::PlatformError;

const SERVICE: &str = "systemd-logind";
const CALL_TIMEOUT: Duration = Duration::from_secs(5);

#[zbus::proxy(
    interface = "org.freedesktop.login1.Manager",
    default_service = "org.freedesktop.login1",
    default_path = "/org/freedesktop/login1"
)]
trait Manager {
    fn get_session(&self, session_id: &str) -> zbus::Result<OwnedObjectPath>;
}

#[zbus::proxy(
    interface = "org.freedesktop.login1.Session",
    default_service = "org.freedesktop.login1"
)]
trait Session {
    #[zbus(property)]
    fn locked_hint(&self) -> zbus::Result<bool>;
}

/// The lock state of the user's session, followed as it changes.
pub struct SessionLock {
    session: SessionProxy<'static>,
    changes: PropertyStream<'static, bool>,
    last: Option<bool>,
}

impl std::fmt::Debug for SessionLock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionLock").finish_non_exhaustive()
    }
}

impl SessionLock {
    /// Follows the session logind calls `auto`: the app's own, or the
    /// user's graphical session when the app was started outside one, as
    /// desktop launchers do.
    ///
    /// # Errors
    ///
    /// Fails without a system bus, without logind, or without a session.
    pub async fn connect() -> Result<Self, PlatformError> {
        let connection = Connection::system()
            .await
            .map_err(|e| unavailable(format!("no system D-Bus: {e}")))?;
        Self::on(&connection).await
    }

    /// Follows the session on `connection`, for tests with a stand-in for
    /// logind.
    ///
    /// # Errors
    ///
    /// Fails when logind does not answer or knows no session.
    pub async fn on(connection: &Connection) -> Result<Self, PlatformError> {
        tokio::time::timeout(CALL_TIMEOUT, async {
            let manager = ManagerProxy::new(connection).await?;
            let path = manager.get_session("auto").await?;
            let session = SessionProxy::builder(connection)
                .path(path)?
                .build()
                .await?;
            let changes = session.receive_locked_hint_changed().await;
            Ok(Self {
                session,
                changes,
                last: None,
            })
        })
        .await
        .map_err(|_| unavailable("logind did not answer".to_owned()))?
        .map_err(|e: zbus::Error| unavailable(e.to_string()))
    }

    /// Whether the session is locked now.
    ///
    /// # Errors
    ///
    /// Fails when logind does not answer.
    pub async fn is_locked(&mut self) -> Result<bool, PlatformError> {
        let locked = tokio::time::timeout(CALL_TIMEOUT, self.session.locked_hint())
            .await
            .map_err(|_| unavailable("logind did not answer".to_owned()))?
            .map_err(|e| unavailable(e.to_string()))?;
        self.last = Some(locked);
        Ok(locked)
    }

    /// Waits until the session is locked or unlocked, and says which. `None`
    /// means logind went away.
    pub async fn changed(&mut self) -> Option<bool> {
        loop {
            let change = self.changes.next().await?;
            let Ok(locked) = change.get().await else {
                continue;
            };
            if self.last.replace(locked) != Some(locked) {
                return Some(locked);
            }
        }
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
    use tokio::net::UnixStream;
    use zbus::connection::Builder;
    use zbus::zvariant::ObjectPath;

    use super::*;

    const SESSION: &str = "/org/freedesktop/login1/session/_32";

    struct MockManager;

    #[zbus::interface(name = "org.freedesktop.login1.Manager")]
    impl MockManager {
        fn get_session(&self, session_id: &str) -> zbus::fdo::Result<OwnedObjectPath> {
            assert_eq!(session_id, "auto");
            Ok(ObjectPath::try_from(SESSION).unwrap().into())
        }
    }

    struct MockSession {
        locked: bool,
    }

    #[zbus::interface(name = "org.freedesktop.login1.Session")]
    impl MockSession {
        #[zbus(property)]
        fn locked_hint(&self) -> bool {
            self.locked
        }
    }

    #[tokio::test]
    async fn follows_the_locked_hint() {
        let (a, b) = UnixStream::pair().unwrap();
        let server = Builder::unix_stream(a)
            .server(zbus::Guid::generate())
            .unwrap()
            .p2p()
            .serve_at("/org/freedesktop/login1", MockManager)
            .unwrap()
            .serve_at(SESSION, MockSession { locked: false })
            .unwrap()
            .build();
        let client = Builder::unix_stream(b).p2p().build();
        let (server, client) = tokio::try_join!(server, client).unwrap();

        let mut lock = SessionLock::on(&client).await.unwrap();
        assert!(!lock.is_locked().await.unwrap());

        let session = server
            .object_server()
            .interface::<_, MockSession>(SESSION)
            .await
            .unwrap();
        for locked in [true, false] {
            session.get_mut().await.locked = locked;
            session
                .get()
                .await
                .locked_hint_changed(session.signal_emitter())
                .await
                .unwrap();
            let change = tokio::time::timeout(CALL_TIMEOUT, lock.changed()).await;
            assert_eq!(change.unwrap(), Some(locked));
        }
    }
}
