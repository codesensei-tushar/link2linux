//! The desktop runtime: everything between the app shell and the phones it
//! is paired with.
//!
//! The shell hands it a notification sink, the desktop clipboard and an
//! event sink; the runtime loads the identity and the paired phones from
//! the data directory, runs discovery, keeps a session with each paired
//! phone, pairs new ones, and reports every change as a snapshot through
//! [`view::EventSink`].

pub mod candidates;
pub mod clipboard;
mod device;
pub mod dial;
mod discovery;
pub mod features;
pub mod files;
pub mod link;
pub mod notify;
mod pairing;
pub mod paths;
pub mod qr;
mod runtime;
pub mod settings;
mod shared;
pub mod view;

pub use runtime::Runtime;
pub use shared::{RuntimeConfig, RuntimeError, Shell};

/// The stand-in phone of the loopback tests, for tests inside the crate.
#[cfg(test)]
#[path = "../../core/tests/support/mod.rs"]
mod test_phone;
