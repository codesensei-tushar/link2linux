use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use tauri::State;

use crate::backend::MirroredNotification;
use crate::commands::CommandError;
use crate::state::AppState;

/// The notifications currently mirrored from a phone, in no particular
/// order. Later changes arrive as `NotificationChanged` events: subscribe
/// first, then call this.
#[tauri::command]
#[specta::specta]
pub fn list_notifications(
    state: State<'_, AppState>,
    device_id: String,
) -> Result<Vec<MirroredNotification>, CommandError> {
    state
        .backend()
        .notifications(&device_id)
        .map_err(Into::into)
}

/// Dismisses a notification on the phone. It is queued and sent once the
/// phone is connected.
#[tauri::command]
#[specta::specta]
pub fn dismiss_notification(
    state: State<'_, AppState>,
    device_id: String,
    key: String,
) -> Result<(), CommandError> {
    state
        .backend()
        .dismiss_notification(&device_id, &key)
        .map_err(Into::into)
}

/// Runs one of a notification's actions on the phone, with the text of a
/// reply for actions that take one.
#[tauri::command]
#[specta::specta]
pub fn run_notification_action(
    state: State<'_, AppState>,
    device_id: String,
    key: String,
    action_id: String,
    reply_text: Option<String>,
) -> Result<(), CommandError> {
    state
        .backend()
        .run_notification_action(&device_id, &key, &action_id, reply_text.as_deref())
        .map_err(Into::into)
}

/// A picture a phone shared with a notification, as a `data:` URL the page
/// can put in an `<img>`, or nothing while it has not arrived.
#[tauri::command]
#[specta::specta]
pub async fn notification_image(
    state: State<'_, AppState>,
    device_id: String,
    sha256: String,
) -> Result<Option<String>, CommandError> {
    // Pictures are in memory or on the runtime directory's tmpfs, so
    // reading one here is quick.
    let image = state.backend().notification_image(&device_id, &sha256);
    Ok(image.map(|image| data_url(Some(image.extension), &image.data)))
}

/// The `data:` URL of an image file with this extension.
fn data_url(extension: Option<&str>, bytes: &[u8]) -> String {
    let mime = match extension {
        Some("jpg") => "image/jpeg",
        Some("webp") => "image/webp",
        _ => "image/png",
    };
    format!("data:{mime};base64,{}", STANDARD.encode(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn images_become_data_urls_by_extension() {
        assert_eq!(data_url(Some("png"), b"ab"), "data:image/png;base64,YWI=");
        assert!(data_url(Some("jpg"), b"").starts_with("data:image/jpeg;base64,"));
        assert!(data_url(Some("webp"), b"").starts_with("data:image/webp;base64,"));
        assert!(data_url(None, b"").starts_with("data:image/png;base64,"));
    }
}
