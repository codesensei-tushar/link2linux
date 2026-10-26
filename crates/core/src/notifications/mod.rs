//! Phone notifications, mirrored on the desktop (`docs/protocol.md`,
//! "Notifications").
//!
//! The phone owns its notifications; the desktop follows them through delta
//! sync and shows them, and can dismiss them on the phone or run their
//! actions, including inline replies.
//!
//! - [`mirror`]: the desktop's copy, kept in step with the phone's sync
//!   batches and live events;
//! - [`present`]: how the copy appears as desktop banners, and what the
//!   user's answers to them ask of the phone;
//! - [`images`]: the pictures notifications refer to, fetched once and
//!   kept on disk by SHA-256;
//! - [`service`]: [`NotificationService`], which runs all of the above
//!   over a session.

pub mod images;
pub mod mirror;
pub mod present;
pub mod service;

pub use images::{ImageCache, ImageError};
pub use mirror::{Change, Mirror, MirrorError};
pub use present::{Answer, answer, to_desktop, wants_banner};
pub use service::{NotificationService, NotificationSettings, ServiceError};
