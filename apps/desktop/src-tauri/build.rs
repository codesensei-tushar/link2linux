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
        "list_transfers",
        "pick_files_to_send",
        "send_dropped_files",
        "cancel_transfer",
        "forget_transfer",
        "open_received_file",
        "show_received_file",
        "get_phone_status",
        "get_ring_status",
        "ring_phone",
        "stop_ringing",
        "get_settings",
        "set_settings",
        "get_calls_status",
        "start_bluetooth_pairing",
        "cancel_bluetooth_pairing",
        "confirm_bluetooth_code",
        "choose_bluetooth_device",
        "connect_calls",
        "answer_call",
        "decline_call",
        "hang_up_call",
        "dial_number",
        "send_call_tones",
        "set_call_muted",
    ]);

    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(manifest))
        .expect("failed to run tauri-build");
}
