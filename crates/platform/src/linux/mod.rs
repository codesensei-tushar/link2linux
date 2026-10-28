//! Linux implementations of the platform traits.
//!
//! Each type names the Linux service it uses: D-Bus services for most,
//! and the X11 and Wayland clipboards.

pub mod ble;
pub mod calls;
pub mod clipboard;
mod files;
pub mod notifications;
pub mod process;
pub mod secrets;
pub mod session_lock;
pub mod single_instance;

pub use calls::{BluezBluetooth, PipewireTelephony};
pub use clipboard::LinuxClipboard;
pub use files::XdgFileStore;
pub use notifications::DbusNotifications;
pub use process::deny_memory_access;
pub use secrets::SecretServiceStore;
pub use session_lock::SessionLock;
