//! Showing a phone's screen in the window, and controlling the phone from
//! there.
//!
//! The frames go to the page through the channel it passes to
//! `start_screen`, as raw bytes: each message is one frame packet (see
//! `link2linux_runtime::screen::packet`), which arrives as an
//! `ArrayBuffer`. What the page shows around them, such as the user's
//! answer on the phone, arrives as `ScreenChanged` events.

use std::sync::Arc;

use tauri::ipc::{Channel, InvokeResponseBody, IpcResponse};
use tauri::{AppHandle, Manager, State, Webview};

use crate::backend::{ScreenCodec, ScreenInput, ScreenStatus, ScreenViewer};
use crate::commands::CommandError;
use crate::state::AppState;

/// One frame for the page. It travels as raw bytes, not as JSON, and the
/// page receives an `ArrayBuffer`.
pub struct ScreenPacket(Vec<u8>);

impl IpcResponse for ScreenPacket {
    fn body(self) -> tauri::Result<InvokeResponseBody> {
        Ok(InvokeResponseBody::Raw(self.0))
    }
}

impl specta::Type for ScreenPacket {
    fn definition(types: &mut specta::Types) -> specta::datatype::DataType {
        // An `ArrayBuffer`, which TypeScript bindings have no name for.
        <specta_typescript::Unknown as specta::Type>::definition(types)
    }
}

/// The page that asked for the screen.
struct PageViewer {
    frames: Channel<ScreenPacket>,
    app: AppHandle,
    /// The webview the page lives in. A channel to a closed webview drops
    /// what it is sent without an error, so the viewer looks for the
    /// webview instead.
    label: String,
}

impl ScreenViewer for PageViewer {
    fn show(&self, packet: Vec<u8>) -> bool {
        self.app.get_webview_window(&self.label).is_some()
            && self.frames.send(ScreenPacket(packet)).is_ok()
    }
}

/// What the page shows of a phone's screen. Later changes arrive as
/// `ScreenChanged` events: subscribe first, then call this.
#[tauri::command]
#[specta::specta]
pub fn get_screen_status(
    state: State<'_, AppState>,
    device_id: String,
) -> Result<ScreenStatus, CommandError> {
    state
        .backend()
        .screen_status(&device_id)
        .map_err(Into::into)
}

/// Asks a phone to show its screen: in one of `codecs`, most preferred
/// first, with its longer side at most `max_size` pixels (0: as the phone
/// likes). The phone asks its user first. Frames go to `frames` until the
/// stream ends; how it goes arrives as `ScreenChanged` events.
#[tauri::command]
#[specta::specta]
pub fn start_screen(
    app: AppHandle,
    webview: Webview,
    state: State<'_, AppState>,
    device_id: String,
    codecs: Vec<ScreenCodec>,
    max_size: u32,
    frames: Channel<ScreenPacket>,
) -> Result<(), CommandError> {
    let viewer = Arc::new(PageViewer {
        frames,
        app,
        label: webview.label().to_owned(),
    });
    state
        .backend()
        .start_screen(&device_id, &codecs, max_size, viewer)
        .map_err(Into::into)
}

/// Stops showing a phone's screen. The page calls it when it closes.
#[tauri::command]
#[specta::specta]
pub fn stop_screen(state: State<'_, AppState>, device_id: String) -> Result<(), CommandError> {
    state.backend().stop_screen(&device_id).map_err(Into::into)
}

/// Sends what the user did on the phone's screen in the page. Input the
/// phone did not take shows in the next `ScreenChanged` event.
#[tauri::command]
#[specta::specta]
pub fn send_screen_input(
    state: State<'_, AppState>,
    device_id: String,
    input: ScreenInput,
) -> Result<(), CommandError> {
    state
        .backend()
        .send_screen_input(&device_id, input)
        .map_err(Into::into)
}

/// Asks the phone for a key frame, because the page could not decode a
/// frame or fell behind. It drops frames until the key frame comes.
#[tauri::command]
#[specta::specta]
pub fn request_screen_key_frame(
    state: State<'_, AppState>,
    device_id: String,
) -> Result<(), CommandError> {
    state
        .backend()
        .request_screen_key_frame(&device_id)
        .map_err(Into::into)
}
