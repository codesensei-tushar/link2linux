//! A clipboard shared between the phone and the desktop.
//!
//! Each device keeps its own clipboard: when the user copies text on one,
//! it pushes the text to the other, which puts it on its own clipboard
//! (`docs/protocol.md`, "Clipboard"). On the desktop, two parts do this:
//!
//! - [`ClipboardHub`] is what the services of all paired phones share: the
//!   user's settings, the latest copy made on this desktop, the text a
//!   phone last put on the clipboard, which is never shared on by itself,
//!   and the text that waits for the window on a desktop that lets the app
//!   use its clipboard only while the window has focus
//!   ([`ClipboardAccess::FocusOnly`]).
//! - [`ClipboardService`] is one phone's part, driven by its [`Session`]
//!   like the notifications service: it pushes the copies the hub hands
//!   it, one at a time, and takes the phone's pushes.
//!
//! What happens comes out of [`ClipboardHub::subscribe`] as
//! [`ClipboardEvent`]s, for the clipboard page. Clipboard text may be a
//! password, so it never reaches a log or an event: only its length does.
//!
//! [`Session`]: crate::session::Session
//! [`ClipboardAccess::FocusOnly`]: link2linux_platform::ClipboardAccess::FocusOnly

use std::fmt;

use aws_lc_rs::digest;
use link2linux_platform::ClipboardText;
use tokio::time::Duration;

mod hub;
mod service;

pub use hub::{ClipboardHub, Held};
pub use service::ClipboardService;

/// The longest text a push carries, in bytes. Longer text is not shared,
/// and never cut short: a partial paste is worse than none.
pub const MAX_TEXT_BYTES: usize = 256 * 1024;

/// A phone whose session starts within this time after the user copied
/// something gets that copy.
pub const RECENT_COPY: Duration = Duration::from_secs(60);

/// The user's settings for the feature on this desktop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClipboardSettings {
    /// The desktop shares its clipboard and takes the phones' pushes. On
    /// by default; the phone has its own switch, off until the user turns
    /// it on.
    pub sharing: bool,
    /// Text that a password manager marked as secret is shared too. Off by
    /// default.
    pub share_passwords: bool,
}

impl Default for ClipboardSettings {
    fn default() -> Self {
        Self {
            sharing: true,
            share_passwords: false,
        }
    }
}

/// Copied text on its way to the phones, with the digest that recognizes
/// it when it comes back.
#[derive(Clone)]
pub struct Clip {
    text: ClipboardText,
    digest: Digest,
}

impl Clip {
    /// Wraps `text`.
    #[must_use]
    pub fn new(text: ClipboardText) -> Self {
        let digest = Digest::of(&text.text);
        Self { text, digest }
    }

    /// The text.
    #[must_use]
    pub fn text(&self) -> &ClipboardText {
        &self.text
    }

    /// Its length in bytes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.text.text.len()
    }

    /// Whether it holds no text.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.text.text.is_empty()
    }

    /// Whether the app it was copied from marked it as secret.
    #[must_use]
    pub fn is_sensitive(&self) -> bool {
        self.text.sensitive
    }

    fn digest(&self) -> Digest {
        self.digest
    }
}

impl fmt::Debug for Clip {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.text.fmt(f)
    }
}

/// The SHA-256 of a text, which is how the desktop recognizes text it has
/// seen without keeping it.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Digest([u8; 32]);

impl Digest {
    fn of(text: &str) -> Self {
        let digest = digest::digest(&digest::SHA256, text.as_bytes());
        let mut bytes = [0; 32];
        bytes.copy_from_slice(digest.as_ref());
        Self(bytes)
    }
}

impl fmt::Debug for Digest {
    // Even a digest lets anyone who guesses the text confirm it.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Digest(..)")
    }
}

/// Why a push is sent: the desktop shares a copy by itself, or the user
/// asked for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// A copy the desktop saw. It does not go back to the phone it came
    /// from, nor to a phone that already has it.
    Automatic,
    /// The user asked to send the clipboard: it goes to every phone.
    Explicit,
}

/// Why something the user copied on this desktop was not shared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotShared {
    /// The clipboard holds no text, such as copied files or an image.
    NoText,
    /// The text is longer than [`MAX_TEXT_BYTES`].
    TooLarge {
        /// Its length.
        bytes: usize,
    },
    /// A password manager marked it as secret, and "Share passwords" is
    /// off.
    Secret,
    /// The user turned clipboard sharing off on this desktop.
    Off,
}

/// Why a phone did not take a push.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushFailure {
    /// The user turned clipboard sharing off on the phone.
    TurnedOff,
    /// The phone could not set its clipboard right now.
    Unavailable,
    /// The phone refused it, gave no answer in time, or the session ended
    /// first.
    Failed,
}

/// Whether a phone can take pushes now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhoneClipboard {
    /// It takes them.
    Ready,
    /// The user has clipboard sharing off on the phone.
    TurnedOff,
    /// It shares a clipboard, but cannot right now.
    Unavailable,
    /// Its app does not share a clipboard: it is older than this desktop's.
    Unsupported,
    /// No session with it is up.
    Disconnected,
}

/// What happened, for the clipboard page. It never carries the text.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ClipboardEvent {
    /// A phone took a push from this desktop.
    Sent {
        /// The phone.
        device_id: String,
        /// The text's length.
        bytes: usize,
        /// The text was marked as secret.
        sensitive: bool,
    },
    /// A phone pushed text, and it is on this desktop's clipboard, or
    /// `held` for the window to gain focus.
    Received {
        /// The phone.
        device_id: String,
        /// The text's length.
        bytes: usize,
        /// The text was marked as secret.
        sensitive: bool,
        /// It waits in [`ClipboardHub::held`].
        held: bool,
    },
    /// The text that waited for the window is on the clipboard now.
    Released {
        /// The phone it came from.
        device_id: String,
    },
    /// A phone did not take a push.
    Failed {
        /// The phone.
        device_id: String,
        /// Why.
        reason: PushFailure,
    },
    /// Something copied on this desktop was not shared. Only reasons the
    /// user can act on come out: never [`NotShared::NoText`] or
    /// [`NotShared::Off`] for a copy the desktop saw by itself.
    NotShared(NotShared),
    /// A phone's state changed.
    Phone {
        /// The phone.
        device_id: String,
        /// Its new state.
        state: PhoneClipboard,
    },
}

/// Checks a copy against the protocol's limits and the user's settings.
fn check(clip: &Clip, settings: ClipboardSettings) -> Result<(), NotShared> {
    if !settings.sharing {
        Err(NotShared::Off)
    } else if clip.is_empty() {
        Err(NotShared::NoText)
    } else if clip.len() > MAX_TEXT_BYTES {
        Err(NotShared::TooLarge { bytes: clip.len() })
    } else if clip.is_sensitive() && !settings.share_passwords {
        Err(NotShared::Secret)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clip(text: &str, sensitive: bool) -> Clip {
        Clip::new(ClipboardText {
            text: text.to_owned(),
            sensitive,
        })
    }

    #[test]
    fn copies_are_checked_against_the_limits_and_the_settings() {
        let on = ClipboardSettings::default();
        assert!(on.sharing && !on.share_passwords);
        assert_eq!(check(&clip("hi", false), on), Ok(()));
        assert_eq!(check(&clip("", false), on), Err(NotShared::NoText));
        let longest = "a".repeat(MAX_TEXT_BYTES);
        assert_eq!(check(&clip(&longest, false), on), Ok(()));
        let longer = "a".repeat(MAX_TEXT_BYTES + 1);
        assert_eq!(
            check(&clip(&longer, false), on),
            Err(NotShared::TooLarge {
                bytes: MAX_TEXT_BYTES + 1
            })
        );
        assert_eq!(check(&clip("hunter2", true), on), Err(NotShared::Secret));
        let passwords = ClipboardSettings {
            share_passwords: true,
            ..on
        };
        assert_eq!(check(&clip("hunter2", true), passwords), Ok(()));
        let off = ClipboardSettings {
            sharing: false,
            ..on
        };
        assert_eq!(check(&clip("hi", false), off), Err(NotShared::Off));
    }

    #[test]
    fn the_text_stays_out_of_debug_output() {
        let printed = format!("{:?}", clip("hunter2", true));
        assert!(!printed.contains("hunter2"), "{printed}");
        assert!(printed.contains("bytes: 7"), "{printed}");
        assert_eq!(format!("{:?}", Digest::of("hunter2")), "Digest(..)");
    }

    #[test]
    fn equal_text_has_equal_digests() {
        assert_eq!(clip("a", false).digest(), clip("a", true).digest());
        assert_ne!(clip("a", false).digest(), clip("b", false).digest());
    }
}
