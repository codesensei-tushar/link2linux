//! The UI-free core of the link2linux desktop app.
//!
//! Everything between the network and the UI lives here, so it can be tested
//! without a window:
//!
//! - [`device`]: who a device is and which [`Feature`]s it serves;
//! - [`identity`]: this desktop's key and certificate, and the fingerprints
//!   devices know each other by;
//! - [`key_storage`]: where that key is kept, the keyring or a private file;
//! - [`discovery`]: finding devices on the local network;
//! - [`pairing`]: the one-time trust step and the store of paired devices;
//! - [`transport`]: the TLS stream to a phone;
//! - [`session`]: the `Hello` exchange on that stream, which settles the
//!   protocol version, who the peer is and which features both sides use,
//!   then liveness, requests and their answers, and the flow of feature
//!   messages, until a `Goodbye`;
//! - [`sync`]: the cursor that follows a collection mirrored from the other
//!   device;
//! - [`outbox`]: the durable queue of actions the user took here that have
//!   to happen on the other device.
//!
//! Then one module per [`Feature`], named as in both apps:
//! [`notifications`], [`calls`], [`photos`], [`files`], [`clipboard`],
//! [`screen`], [`status`] and [`ring`]. Each one
//! holds the feature's types, the events the phone reports, and the service
//! or trait for the requests the desktop makes.
//!
//! Nothing here talks to the OS directly. Desktop integration sits behind
//! the traits in `link2linux-platform`, and the app shell wires the two
//! together.

#![warn(missing_docs)]

pub mod device;
pub mod discovery;
pub mod identity;
pub mod key_storage;
pub mod outbox;
pub mod pairing;
pub mod secure_file;
pub mod session;
pub mod sync;
pub mod transport;

pub mod calls;
pub mod clipboard;
pub mod files;
pub mod notifications;
pub mod photos;
pub mod ring;
pub mod screen;
pub mod status;

pub use device::{DeviceInfo, DeviceKind, Feature, FeatureError};
pub use identity::{CertificateFingerprint, Identity};
pub use session::{Session, SessionConfig, SessionError, SessionEvent};
