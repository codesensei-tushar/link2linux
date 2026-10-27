//! What the UI shows of a phone's photos, as snapshots. They are IPC
//! types, so they derive the serde and specta traits the generated bindings
//! need.

use link2linux_core::photos::{PhotoError, PhotoInfo, PhotosState};
use serde::{Deserialize, Serialize};
use specta::Type;

use crate::view::BackendError;

/// What the desktop shows of a phone's photos.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PhonePhotos {
    pub state: PhotosStatus,
    /// The phone's recent photos, newest first, while the state is
    /// `listed`.
    pub photos: Vec<Photo>,
}

/// Whether the phone's photos can be shown, and why not.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum PhotosStatus {
    /// The phone's list is here.
    Listed,
    /// Connected, and the list is on its way.
    Waiting,
    /// The user has photos turned off on the phone.
    TurnedOff,
    /// The user has to let the app on the phone read the photos.
    PermissionRequired,
    /// The phone does not share its photos right now.
    Unavailable,
    /// The phone's app is too old to share photos.
    Unsupported,
    /// The phone is not connected.
    #[default]
    Disconnected,
}

/// One photo on the phone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Photo {
    /// The phone's name for it, which the commands take.
    pub id: String,
    /// Its file name, safe to show.
    pub name: String,
    /// Such as `image/jpeg`; empty when unknown.
    pub mime_type: String,
    /// Bytes.
    #[specta(type = specta_typescript::Number)]
    pub size: u64,
    #[specta(type = specta_typescript::Number)]
    pub taken_unix_ms: i64,
    /// Changes with each new version of the photo.
    #[specta(type = specta_typescript::Number)]
    pub modified_unix_ms: i64,
    /// Pixels as the photo is shown; 0 when unknown.
    pub width: u32,
    pub height: u32,
    /// Set when this version was saved during this run of the app.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saved: Option<SavedPhoto>,
}

/// A photo the user saved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SavedPhoto {
    /// The name it was saved under, which may differ from the phone's when
    /// that was taken.
    pub name: String,
    /// The folder it went into, such as `Pictures` or `Downloads`. The
    /// folder's own name, never a full path.
    pub folder: String,
}

/// The snapshot of `state` and `photos`.
#[must_use]
pub fn photos(state: PhotosState, photos: &[PhotoInfo]) -> PhonePhotos {
    PhonePhotos {
        state: match state {
            PhotosState::Listed => PhotosStatus::Listed,
            PhotosState::Waiting => PhotosStatus::Waiting,
            PhotosState::TurnedOff => PhotosStatus::TurnedOff,
            PhotosState::PermissionRequired => PhotosStatus::PermissionRequired,
            PhotosState::Unavailable => PhotosStatus::Unavailable,
            PhotosState::Unsupported => PhotosStatus::Unsupported,
            PhotosState::Disconnected => PhotosStatus::Disconnected,
        },
        photos: photos
            .iter()
            .map(|photo| Photo {
                id: photo.id.clone(),
                name: photo.name.clone(),
                mime_type: photo.mime_type.clone(),
                size: photo.size,
                taken_unix_ms: photo.taken_unix_ms,
                modified_unix_ms: photo.modified_unix_ms,
                width: photo.width,
                height: photo.height,
                saved: None,
            })
            .collect(),
    }
}

/// What the user is told when a photo or thumbnail does not come.
#[must_use]
pub fn error(error: PhotoError) -> BackendError {
    match error {
        PhotoError::NotConnected => BackendError::PhoneNotConnected,
        PhotoError::NotReady => BackendError::PhotosNotShared,
        PhotoError::NotFound => BackendError::PhotoGone,
        PhotoError::TooLarge => BackendError::PhotoTooLarge,
        PhotoError::TimedOut | PhotoError::Changed | PhotoError::Failed => {
            BackendError::PhotoNotFetched
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn photos_reach_the_ui_in_camel_case() {
        let info = PhotoInfo {
            id: "42".into(),
            name: "IMG_1.jpg".into(),
            mime_type: "image/jpeg".into(),
            size: 3,
            taken_unix_ms: 1,
            modified_unix_ms: 2,
            width: 4,
            height: 5,
        };
        let json = serde_json::to_string(&photos(PhotosState::Listed, &[info])).unwrap();
        assert_eq!(
            json,
            r#"{"state":"listed","photos":[{"id":"42","name":"IMG_1.jpg","mimeType":"image/jpeg","size":3,"takenUnixMs":1,"modifiedUnixMs":2,"width":4,"height":5}]}"#
        );
        let json = serde_json::to_string(&photos(PhotosState::PermissionRequired, &[])).unwrap();
        assert_eq!(json, r#"{"state":"permissionRequired","photos":[]}"#);
    }
}
