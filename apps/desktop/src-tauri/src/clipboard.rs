//! The clipboard through the window's toolkit, for desktop sessions that let
//! an app use the clipboard only while its window has focus, such as a
//! Wayland session without a clipboard protocol for background apps.
//!
//! GTK runs on the app's main thread, so each call is sent there and its
//! answer comes back through a channel. The runtime uses this clipboard
//! only while the window has focus.

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
    use super::*;

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
