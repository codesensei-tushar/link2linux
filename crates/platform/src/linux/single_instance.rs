//! One desktop app per login session.
//!
//! The first app owns the name `dev.link2linux.desktop` on the session bus.
//! Starting the app again shows the first one's window and exits, as other
//! desktop apps do, instead of starting a second app that would dial the
//! same phones and keep replacing the first one's sessions. The bus drops
//! the name when the app exits, or crashes, so a later start finds it free.

use std::time::Duration;

use zbus::Connection;
use zbus::fdo::{RequestNameFlags, RequestNameReply};

use crate::PlatformError;

const SERVICE: &str = "the session bus";
/// The app's id, which a Flatpak app may own without asking for it.
const NAME: &str = "dev.link2linux.desktop";
const PATH: &str = "/dev/link2linux/desktop";
const CALL_TIMEOUT: Duration = Duration::from_secs(5);

#[zbus::proxy(
    interface = "dev.link2linux.Desktop1",
    default_service = "dev.link2linux.desktop",
    default_path = "/dev/link2linux/desktop"
)]
trait Desktop {
    fn show(&self) -> zbus::Result<()>;
}

/// The running app's side of `dev.link2linux.Desktop1`.
struct Window {
    show: Box<dyn Fn() + Send + Sync>,
}

#[zbus::interface(name = "dev.link2linux.Desktop1")]
impl Window {
    /// Brings the window to the front.
    fn show(&self) {
        (self.show)();
    }
}

/// What starting the app found.
#[derive(Debug)]
pub enum Instance {
    /// No other app runs in this session. Serve the window through
    /// [`FirstInstance::serve`] once it exists.
    First(FirstInstance),
    /// The app already runs, and was asked to show its window.
    AlreadyRunning,
}

/// The app's name on the session bus, held as long as this lives.
#[derive(Debug)]
pub struct FirstInstance {
    connection: Connection,
}

impl FirstInstance {
    /// Calls `show` whenever the app is started again.
    ///
    /// # Errors
    ///
    /// Fails when the object cannot be exported on the bus.
    pub async fn serve(
        &self,
        show: impl Fn() + Send + Sync + 'static,
    ) -> Result<(), PlatformError> {
        let window = Window {
            show: Box::new(show),
        };
        self.connection
            .object_server()
            .at(PATH, window)
            .await
            .map(drop)
            .map_err(|e| unavailable(format!("could not export the window: {e}")))
    }
}

/// Claims the app's name on the session bus.
///
/// # Errors
///
/// Fails without a session bus. The app then starts without knowing
/// whether another one runs.
pub async fn claim() -> Result<Instance, PlatformError> {
    let connection = Connection::session()
        .await
        .map_err(|e| unavailable(format!("no session D-Bus: {e}")))?;
    claim_on(connection).await
}

/// Claims the name on `connection`, for tests with a private bus.
///
/// # Errors
///
/// Fails when the bus does not answer the request for the name.
pub async fn claim_on(connection: Connection) -> Result<Instance, PlatformError> {
    let requested = tokio::time::timeout(
        CALL_TIMEOUT,
        connection.request_name_with_flags(NAME, RequestNameFlags::DoNotQueue.into()),
    )
    .await
    .map_err(|_| unavailable("the request for the app's name timed out".into()))?;
    match requested {
        Ok(RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner) => {
            return Ok(Instance::First(FirstInstance { connection }));
        }
        // zbus reports the bus's `Exists` answer as `NameTaken`.
        Ok(RequestNameReply::Exists | RequestNameReply::InQueue) | Err(zbus::Error::NameTaken) => {}
        Err(e) => {
            return Err(unavailable(format!(
                "could not request the app's name: {e}"
            )));
        }
    }
    // An app that is still starting serves no window yet, so the call goes
    // unanswered until it times out; that app shows its window anyway.
    let shown = tokio::time::timeout(CALL_TIMEOUT, async {
        DesktopProxy::new(&connection).await?.show().await
    })
    .await;
    match shown {
        Ok(Ok(())) => tracing::debug!("showed the running app's window"),
        Ok(Err(error)) => tracing::debug!(%error, "the running app did not show its window"),
        Err(_) => tracing::debug!("the running app did not answer"),
    }
    Ok(Instance::AlreadyRunning)
}

fn unavailable(reason: String) -> PlatformError {
    PlatformError::Unavailable {
        service: SERVICE,
        reason,
    }
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader};
    use std::path::PathBuf;
    use std::process::{Child, Command, Stdio};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    /// A bus of the test's own, so the tests never touch the user's session.
    struct PrivateBus {
        daemon: Child,
        config: PathBuf,
        address: String,
    }

    impl PrivateBus {
        /// Starts `dbus-daemon`, or returns `None` where it is not installed.
        fn start(test: &str) -> Option<Self> {
            let config =
                std::env::temp_dir().join(format!("l2l-bus-{}-{test}.conf", std::process::id()));
            let socket_dir = std::env::temp_dir();
            std::fs::write(
                &config,
                format!(
                    "<busconfig><type>session</type>\
                     <listen>unix:tmpdir={}</listen>\
                     <policy context=\"default\"><allow send_destination=\"*\"/>\
                     <allow receive_sender=\"*\"/><allow own=\"*\"/></policy>\
                     </busconfig>",
                    socket_dir.display()
                ),
            )
            .expect("write the bus config");
            let mut daemon = match Command::new("dbus-daemon")
                .arg(format!("--config-file={}", config.display()))
                .args(["--nofork", "--print-address=1"])
                .stdout(Stdio::piped())
                .spawn()
            {
                Ok(daemon) => daemon,
                Err(error) => {
                    eprintln!("skipped: no dbus-daemon ({error})");
                    let _ = std::fs::remove_file(&config);
                    return None;
                }
            };
            let mut address = String::new();
            BufReader::new(daemon.stdout.take().expect("piped"))
                .read_line(&mut address)
                .expect("read the bus address");
            Some(Self {
                daemon,
                config,
                address: address.trim().to_owned(),
            })
        }

        /// Fails rather than hangs when the bus never answers, as it does
        /// when its policy drops the replies.
        async fn connect(&self) -> Connection {
            let build = zbus::connection::Builder::address(self.address.as_str())
                .expect("a bus address")
                .build();
            tokio::time::timeout(CALL_TIMEOUT, build)
                .await
                .expect("the private bus did not answer")
                .expect("connect to the private bus")
        }
    }

    impl Drop for PrivateBus {
        fn drop(&mut self) {
            let _ = self.daemon.kill();
            let _ = self.daemon.wait();
            let _ = std::fs::remove_file(&self.config);
        }
    }

    #[tokio::test]
    async fn a_second_start_shows_the_first_apps_window_instead() {
        let Some(bus) = PrivateBus::start("second") else {
            return;
        };
        let shown = Arc::new(AtomicUsize::new(0));
        let Instance::First(first) = claim_on(bus.connect().await).await.unwrap() else {
            panic!("the first app found another one");
        };
        let count = shown.clone();
        first
            .serve(move || {
                count.fetch_add(1, Ordering::SeqCst);
            })
            .await
            .unwrap();

        for _ in 0..2 {
            let again = claim_on(bus.connect().await).await.unwrap();
            assert!(matches!(again, Instance::AlreadyRunning), "{again:?}");
        }
        assert_eq!(shown.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn an_app_still_starting_counts_as_running() {
        let Some(bus) = PrivateBus::start("starting") else {
            return;
        };
        // Owns the name but serves no window yet.
        let _first = claim_on(bus.connect().await).await.unwrap();

        let again = claim_on(bus.connect().await).await.unwrap();
        assert!(matches!(again, Instance::AlreadyRunning), "{again:?}");
    }

    #[tokio::test]
    async fn the_name_is_free_again_once_the_app_exits() {
        let Some(bus) = PrivateBus::start("exits") else {
            return;
        };
        let first = claim_on(bus.connect().await).await.unwrap();
        assert!(matches!(first, Instance::First(_)));
        drop(first);

        // The bus drops the name once it sees the connection close.
        let mut next = claim_on(bus.connect().await).await.unwrap();
        for _ in 0..50 {
            if matches!(next, Instance::First(_)) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
            next = claim_on(bus.connect().await).await.unwrap();
        }
        assert!(matches!(next, Instance::First(_)), "{next:?}");
    }
}
