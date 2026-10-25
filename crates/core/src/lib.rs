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
//!   protocol version, who the peer is and which features both sides use,
//!   then `Ping`/`Pong` liveness and the flow of feature messages.
//!
//! Then one module per [`Feature`], named as in both apps:
//! [`notifications`], [`messages`], [`calls`], [`photos`], [`files`],
//! [`clipboard`] and [`screen`]. Each one
//! holds the feature's types, the events the phone reports, and a trait for
//! the requests the desktop makes, which fail with a [`FeatureError`].
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

pub mod calls;
pub mod clipboard;
pub mod files;
pub mod messages;
pub mod notifications;
pub mod photos;
pub mod screen;

pub use device::{DeviceInfo, DeviceKind, Feature, FeatureError};
pub use session::{Session, SessionConfig, SessionError, SessionEvent};
