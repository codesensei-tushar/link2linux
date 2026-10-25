fn main() {
    // Listing the app's commands makes tauri-build generate an
    // `allow-<command>` permission for each one. Commands are then denied
    // unless a capability grants them (see capabilities/default.json).
    let manifest = tauri_build::AppManifest::new().commands(&["app_info", "list_devices"]);

    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(manifest))
        .expect("failed to run tauri-build");
}
