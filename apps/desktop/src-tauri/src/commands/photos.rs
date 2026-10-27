//! The phone's recent photos: the list, thumbnails, and saving a photo.
//!
//! Pictures reach the page as raw bytes, which it turns into a `blob:` URL:
//! a picture for the viewer can be hundreds of KiB, which JSON or base64
//! would make larger and slower to read. A saved photo is named by its phone
//! and photo id, so the page never names a path.

use specta::datatype::DataType;
use tauri::State;
use tauri::ipc::{InvokeResponseBody, IpcResponse};

use crate::backend::{PhonePhotos, SavedPhoto};
use crate::commands::CommandError;
use crate::state::AppState;

/// An encoded image, which the page receives as an `ArrayBuffer` with no
/// JSON or base64 in between.
pub struct ImageBytes(Vec<u8>);

impl IpcResponse for ImageBytes {
    fn body(self) -> tauri::Result<InvokeResponseBody> {
        Ok(InvokeResponseBody::Raw(self.0))
    }
}

/// Specta has no type for a raw response, so the bindings name what the
/// page receives instead.
impl specta::Type for ImageBytes {
    fn definition(_: &mut specta::Types) -> DataType {
        DataType::Reference(specta_typescript::define("ArrayBuffer"))
    }
}

/// A phone's recent photos, newest first, and whether they can be shown.
/// Later changes arrive as `PhotosChanged` events: subscribe first, then
/// call this.
#[tauri::command]
#[specta::specta]
pub fn list_photos(
    state: State<'_, AppState>,
    device_id: String,
) -> Result<PhonePhotos, CommandError> {
    state.backend().photos(&device_id).map_err(Into::into)
}

/// A thumbnail of a phone's photo, at most `max_size` pixels (64 to 2048)
/// on its longest side: the bytes of a JPEG, PNG or WebP image. The desktop
/// keeps the thumbnails it fetched in memory, so asking again for the same
/// size is quick.
#[tauri::command]
#[specta::specta]
pub async fn photo_thumbnail(
    state: State<'_, AppState>,
    device_id: String,
    photo_id: String,
    max_size: u32,
) -> Result<ImageBytes, CommandError> {
    let thumbnail = state
        .backend()
        .photo_thumbnail(&device_id, &photo_id, max_size);
    Ok(ImageBytes(thumbnail.await?.to_vec()))
}

/// Fetches a phone's photo and saves it in the user's Pictures folder,
/// under a free name. Resolves with the name it got.
#[tauri::command]
#[specta::specta]
pub async fn save_photo(
    state: State<'_, AppState>,
    device_id: String,
    photo_id: String,
) -> Result<SavedPhoto, CommandError> {
    let saving = state.backend().save_photo(&device_id, &photo_id);
    saving.await.map_err(Into::into)
}

/// Opens a photo the user saved with the desktop's default application.
#[tauri::command]
#[specta::specta]
pub fn open_saved_photo(
    state: State<'_, AppState>,
    device_id: String,
    photo_id: String,
) -> Result<(), CommandError> {
    state
        .backend()
        .open_saved_photo(&device_id, &photo_id)
        .map_err(Into::into)
}

/// Shows a photo the user saved in its folder.
#[tauri::command]
#[specta::specta]
pub fn show_saved_photo(
    state: State<'_, AppState>,
    device_id: String,
    photo_id: String,
) -> Result<(), CommandError> {
    state
        .backend()
        .show_saved_photo(&device_id, &photo_id)
        .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn images_reach_the_page_as_raw_bytes() {
        let body = ImageBytes(vec![0xff, 0xd8, 0xff]).body().unwrap();
        assert!(matches!(body, InvokeResponseBody::Raw(bytes) if bytes == [0xff, 0xd8, 0xff]));
    }
}
