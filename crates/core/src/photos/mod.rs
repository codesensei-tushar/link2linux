//! The phone's recent photos, which the desktop shows and saves
//! (`docs/protocol.md`, "Photos").
//!
//! The photos live on the phone. [`PhotoService`] asks for the list
//! whenever the feature becomes ready on both devices, which also
//! subscribes the desktop to its changes, and fetches thumbnails and the
//! original files only when the user's view asks for them. What it fetched
//! stays in memory, never on disk: thumbnails in a bounded cache that
//! lasts across sessions with the phone, until the phone's photos stop
//! being ready. [`save`] writes a fetched photo where the user keeps
//! pictures, following the rules for received files.
//!
//! Names and photos are the user's: they never reach a log.

mod cache;
mod service;

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use link2linux_protocol::v1::{self as wire, ImageFormat};

pub use service::{PhotoService, PhotoWaiter, ThumbnailWaiter};

use crate::files::{received_name, save_new};

/// The most photos in a list.
pub const MAX_PHOTOS: usize = 200;

/// The longest photo id, in bytes.
pub const MAX_ID_BYTES: usize = 64;

/// The smallest and largest longest side a thumbnail can be asked for.
pub const THUMBNAIL_SIZES: std::ops::RangeInclusive<u32> = 64..=2048;

/// The largest thumbnail the phone may send, in bytes.
pub const MAX_THUMBNAIL_BYTES: usize = 400 * 1024;

/// The bytes of a photo one `PhotoData` carries.
pub const CHUNK_BYTES: u64 = 256 * 1024;

/// The largest photo the desktop fetches. It holds a photo in memory
/// until it is saved, so a larger one is refused.
pub const MAX_PHOTO_BYTES: u64 = 256 * 1024 * 1024;

/// One photo on the phone, checked against the protocol's rules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhotoInfo {
    /// The phone's name for it.
    pub id: String,
    /// Its file name, safe to show and to save under.
    pub name: String,
    /// The MIME type, such as `image/jpeg`, or empty when unknown.
    pub mime_type: String,
    /// The file's size in bytes.
    pub size: u64,
    /// When it was taken, in Unix milliseconds.
    pub taken_unix_ms: i64,
    /// When its file last changed, in Unix milliseconds. A new value is a
    /// new version of the photo.
    pub modified_unix_ms: i64,
    /// Its size in pixels as it is shown; 0 when unknown.
    pub width: u32,
    /// Its height in pixels as it is shown; 0 when unknown.
    pub height: u32,
}

impl PhotoInfo {
    /// The photo as it arrived, or `None` when its id breaks the rules.
    #[must_use]
    pub fn from_wire(photo: &wire::Photo) -> Option<Self> {
        if photo.id.is_empty() || photo.id.len() > MAX_ID_BYTES {
            return None;
        }
        let mime_type = if photo.mime_type.len() <= MAX_MIME_BYTES
            && photo.mime_type.bytes().all(|b| b.is_ascii_graphic())
        {
            photo.mime_type.clone()
        } else {
            String::new()
        };
        Some(Self {
            id: photo.id.clone(),
            name: received_name(&photo.name),
            mime_type,
            size: photo.size,
            taken_unix_ms: photo.taken_unix_ms,
            modified_unix_ms: photo.modified_unix_ms,
            width: photo.width,
            height: photo.height,
        })
    }
}

/// The longest MIME type kept; a longer one is no type the desktop knows.
const MAX_MIME_BYTES: usize = 127;

/// What the desktop can show of the phone's photos.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhotosState {
    /// The phone's list, which [`PhotoService::photos`] holds.
    Listed,
    /// The desktop asked for the list, or waits for the phone to say
    /// whether it shares its photos.
    Waiting,
    /// The user has photos turned off on the phone.
    TurnedOff,
    /// The phone's app may not read the photos: the user has to allow it
    /// on the phone.
    PermissionRequired,
    /// The phone does not share its photos right now, or did not send the
    /// list.
    Unavailable,
    /// The phone's app does not share photos: it is older than this
    /// desktop's.
    Unsupported,
    /// No session with the phone is up.
    Disconnected,
}

/// A smaller copy of a photo, to show it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thumbnail {
    /// `image/jpeg`, `image/png` or `image/webp`.
    pub mime_type: &'static str,
    /// The encoded image.
    pub data: Arc<[u8]>,
    /// Its width in pixels.
    pub width: u32,
    /// Its height in pixels.
    pub height: u32,
}

impl Thumbnail {
    /// The thumbnail as it arrived, or `None` when it breaks the rules: an
    /// unknown format, or no data, or more than [`MAX_THUMBNAIL_BYTES`].
    fn from_wire(thumbnail: wire::PhotoThumbnail) -> Option<Self> {
        let mime_type = match thumbnail.format() {
            ImageFormat::Jpeg => "image/jpeg",
            ImageFormat::Png => "image/png",
            ImageFormat::Webp => "image/webp",
            ImageFormat::Unspecified => return None,
        };
        if thumbnail.data.is_empty() || thumbnail.data.len() > MAX_THUMBNAIL_BYTES {
            return None;
        }
        Some(Self {
            mime_type,
            data: thumbnail.data.into(),
            width: thumbnail.width,
            height: thumbnail.height,
        })
    }
}

/// A photo's original file, fetched whole.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhotoFile {
    /// Its file name, safe to save under.
    pub name: String,
    /// When the file last changed, as the phone said while it was fetched.
    pub modified_unix_ms: i64,
    /// The file's bytes.
    pub data: Arc<[u8]>,
}

/// Why the desktop has no thumbnail or photo to show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PhotoError {
    /// No session with the phone is up.
    #[error("the phone is not connected")]
    NotConnected,
    /// The phone's photos are not ready, as when the user turned them off
    /// there.
    #[error("the phone does not share its photos right now")]
    NotReady,
    /// The phone's list holds no such photo, or the phone no longer has it.
    #[error("the photo is no longer on the phone")]
    NotFound,
    /// The phone did not answer in time.
    #[error("the phone did not answer in time")]
    TimedOut,
    /// The photo kept changing on the phone while it was fetched.
    #[error("the photo changed on the phone while it was fetched")]
    Changed,
    /// The photo is larger than [`MAX_PHOTO_BYTES`].
    #[error("the photo is too large")]
    TooLarge,
    /// The phone could not send it, or sent something else.
    #[error("the phone could not send the photo")]
    Failed,
}

/// Saves a fetched photo in `dir` under its own name, as a received file is
/// saved: a free name such as `photo (1).jpg` when the name is taken, a
/// temporary name until the bytes are written, and never executable.
/// Returns where it was saved. Blocks, so callers run it off the threads
/// that drive sessions.
///
/// # Errors
///
/// Fails when the directory cannot be made or written to.
pub fn save(dir: &Path, photo: &PhotoFile) -> io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    save_new(dir, &photo.name, &photo.data)
}
