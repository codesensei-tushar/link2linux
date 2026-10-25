//! The UI-free core of the link2linux desktop app.
//!
//! Everything between the network and the UI lives here, so it can be tested
//! without a window:
//!
//! - [`device`]: who a device is and which [`Feature`]s it serves;
//! - [`discovery`]: finding devices on the local network;
//! - [`pairing`]: the one-time trust step and the store of paired devices;
//! - [`transport`]: the TLS stream to a paired device;
//! - [`session`]: the `Hello` exchange on that stream, which settles the
//!   protocol version, who the peer is and which features both sides use.
//!
//! Nothing here talks to the OS directly. Desktop integration sits behind
//! the traits in `link2linux-platform`, and the app shell wires the two
//! together.

#![warn(missing_docs)]

pub mod device;
pub mod discovery;
pub mod pairing;
pub mod session;
pub mod transport;

pub use device::{DeviceInfo, DeviceKind, Feature};
pub use session::{Session, SessionConfig, SessionError};
