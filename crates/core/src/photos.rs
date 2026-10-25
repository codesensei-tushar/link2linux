//! Recent photos from the phone's gallery.
//!
//! The desktop shows thumbnails of the newest photos and downloads the
//! originals the user picks. Image data travels in chunks well below the
//! frame size limit.
//!
//! Stub: the types and the [`Photos`] trait are in place; the wire messages
//! come with this feature's schema.

use std::path::Path;
use std::time::SystemTime;

use crate::device::FeatureError;

/// A photo in the phone's gallery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhotoInfo {
    /// The phone's identifier for it.
    pub id: String,
    /// File name on the phone.
    pub file_name: String,
    /// MIME type, such as `image/jpeg`.
    pub mime_type: String,
    /// Size of the original, in bytes.
    pub size: u64,
    /// When it was taken.
    pub taken_at: SystemTime,
}

/// What the phone reports about its gallery.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PhotoEvent {
    /// A new photo was taken or saved.
    Added(PhotoInfo),
    /// A photo was deleted.
    Removed {
        /// Its identifier.
        id: String,
    },
}

/// Requests the desktop makes about the phone's photos.
pub trait Photos: Send + Sync {
    /// The newest photos, newest first.
    fn recent(
        &self,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<PhotoInfo>, FeatureError>> + Send;

    /// A JPEG thumbnail whose longer edge is at most `max_edge` pixels.
    fn thumbnail(
        &self,
        id: &str,
        max_edge: u32,
    ) -> impl Future<Output = Result<Vec<u8>, FeatureError>> + Send;

    /// Downloads the original to `dest` and returns the bytes written.
    fn download(
        &self,
        id: &str,
        dest: &Path,
    ) -> impl Future<Output = Result<u64, FeatureError>> + Send;
}
