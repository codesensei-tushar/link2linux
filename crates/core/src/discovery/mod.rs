//! Finding phones on the local network.
//!
//! Discovery only ever flows from the phone to the desktop: the phone
//! announces itself over mDNS and sends UDP beacons, and the desktop
//! listens, so it can list phones to pair with and dial paired ones when
//! their address changes. The desktop never listens for connections and
//! never announces itself over mDNS (`docs/adr/0006-discovery-and-dialing.md`).
//!
//! - [`presence`]: the tags and the TXT record that carry them;
//! - [`beacon`] and [`listener`]: the UDP beacon codec and its sockets;
//! - [`browser`] and [`tracker`]: browsing mDNS and deciding which phones
//!   matter;
//! - [`reconnect`]: when to dial a paired phone again;
//! - [`lan`]: [`LanDiscovery`], which joins all of the above behind the
//!   [`Discovery`] trait.
//!
//! Everything discovery reports is a hint that only leads to a dial
//! attempt. Before dialing an address from a record or beacon, the dialer
//! must check that it is on-link for one of the desktop's interfaces
//! (`docs/protocol.md`, "Choosing what to dial"); that check needs the
//! interface list and belongs to the dialer.

use std::net::SocketAddr;

use tokio::sync::watch;

pub mod beacon;
pub mod browser;
pub mod lan;
pub mod listener;
pub mod presence;
pub mod reconnect;
pub mod tracker;

pub use beacon::BeaconSender;
pub use lan::{LanConfig, LanDiscovery};
pub use presence::{DiscoveryKeys, PairedKey, SecretBytes};
pub use tracker::{Match, PhoneRecord, Sighting};

/// A phone's beacon that passed every check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BeaconSighting {
    /// Whose key verified it: a paired phone's id, or the QR offer.
    pub sender: BeaconSender<String>,
    /// Where to dial: the datagram's source address at the beacon's port.
    pub address: SocketAddr,
}

/// A change in what discovery knows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscoveryEvent {
    /// A phone that matters appeared over mDNS.
    Found(Sighting),
    /// A phone already found announced again, or its matches changed.
    /// An announcement is fresh evidence that the phone is reachable.
    Updated(Sighting),
    /// A phone that mattered was withdrawn or stopped matching.
    Lost {
        /// Its mDNS instance name.
        instance: String,
    },
    /// A beacon arrived from a phone that matters.
    Beacon(BeaconSighting),
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

/// Watches the local network for phones. It only browses and listens; it
/// announces nothing.
pub trait Discovery: Send {
    /// Starts browsing and listening. `keys` always holds the current
    /// keys of the paired phones and the active QR offer; discovery
    /// re-checks what it has seen whenever they change.
    fn start(
        &mut self,
        keys: watch::Receiver<DiscoveryKeys>,
    ) -> impl Future<Output = Result<(), DiscoveryError>> + Send;

    /// Waits for the next change. Returns `None` once discovery has
    /// stopped. Safe to cancel.
    fn next_event(&mut self) -> impl Future<Output = Option<DiscoveryEvent>> + Send;

    /// Stops browsing and listening.
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
