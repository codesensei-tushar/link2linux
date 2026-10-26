//! Files in the XDG user directories, handed to the desktop to open or
//! show.
//!
//! [`XdgFileStore`] opens a file through the desktop portal
//! (`org.freedesktop.portal.OpenURI`), which starts the user's default
//! application. The portal gets the open file rather than its name, and
//! applies the desktop's own rules, such as asking first before it opens a
//! file that could run as a program. Showing a file in its folder asks the
//! file manager to select it (`org.freedesktop.FileManager1`, which the
//! common file managers implement). Without either service, `xdg-open`
//! opens the file, or the folder.

use std::collections::HashMap;
use std::env;
use std::fmt::Write as _;
use std::fs::{self, File};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use tokio::sync::OnceCell;
use zbus::Connection;
use zbus::zvariant::{Fd, OwnedObjectPath, Value};

use crate::{FileStore, PlatformError};

/// How long the desktop may take to answer, starting a file manager if it
/// has to.
const CALL_TIMEOUT: Duration = Duration::from_secs(10);

#[zbus::proxy(
    interface = "org.freedesktop.portal.OpenURI",
    default_service = "org.freedesktop.portal.Desktop",
    default_path = "/org/freedesktop/portal/desktop"
)]
trait OpenUri {
    fn open_file(
        &self,
        parent_window: &str,
        fd: Fd<'_>,
        options: HashMap<&str, Value<'_>>,
    ) -> zbus::Result<OwnedObjectPath>;
}

#[zbus::proxy(
    interface = "org.freedesktop.FileManager1",
    default_service = "org.freedesktop.FileManager1",
    default_path = "/org/freedesktop/FileManager1"
)]
trait FileManager1 {
    fn show_items(&self, uris: &[&str], startup_id: &str) -> zbus::Result<()>;
}

/// Files in the XDG user directories, opened and shown through the
/// desktop.
#[derive(Debug)]
pub struct XdgFileStore {
    /// The session bus, connected on first use, or `None` without one.
    bus: OnceCell<Option<Connection>>,
    /// What opens a path when the desktop services cannot.
    opener: PathBuf,
}

impl Default for XdgFileStore {
    fn default() -> Self {
        Self {
            bus: OnceCell::new(),
            opener: PathBuf::from("xdg-open"),
        }
    }
}

impl XdgFileStore {
    /// A store that reaches the desktop over the session bus, once it
    /// needs to.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    async fn bus(&self) -> Option<&Connection> {
        self.bus
            .get_or_init(|| async {
                Connection::session()
                    .await
                    .inspect_err(
                        |error| tracing::debug!(%error, "no session bus for opening files"),
                    )
                    .ok()
            })
            .await
            .as_ref()
    }
}

impl FileStore for XdgFileStore {
    /// `XDG_DOWNLOAD_DIR` from the environment or from
    /// `$XDG_CONFIG_HOME/user-dirs.dirs`, falling back to `~/Downloads`.
    fn downloads_dir(&self) -> Result<PathBuf, PlatformError> {
        let home = env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or(PlatformError::Unavailable {
                service: "XDG user directories",
                reason: "HOME is not set".into(),
            })?;
        if let Some(dir) = env::var_os("XDG_DOWNLOAD_DIR") {
            return Ok(PathBuf::from(dir));
        }
        let config_home = env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"));
        let from_file = fs::read_to_string(config_home.join("user-dirs.dirs"))
            .ok()
            .and_then(|contents| download_dir_from_user_dirs(&contents, &home));
        Ok(from_file.unwrap_or_else(|| home.join("Downloads")))
    }

    async fn open(&self, path: &Path) -> Result<(), PlatformError> {
        let path = absolute(path)?;
        if let Some(bus) = self.bus().await {
            match open_with_portal(bus, path).await {
                Ok(()) => return Ok(()),
                Err(error) => tracing::debug!(%error, "the desktop portal did not open a file"),
            }
        }
        run(&self.opener, path)
    }

    async fn show_in_folder(&self, path: &Path) -> Result<(), PlatformError> {
        let path = absolute(path)?;
        if let Some(bus) = self.bus().await {
            match show_with_file_manager(bus, path).await {
                Ok(()) => return Ok(()),
                Err(error) => tracing::debug!(%error, "no file manager showed a file"),
            }
        }
        let folder = path.parent().unwrap_or(path);
        run(&self.opener, folder)
    }
}

/// `path`, which must be absolute: a relative one would depend on this
/// process's directory, and one starting with `-` would read as an option.
fn absolute(path: &Path) -> Result<&Path, PlatformError> {
    if path.is_absolute() {
        Ok(path)
    } else {
        Err(PlatformError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "the path is not absolute",
        )))
    }
}

async fn open_with_portal(bus: &Connection, path: &Path) -> Result<(), PlatformError> {
    let path = path.to_owned();
    let file = tokio::task::spawn_blocking(move || File::open(path))
        .await
        .map_err(std::io::Error::other)??;
    let portal = OpenUriProxy::new(bus).await.map_err(unavailable)?;
    let call = portal.open_file("", Fd::from(&file), HashMap::new());
    tokio::time::timeout(CALL_TIMEOUT, call)
        .await
        .map_err(|_| unavailable("no answer in time"))?
        .map_err(unavailable)?;
    Ok(())
}

async fn show_with_file_manager(bus: &Connection, path: &Path) -> Result<(), PlatformError> {
    let uri = file_uri(path);
    let manager = FileManager1Proxy::new(bus).await.map_err(unavailable)?;
    tokio::time::timeout(CALL_TIMEOUT, manager.show_items(&[&uri], ""))
        .await
        .map_err(|_| unavailable("no answer in time"))?
        .map_err(unavailable)
}

fn unavailable(reason: impl ToString) -> PlatformError {
    PlatformError::Unavailable {
        service: "the desktop's file services",
        reason: reason.to_string(),
    }
}

/// Starts `program` on `target` without waiting for it: `xdg-open` may
/// wait for the application it starts.
fn run(program: &Path, target: &Path) -> Result<(), PlatformError> {
    let mut child = Command::new(program)
        .arg(target)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| PlatformError::Unavailable {
            service: "xdg-open",
            reason: error.to_string(),
        })?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// The `file://` URI of the absolute `path`, with every byte other than
/// unreserved characters and `/` percent-encoded.
fn file_uri(path: &Path) -> String {
    let mut uri = String::from("file://");
    for &byte in path.as_os_str().as_bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~/".contains(&byte) {
            uri.push(char::from(byte));
        } else {
            let _ = write!(uri, "%{byte:02X}");
        }
    }
    uri
}

/// Reads `XDG_DOWNLOAD_DIR` from the contents of a `user-dirs.dirs` file,
/// whose values look like `"$HOME/Downloads"` or `"/absolute/path"`.
fn download_dir_from_user_dirs(contents: &str, home: &Path) -> Option<PathBuf> {
    let value = contents.lines().find_map(|line| {
        line.trim()
            .strip_prefix("XDG_DOWNLOAD_DIR=")
            .map(|value| value.trim_matches('"'))
    })?;
    if let Some(rest) = value.strip_prefix("$HOME") {
        Some(home.join(rest.trim_start_matches('/')))
    } else if value.starts_with('/') {
        Some(PathBuf::from(value))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use std::io::Read;
    use std::os::fd::OwnedFd;
    use std::sync::{Arc, Mutex};

    use tokio::net::UnixStream;
    use zbus::connection::Builder;

    use super::*;

    #[test]
    fn reads_the_download_dir_from_user_dirs() {
        let home = Path::new("/home/ana");
        let contents = "# written by xdg-user-dirs-update\n\
                        XDG_DESKTOP_DIR=\"$HOME/Desktop\"\n\
                        XDG_DOWNLOAD_DIR=\"$HOME/Téléchargements\"\n";
        assert_eq!(
            download_dir_from_user_dirs(contents, home),
            Some(PathBuf::from("/home/ana/Téléchargements"))
        );
        assert_eq!(
            download_dir_from_user_dirs("XDG_DOWNLOAD_DIR=\"/data/dl\"", home),
            Some(PathBuf::from("/data/dl"))
        );
        assert_eq!(
            download_dir_from_user_dirs("XDG_DOWNLOAD_DIR=\"$HOME/\"", home),
            Some(PathBuf::from("/home/ana/"))
        );
        assert_eq!(
            download_dir_from_user_dirs("XDG_MUSIC_DIR=\"$HOME/Music\"", home),
            None
        );
    }

    #[test]
    fn uris_encode_everything_but_plain_characters() {
        assert_eq!(
            file_uri(Path::new("/home/ana/Downloads/photo (1).jpg")),
            "file:///home/ana/Downloads/photo%20%281%29.jpg"
        );
        assert_eq!(
            file_uri(Path::new("/tmp/é#?%")),
            "file:///tmp/%C3%A9%23%3F%25"
        );
    }

    /// What the stand-in desktop services were asked.
    #[derive(Debug, Default)]
    struct Seen {
        shown: Vec<Vec<String>>,
        opened: Vec<Vec<u8>>,
    }

    struct FakeFileManager(Arc<Mutex<Seen>>);

    #[zbus::interface(name = "org.freedesktop.FileManager1")]
    impl FakeFileManager {
        fn show_items(&self, uris: Vec<String>, _startup_id: &str) {
            self.0.lock().unwrap().shown.push(uris);
        }
    }

    struct FakePortal(Arc<Mutex<Seen>>);

    #[zbus::interface(name = "org.freedesktop.portal.OpenURI")]
    impl FakePortal {
        fn open_file(
            &self,
            _parent_window: &str,
            fd: zbus::zvariant::OwnedFd,
            _options: HashMap<String, zbus::zvariant::OwnedValue>,
        ) -> OwnedObjectPath {
            let mut contents = Vec::new();
            File::from(OwnedFd::from(fd))
                .read_to_end(&mut contents)
                .unwrap();
            self.0.lock().unwrap().opened.push(contents);
            OwnedObjectPath::try_from("/org/freedesktop/portal/desktop/request/1").unwrap()
        }
    }

    /// A directory for a test's files, removed when dropped.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let dir = env::temp_dir().join(format!("l2l-files-{name}-{}", std::process::id()));
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// A store whose session bus is a peer serving the stand-ins, and whose
    /// fallback is `opener`.
    async fn store(opener: &str) -> (XdgFileStore, Connection, Arc<Mutex<Seen>>) {
        let seen = Arc::new(Mutex::new(Seen::default()));
        let (a, b) = UnixStream::pair().unwrap();
        let server = Builder::unix_stream(a)
            .server(zbus::Guid::generate())
            .unwrap()
            .p2p()
            .serve_at(
                "/org/freedesktop/FileManager1",
                FakeFileManager(seen.clone()),
            )
            .unwrap()
            .serve_at("/org/freedesktop/portal/desktop", FakePortal(seen.clone()))
            .unwrap()
            .build();
        let client = Builder::unix_stream(b).p2p().build();
        let (server, client) = tokio::try_join!(server, client).unwrap();
        let store = XdgFileStore {
            bus: OnceCell::new_with(Some(Some(client))),
            opener: PathBuf::from(opener),
        };
        (store, server, seen)
    }

    #[tokio::test]
    async fn files_are_opened_through_the_portal_and_shown_by_the_file_manager() {
        let dir = TempDir::new("shown");
        let path = dir.0.join("note (1).txt");
        fs::write(&path, b"hello").unwrap();
        let (store, _server, seen) = store("/nonexistent/opener").await;

        store.open(&path).await.unwrap();
        store.show_in_folder(&path).await.unwrap();
        let seen = seen.lock().unwrap();
        assert_eq!(seen.opened, [b"hello".to_vec()]);
        assert_eq!(seen.shown, [vec![file_uri(&path)]]);
    }

    #[tokio::test]
    async fn without_desktop_services_the_opener_runs() {
        let store = XdgFileStore {
            bus: OnceCell::new_with(Some(None)),
            opener: PathBuf::from("true"),
        };
        let dir = TempDir::new("opener");
        let path = dir.0.join("a.txt");
        fs::write(&path, b"a").unwrap();
        store.open(&path).await.unwrap();
        store.show_in_folder(&path).await.unwrap();

        let missing = XdgFileStore {
            bus: OnceCell::new_with(Some(None)),
            opener: PathBuf::from("/nonexistent/opener"),
        };
        assert!(matches!(
            missing.open(&path).await,
            Err(PlatformError::Unavailable { .. })
        ));
        assert!(matches!(
            store.open(Path::new("relative.txt")).await,
            Err(PlatformError::Io(_))
        ));
    }
}
