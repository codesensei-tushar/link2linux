//! Phone notifications, mirrored on the desktop (`docs/protocol.md`,
//! "Notifications").
//!
//! The phone owns its notifications; the desktop follows them through delta
//! sync and shows them, and can dismiss them on the phone or run their
//! actions, including inline replies.
//!
//! - [`mirror`]: the desktop's copy, kept in step with the phone's sync
//!   batches and live events.

pub mod mirror;

pub use mirror::{Change, Mirror, MirrorError};
