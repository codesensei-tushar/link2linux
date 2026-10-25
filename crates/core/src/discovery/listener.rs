//! The UDP sockets that receive beacons.
//!
//! The desktop listens on the beacon port on all IPv4 addresses and, in a
//! second socket, on all IPv6 addresses (IPv6 only, so the two do not
//! overlap). Both set address reuse, so the desktop coexists with other
//! listeners on the machine, such as another user's session.
//!
//! The listener only delivers datagrams and their source. Checking them is
//! [`BeaconVerifier`](super::beacon::BeaconVerifier)'s job.

use std::future::pending;
use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};

use socket2::{Domain, Protocol, Socket, Type};
use tokio::net::UdpSocket;

/// Larger than any beacon, so an oversized datagram is seen as such
/// instead of being cut to a valid length.
const RECV_BUFFER: usize = 128;

/// One datagram and where it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Datagram {
    /// The payload, at most 128 bytes (longer ones are cut and are never
    /// valid beacons anyway).
    pub data: Vec<u8>,
    /// The sender. IPv4-mapped IPv6 addresses are shown as IPv4.
    pub source: SocketAddr,
}

/// Receives beacons on IPv4 and IPv6.
#[derive(Debug)]
pub struct BeaconListener {
    v4: Option<UdpSocket>,
    v6: Option<UdpSocket>,
}

impl BeaconListener {
    /// Binds `port` on `0.0.0.0` and `[::]`. With port 0 the system picks
    /// one for IPv4 and IPv6 uses the same.
    ///
    /// A family the system does not support is skipped with a warning.
    ///
    /// # Errors
    ///
    /// Fails only when neither socket can be bound; the error is the IPv4
    /// one.
    pub fn bind(port: u16) -> io::Result<Self> {
        let v4 = bind_socket(SocketAddr::V4(SocketAddrV4::new(
            Ipv4Addr::UNSPECIFIED,
            port,
        )));
        let port = match &v4 {
            Ok(socket) => socket.local_addr()?.port(),
            Err(_) => port,
        };
        let v6 = bind_socket(SocketAddr::V6(SocketAddrV6::new(
            Ipv6Addr::UNSPECIFIED,
            port,
            0,
            0,
        )));
        match (v4, v6) {
            (Err(e4), Err(e6)) => {
                tracing::warn!(%e6, "cannot listen for beacons on IPv6");
                Err(e4)
            }
            (v4, v6) => {
                if let Err(e) = &v4 {
                    tracing::warn!(%e, "cannot listen for beacons on IPv4");
                }
                if let Err(e) = &v6 {
                    tracing::warn!(%e, "cannot listen for beacons on IPv6");
                }
                Ok(Self {
                    v4: v4.ok(),
                    v6: v6.ok(),
                })
            }
        }
    }

    /// The addresses the sockets are bound to.
    #[must_use]
    pub fn local_addrs(&self) -> Vec<SocketAddr> {
        [&self.v4, &self.v6]
            .into_iter()
            .flatten()
            .filter_map(|socket| socket.local_addr().ok())
            .collect()
    }

    /// Waits for the next datagram on either socket. Safe to cancel.
    ///
    /// # Errors
    ///
    /// Returns the socket error. Some errors (for example ICMP errors
    /// reported on a later receive) are transient; callers keep calling.
    pub async fn recv(&self) -> io::Result<Datagram> {
        let mut buf4 = [0u8; RECV_BUFFER];
        let mut buf6 = [0u8; RECV_BUFFER];
        let (len, source, buf) = tokio::select! {
            r = recv_on(self.v4.as_ref(), &mut buf4) => r.map(|(len, source)| (len, source, &buf4))?,
            r = recv_on(self.v6.as_ref(), &mut buf6) => r.map(|(len, source)| (len, source, &buf6))?,
        };
        Ok(Datagram {
            data: buf[..len].to_vec(),
            source: unmap(source),
        })
    }
}

async fn recv_on(socket: Option<&UdpSocket>, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
    match socket {
        Some(socket) => socket.recv_from(buf).await,
        None => pending().await,
    }
}

fn bind_socket(addr: SocketAddr) -> io::Result<UdpSocket> {
    let socket = Socket::new(Domain::for_address(addr), Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_reuse_address(true)?;
    if addr.is_ipv6() {
        socket.set_only_v6(true)?;
    }
    socket.set_nonblocking(true)?;
    socket.bind(&addr.into())?;
    UdpSocket::from_std(socket.into())
}

/// Shows an IPv4-mapped IPv6 address (`::ffff:a.b.c.d`) as IPv4.
fn unmap(addr: SocketAddr) -> SocketAddr {
    match addr {
        SocketAddr::V6(v6) => match v6.ip().to_ipv4_mapped() {
            Some(v4) => SocketAddr::new(v4.into(), v6.port()),
            None => addr,
        },
        SocketAddr::V4(_) => addr,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    async fn recv_soon(listener: &BeaconListener) -> Datagram {
        tokio::time::timeout(Duration::from_secs(5), listener.recv())
            .await
            .expect("a datagram within 5 s")
            .unwrap()
    }

    fn port_of(listener: &BeaconListener) -> u16 {
        listener.local_addrs()[0].port()
    }

    #[test]
    fn maps_ipv4_mapped_sources_back() {
        let mapped: SocketAddr = "[::ffff:192.168.1.7]:4000".parse().unwrap();
        assert_eq!(unmap(mapped), "192.168.1.7:4000".parse().unwrap());
        let plain: SocketAddr = "[fe80::1]:4000".parse().unwrap();
        assert_eq!(unmap(plain), plain);
    }

    #[tokio::test]
    async fn receives_unicast_on_ipv4() {
        let listener = BeaconListener::bind(0).unwrap();
        let sender = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        sender
            .send_to(b"hello", ("127.0.0.1", port_of(&listener)))
            .await
            .unwrap();
        let got = recv_soon(&listener).await;
        assert_eq!(got.data, b"hello");
        assert_eq!(got.source, sender.local_addr().unwrap());
    }

    #[tokio::test]
    async fn receives_unicast_on_ipv6_when_available() {
        let listener = BeaconListener::bind(0).unwrap();
        let Ok(sender) = UdpSocket::bind("[::1]:0").await else {
            return; // no IPv6 loopback here
        };
        if listener.local_addrs().len() < 2 {
            return;
        }
        sender
            .send_to(b"six", ("::1", port_of(&listener)))
            .await
            .unwrap();
        let got = recv_soon(&listener).await;
        assert_eq!(got.data, b"six");
        assert_eq!(got.source, sender.local_addr().unwrap());
    }

    #[tokio::test]
    async fn two_listeners_can_share_the_port() {
        let first = BeaconListener::bind(0).unwrap();
        let second = BeaconListener::bind(port_of(&first));
        assert!(second.is_ok());
    }

    #[tokio::test]
    async fn oversized_datagrams_stay_oversized() {
        let listener = BeaconListener::bind(0).unwrap();
        let sender = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        sender
            .send_to(&[1u8; 200], ("127.0.0.1", port_of(&listener)))
            .await
            .unwrap();
        assert_eq!(recv_soon(&listener).await.data.len(), RECV_BUFFER);
    }

    #[tokio::test]
    async fn receives_broadcast_on_loopback() {
        let listener = BeaconListener::bind(0).unwrap();
        let sender = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        sender.set_broadcast(true).unwrap();
        sender
            .send_to(b"all", ("127.255.255.255", port_of(&listener)))
            .await
            .unwrap();
        assert_eq!(recv_soon(&listener).await.data, b"all");
    }
}
