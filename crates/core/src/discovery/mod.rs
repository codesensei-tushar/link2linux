//! Finding devices on the local network.
//!
//! The phone announces itself on the local network (mDNS, with UDP beacons
//! as a fallback) and the desktop listens for it, so the desktop can
//! list phones to pair with and dial paired ones when their address
//! changes. The desktop never listens for connections: it always dials the
//! phone (`docs/adr/0006-discovery-and-dialing.md`).
//!
//! Stub: the [`Discovery`] trait and its types are in place; the network
//! implementation comes next.

use std::net::SocketAddr;

pub mod beacon;
pub mod listener;
pub mod presence;

use crate::device::{DeviceInfo, DeviceKind};

/// A device seen on the local network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredDevice {
    /// The device's stable identifier, as in its `Hello`.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Desktop or phone.
    pub kind: DeviceKind,
    /// Where the device accepts connections. There can be several, for
    /// example one per network interface and address family.
    pub addresses: Vec<SocketAddr>,
}

/// A change in the set of devices seen on the network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscoveryEvent {
    /// A device appeared, or its addresses changed.
    Found(DiscoveredDevice),
    /// A device stopped announcing itself.
    Lost {
        /// The device's identifier.
        id: String,
    },
}

/// Why discovery failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DiscoveryError {
    /// The network does not allow discovery, for example no multicast.
    #[error("network discovery is unavailable: {0}")]
    Unavailable(String),
    /// A socket error.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Announces this device on the local network and watches for others.
pub trait Discovery: Send {
    /// Starts announcing `local` as reachable on `port`, and browsing for
    /// other devices.
    fn start(
        &mut self,
        local: &DeviceInfo,
        port: u16,
    ) -> impl Future<Output = Result<(), DiscoveryError>> + Send;

    /// Waits for the next change. Returns `None` once discovery has stopped.
    fn next_event(&mut self) -> impl Future<Output = Option<DiscoveryEvent>> + Send;

    /// Stops announcing and browsing.
    fn stop(&mut self) -> impl Future<Output = Result<(), DiscoveryError>> + Send;
}

/// Reader for the `name = value` test vectors in `proto/testdata/v1`.
#[cfg(test)]
pub(crate) mod testvec {
    use std::collections::HashMap;

    pub struct Vector(HashMap<String, String>);

    impl Vector {
        pub fn parse(text: &str) -> Self {
            Self(
                text.lines()
                    .filter(|l| !l.starts_with('#'))
                    .filter_map(|l| l.split_once(" = "))
                    .map(|(k, v)| (k.to_owned(), v.to_owned()))
                    .collect(),
            )
        }

        pub fn pairing() -> Self {
            Self::parse(include_str!("../../../../proto/testdata/v1/pairing.txt"))
        }

        pub fn names(&self) -> impl Iterator<Item = &str> {
            self.0.keys().map(String::as_str)
        }

        pub fn text(&self, name: &str) -> &str {
            self.0
                .get(name)
                .unwrap_or_else(|| panic!("no vector value {name}"))
        }

        pub fn bytes(&self, name: &str) -> Vec<u8> {
            let hex = self.text(name);
            (0..hex.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
                .collect()
        }

        pub fn array<const N: usize>(&self, name: &str) -> [u8; N] {
            self.bytes(name).try_into().expect("vector length")
        }
    }
}
