use serde::Serialize;
use specta::Type;
use tauri::AppHandle;

/// Facts about the running desktop app, shown in the UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    /// Product name, e.g. "link2linux".
    pub name: String,
    /// App version (semver).
    pub version: String,
    /// Reverse-DNS app identifier.
    pub identifier: String,
    /// Operating system family, e.g. "linux".
    pub os: String,
    /// CPU architecture, e.g. "x86_64".
    pub arch: String,
}

/// Returns the app's name, version and the platform it runs on.
#[tauri::command]
#[specta::specta]
pub fn app_info(app: AppHandle) -> AppInfo {
    let package = app.package_info();
    AppInfo {
        name: package.name.clone(),
        version: package.version.to_string(),
        identifier: app.config().identifier.clone(),
        os: std::env::consts::OS.to_owned(),
        arch: std::env::consts::ARCH.to_owned(),
    }
}
