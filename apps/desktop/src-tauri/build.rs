fn main() {
    // Listing the app's commands makes tauri-build generate an
    // `allow-<command>` permission for each one. Commands are then denied
    // unless a capability grants them (see capabilities/default.json).
    let manifest = tauri_build::AppManifest::new().commands(&[
        "app_info",
        "startup_error",
        "restart_app",
        "list_devices",
        "unpair_device",
        "get_pairing_state",
        "start_pairing",
        "pair_with",
        "answer_code",
        "cancel_pairing",
        "list_notifications",
        "dismiss_notification",
        "run_notification_action",
        "notification_image",
        "get_clipboard_status",
        "send_clipboard",
        "get_settings",
        "set_settings",
    ]);

    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(manifest))
        .expect("failed to run tauri-build");
}
