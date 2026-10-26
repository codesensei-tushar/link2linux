//! The desktop clipboard as the shell hands it to the runtime: the
//! platform's while it works in the background, else the clipboard as the
//! app's window sees it ([`WindowClipboard`]), for desktop sessions that let
//! an app use the clipboard only while its window has focus, such as a
//! Wayland session without a clipboard protocol for background apps, or one
//! whose display server went away.
//!
//! GTK runs on the app's main thread, so each call is sent there and its
//! answer comes back through a channel. The runtime uses the window's
//! clipboard only while the window has focus.

use link2linux_platform::{Clipboard, ClipboardAccess, ClipboardText, PlatformError};
use tauri::AppHandle;
use tokio::sync::oneshot;

use gtk::gdk::{Atom, SELECTION_CLIPBOARD};

/// The type a password manager adds to mark a copy as secret, and the value
/// that does.
const SECRET_HINT: &str = "x-kde-passwordManagerHint";
const SECRET: &[u8] = b"secret";
/// Types that say the clipboard holds copied files, which are not shared.
const FILE_TYPES: [&str; 2] = ["text/uri-list", "x-special/gnome-copied-files"];
/// The text types a copy marked as secret is offered as.
const TEXT_TYPES: [&str; 5] = [
    "UTF8_STRING",
    "text/plain;charset=utf-8",
    "text/plain",
    "STRING",
    "TEXT",
];
/// What `set_with_data` reports a request for the mark with.
const HINT_INFO: u32 = 1;

/// The desktop clipboard: `background` while it can be used in the
/// background, else `window`. The platform's clipboard turns focus-only
/// when it loses its display server, so the choice is made at each use.
pub struct FallbackClipboard<B, W> {
    background: B,
    window: W,
}

impl<B: Clipboard, W: Clipboard> FallbackClipboard<B, W> {
    pub fn new(background: B, window: W) -> Self {
        Self { background, window }
    }

    fn in_background(&self) -> bool {
        self.background.access() == ClipboardAccess::Background
    }
}

impl<B: Clipboard, W: Clipboard> Clipboard for FallbackClipboard<B, W> {
    fn access(&self) -> ClipboardAccess {
        self.background.access()
    }

    async fn read(&self) -> Result<Option<ClipboardText>, PlatformError> {
        if self.in_background() {
            self.background.read().await
        } else {
            self.window.read().await
        }
    }

    async fn write(&self, text: &ClipboardText) -> Result<(), PlatformError> {
        if self.in_background() {
            self.background.write(text).await
        } else {
            self.window.write(text).await
        }
    }

    async fn changed(&self) -> Result<(), PlatformError> {
        // It fails once when the display server goes away, and then never
        // resolves, as for a clipboard that was focus-only from the start.
        self.background.changed().await
    }
}

/// The desktop clipboard as the app's window sees it.
pub struct WindowClipboard {
    app: AppHandle,
}

impl WindowClipboard {
    pub fn new(app: AppHandle) -> Self {
        Self { app }
    }

    /// Runs `job` on the main thread with the clipboard, and waits for the
    /// answer it sends.
    async fn on_main<T: Send + 'static>(
        &self,
        job: impl FnOnce(gtk::Clipboard, oneshot::Sender<T>) + Send + 'static,
    ) -> Result<T, PlatformError> {
        let (answer, answered) = oneshot::channel();
        self.app
            .run_on_main_thread(move || job(gtk::Clipboard::get(&SELECTION_CLIPBOARD), answer))
            .map_err(|error| unavailable(error.to_string()))?;
        answered
            .await
            .map_err(|_| unavailable("the clipboard did not answer".to_owned()))
    }
}

fn unavailable(reason: String) -> PlatformError {
    PlatformError::Unavailable {
        service: "clipboard",
        reason,
    }
}

impl Clipboard for WindowClipboard {
    fn access(&self) -> ClipboardAccess {
        ClipboardAccess::FocusOnly
    }

    async fn read(&self) -> Result<Option<ClipboardText>, PlatformError> {
        self.on_main(|clipboard, answer| {
            // Which types the copy offers says whether it is files, and
            // whether it carries the secret mark.
            clipboard.request_contents(&Atom::intern("TARGETS"), move |clipboard, offered| {
                let types: Vec<String> = offered
                    .targets()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|atom| atom.name().to_string())
                    .collect();
                if holds_files(&types) {
                    let _ = answer.send(None);
                    return;
                }
                let read_text = move |clipboard: &gtk::Clipboard, sensitive: bool| {
                    clipboard.request_text(move |_, text| {
                        let _ = answer.send(text.map(|text| ClipboardText {
                            text: text.to_owned(),
                            sensitive,
                        }));
                    });
                };
                if types.iter().any(|name| name == SECRET_HINT) {
                    clipboard.request_contents(
                        &Atom::intern(SECRET_HINT),
                        move |clipboard, hint| {
                            read_text(clipboard, is_secret(&hint.data()));
                        },
                    );
                } else {
                    read_text(clipboard, false);
                }
            });
        })
        .await
    }

    async fn write(&self, text: &ClipboardText) -> Result<(), PlatformError> {
        let text = text.clone();
        let written = self
            .on_main(move |clipboard, answer| {
                let written = if text.sensitive {
                    set_secret(&clipboard, text.text)
                } else {
                    clipboard.set_text(&text.text);
                    true
                };
                let _ = answer.send(written);
            })
            .await?;
        if written {
            Ok(())
        } else {
            Err(unavailable("the clipboard refused the text".to_owned()))
        }
    }

    async fn changed(&self) -> Result<(), PlatformError> {
        // The clipboard cannot be followed from the background here.
        std::future::pending().await
    }
}

/// Puts `text` on the clipboard with the mark that keeps it out of
/// clipboard history.
fn set_secret(clipboard: &gtk::Clipboard, text: String) -> bool {
    let flags = gtk::TargetFlags::empty();
    let mut types: Vec<gtk::TargetEntry> = TEXT_TYPES
        .iter()
        .map(|name| gtk::TargetEntry::new(name, flags, 0))
        .collect();
    types.push(gtk::TargetEntry::new(SECRET_HINT, flags, HINT_INFO));
    clipboard.set_with_data(&types, move |_, data, info| {
        if info == HINT_INFO {
            data.set(&Atom::intern(SECRET_HINT), 8, SECRET);
        } else {
            data.set_text(&text);
        }
    })
}

/// Whether a copy that offers `types` is copied files.
fn holds_files(types: &[String]) -> bool {
    types.iter().any(|name| FILE_TYPES.contains(&name.as_str()))
}

/// Whether the secret mark's content says the copy is secret.
fn is_secret(hint: &[u8]) -> bool {
    hint == SECRET
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    /// A clipboard in memory, whose access the test can take away.
    struct Memory {
        access: Mutex<ClipboardAccess>,
        text: Mutex<Option<ClipboardText>>,
    }

    impl Memory {
        fn new(access: ClipboardAccess, text: &str) -> Self {
            Self {
                access: Mutex::new(access),
                text: Mutex::new(Some(ClipboardText {
                    text: text.to_owned(),
                    sensitive: false,
                })),
            }
        }

        fn text(&self) -> String {
            self.text.lock().unwrap().clone().unwrap().text
        }
    }

    impl Clipboard for &Memory {
        fn access(&self) -> ClipboardAccess {
            *self.access.lock().unwrap()
        }

        async fn read(&self) -> Result<Option<ClipboardText>, PlatformError> {
            Ok(self.text.lock().unwrap().clone())
        }

        async fn write(&self, text: &ClipboardText) -> Result<(), PlatformError> {
            *self.text.lock().unwrap() = Some(text.clone());
            Ok(())
        }

        async fn changed(&self) -> Result<(), PlatformError> {
            std::future::pending().await
        }
    }

    #[test]
    fn the_window_takes_over_once_the_background_clipboard_is_gone() {
        tauri::async_runtime::block_on(window_takes_over());
    }

    async fn window_takes_over() {
        let platform = Memory::new(ClipboardAccess::Background, "platform");
        let window = Memory::new(ClipboardAccess::FocusOnly, "window");
        let clipboard = FallbackClipboard::new(&platform, &window);
        let copy = ClipboardText {
            text: "copy".into(),
            sensitive: false,
        };

        assert_eq!(clipboard.access(), ClipboardAccess::Background);
        assert_eq!(clipboard.read().await.unwrap().unwrap().text, "platform");
        clipboard.write(&copy).await.unwrap();
        assert_eq!(platform.text(), "copy");

        // The display server went away.
        *platform.access.lock().unwrap() = ClipboardAccess::FocusOnly;
        assert_eq!(clipboard.access(), ClipboardAccess::FocusOnly);
        assert_eq!(clipboard.read().await.unwrap().unwrap().text, "window");
        clipboard.write(&copy).await.unwrap();
        assert_eq!(window.text(), "copy");
    }

    #[test]
    fn copied_files_are_told_from_text() {
        let text = ["UTF8_STRING".to_owned(), "text/plain".to_owned()];
        assert!(!holds_files(&text));
        let files = [
            "text/uri-list".to_owned(),
            "x-special/gnome-copied-files".to_owned(),
            "UTF8_STRING".to_owned(),
        ];
        assert!(holds_files(&files));
        assert!(holds_files(&["x-special/gnome-copied-files".to_owned()]));
    }

    #[test]
    fn only_the_secret_value_marks_a_copy_secret() {
        assert!(is_secret(b"secret"));
        assert!(!is_secret(b""));
        assert!(!is_secret(b"public"));
    }
}
