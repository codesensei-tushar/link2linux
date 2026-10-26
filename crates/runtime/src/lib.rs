//! The desktop runtime: everything between the app shell and the phones it
//! is paired with.
//!
//! The shell hands it a notification sink and an event sink; the runtime
//! loads the identity and the paired phones from the data directory, runs
//! discovery, keeps a session with each paired phone, pairs new ones, and
//! reports every change as a snapshot through [`view::EventSink`].

pub mod paths;
pub mod view;
