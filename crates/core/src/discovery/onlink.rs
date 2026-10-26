//! The on-link check before dialing an address that discovery reported.
//!
//! Anyone on the network can forge an mDNS record or a beacon, so the
//! desktop dials an address from discovery only when it is on-link for one
//! of its own interfaces (`docs/protocol.md`, "Choosing what to dial"): in
//! the IPv4 subnet of an interface, IPv6 link-local on a known interface,
//! or in the /64 prefix of an interface's IPv6 address. That keeps a forged
//! announcement from steering the desktop towards arbitrary hosts, including
//! itself. An address the user typed is dialed as given and skips the check.

use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

/// One address of one of this machine's interfaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalAddress {
    /// The interface's address.
    pub ip: IpAddr,
    /// The prefix length of its network, such as 24 for an IPv4 /24.
    pub prefix_len: u8,
    /// The interface's index, which scopes IPv6 link-local addresses.
    pub interface_index: Option<u32>,
}

/// The addresses of this machine's interfaces, without loopback. Call it
/// again after a network change: the list is a snapshot.
pub fn local_addresses() -> io::Result<Vec<LocalAddress>> {
    Ok(if_addrs::get_if_addrs()?
        .into_iter()
        .filter(|interface| !interface.is_loopback())
        .map(|interface| LocalAddress {
            ip: interface.ip(),
            prefix_len: match &interface.addr {
                if_addrs::IfAddr::V4(v4) => v4.prefixlen,
                if_addrs::IfAddr::V6(v6) => v6.prefixlen,
            },
            interface_index: interface.index,
        })
        .collect())
}

/// Whether `candidate`, an address from an mDNS record or a beacon, may be
/// dialed: it has a port, is on-link for one of `locals`, and is none of
/// `locals` itself, on any interface. An IPv6 link-local candidate must
/// carry the scope of the interface it was learned on.
#[must_use]
pub fn is_on_link(candidate: SocketAddr, locals: &[LocalAddress]) -> bool {
    if candidate.port() == 0 {
        return false;
    }
    // This machine, by any of its addresses: a second address in the same
    // subnet, a temporary IPv6 address, or its own link-local address.
    let ip = candidate.ip().to_canonical();
    if locals.iter().any(|local| local.ip.to_canonical() == ip) {
        return false;
    }
    match candidate {
        SocketAddr::V4(v4) => v4_on_link(*v4.ip(), locals),
        SocketAddr::V6(v6) => match v6.ip().to_ipv4_mapped() {
            Some(v4) => v4_on_link(v4, locals),
            None => v6_on_link(*v6.ip(), v6.scope_id(), locals),
        },
    }
}

fn v4_on_link(ip: Ipv4Addr, locals: &[LocalAddress]) -> bool {
    if ip.is_loopback() || ip.is_unspecified() || ip.is_broadcast() || ip.is_multicast() {
        return false;
    }
    locals.iter().any(|local| match local.ip {
        IpAddr::V4(own) if (1..=32).contains(&local.prefix_len) => {
            let host_bits = 32 - u32::from(local.prefix_len);
            let mask = u32::MAX.checked_shl(host_bits).unwrap_or(0);
            let (ip, own) = (u32::from(ip), u32::from(own));
            let host = ip & !mask;
            // The network and broadcast addresses of a subnet are not hosts,
            // except on /31 and /32 links, which have no such addresses.
            let is_host = host_bits <= 1 || (host != 0 && host != !mask);
            ip & mask == own & mask && is_host
        }
        _ => false,
    })
}

fn v6_on_link(ip: Ipv6Addr, scope_id: u32, locals: &[LocalAddress]) -> bool {
    if ip.is_loopback() || ip.is_unspecified() || ip.is_multicast() {
        return false;
    }
    if ip.is_unicast_link_local() {
        // A link-local address only means something on one interface.
        return scope_id != 0
            && locals
                .iter()
                .any(|local| local.ip.is_ipv6() && local.interface_index == Some(scope_id));
    }
    locals.iter().any(|local| match local.ip {
        // The spec uses the /64 whatever the interface's own prefix length,
        // since a DHCPv6 address can come with a /128.
        IpAddr::V6(own) if !own.is_unicast_link_local() => own.octets()[..8] == ip.octets()[..8],
        _ => false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddrV6;

    fn v4(ip: &str, prefix_len: u8) -> LocalAddress {
        LocalAddress {
            ip: ip.parse().unwrap(),
            prefix_len,
            interface_index: Some(2),
        }
    }

    fn v6(ip: &str, prefix_len: u8, index: u32) -> LocalAddress {
        LocalAddress {
            ip: ip.parse().unwrap(),
            prefix_len,
            interface_index: Some(index),
        }
    }

    fn at(address: &str) -> SocketAddr {
        address.parse().unwrap()
    }

    #[test]
    fn ipv4_must_be_a_host_in_an_interface_subnet() {
        let locals = [v4("192.168.1.20", 24), v4("10.0.0.5", 8)];
        assert!(is_on_link(at("192.168.1.42:27380"), &locals));
        assert!(is_on_link(at("10.200.3.4:27380"), &locals));
        for off_link in [
            "192.168.2.42:27380",  // another subnet
            "8.8.8.8:27380",       // the internet
            "192.168.1.0:27380",   // the network address
            "192.168.1.255:27380", // the subnet broadcast
            "255.255.255.255:27380",
            "127.0.0.1:27380",
            "224.0.0.251:27380",
            "0.0.0.0:27380",
            "192.168.1.20:27380", // the desktop itself
            "192.168.1.42:0",     // no port
        ] {
            assert!(!is_on_link(at(off_link), &locals), "{off_link}");
        }
    }

    #[test]
    fn point_to_point_links_have_no_network_or_broadcast_address() {
        let locals = [v4("100.64.0.0", 31)];
        assert!(is_on_link(at("100.64.0.1:27380"), &locals));
        assert!(!is_on_link(at("100.64.0.2:27380"), &locals));
        // A /32 has only the interface itself.
        assert!(!is_on_link(at("10.1.1.2:27380"), &[v4("10.1.1.1", 32)]));
    }

    #[test]
    fn mapped_ipv4_is_checked_as_ipv4() {
        let locals = [v4("192.168.1.20", 24)];
        assert!(is_on_link(at("[::ffff:192.168.1.42]:27380"), &locals));
        assert!(!is_on_link(at("[::ffff:8.8.8.8]:27380"), &locals));
    }

    #[test]
    fn ipv6_link_local_needs_the_scope_of_a_known_interface() {
        let locals = [v6("fe80::1", 64, 3), v6("2001:db8:1:2::10", 64, 3)];
        let ip: Ipv6Addr = "fe80::abcd".parse().unwrap();
        let scoped = |scope| SocketAddr::V6(SocketAddrV6::new(ip, 27380, 0, scope));
        assert!(is_on_link(scoped(3), &locals));
        assert!(!is_on_link(scoped(0), &locals), "no scope");
        assert!(!is_on_link(scoped(9), &locals), "unknown interface");
    }

    #[test]
    fn ipv6_must_share_a_64_with_an_interface() {
        // A /128 from DHCPv6 still makes its /64 on-link.
        let locals = [v6("2001:db8:1:2::10", 128, 3), v6("fe80::1", 64, 3)];
        assert!(is_on_link(at("[2001:db8:1:2:aaaa::7]:27380"), &locals));
        for off_link in [
            "[2001:db8:1:3::7]:27380",  // another /64
            "[2001:db8:1:2::10]:27380", // the desktop itself
            "[ff02::fb]:27380",
            "[::1]:27380",
            "[::]:27380",
        ] {
            assert!(!is_on_link(at(off_link), &locals), "{off_link}");
        }
        // Sharing only a link-local prefix doesn't count.
        assert!(!is_on_link(
            at("[2001:db8::7]:27380"),
            &[v6("fe80::1", 64, 3)]
        ));
    }

    #[test]
    fn none_of_the_desktops_own_addresses_is_on_link() {
        // A stable and a temporary IPv6 address in one /64, two IPv4
        // addresses in one subnet, a link-local address and another
        // interface's address.
        let locals = [
            v6("2001:db8:1:2::10", 64, 3),
            v6("2001:db8:1:2:5c1e:9a2b:44d0:7e11", 64, 3),
            v6("fe80::1", 64, 3),
            v4("192.168.1.20", 24),
            v4("192.168.1.21", 24),
            v4("10.0.0.5", 8),
        ];
        let link_local: Ipv6Addr = "fe80::1".parse().unwrap();
        for own in [
            at("[2001:db8:1:2::10]:631"),
            at("[2001:db8:1:2:5c1e:9a2b:44d0:7e11]:631"),
            SocketAddr::V6(SocketAddrV6::new(link_local, 631, 0, 3)),
            at("192.168.1.20:631"),
            at("192.168.1.21:631"),
            at("[::ffff:192.168.1.21]:631"),
            at("10.0.0.5:631"),
        ] {
            assert!(!is_on_link(own, &locals), "{own}");
        }
        assert!(is_on_link(at("192.168.1.22:27380"), &locals));
        assert!(is_on_link(at("[2001:db8:1:2::11]:27380"), &locals));
    }

    #[test]
    fn no_interfaces_means_nothing_is_on_link() {
        assert!(!is_on_link(at("192.168.1.42:27380"), &[]));
    }

    #[test]
    fn this_machines_addresses_leave_out_loopback() {
        let locals = local_addresses().unwrap();
        assert!(locals.iter().all(|local| !local.ip.is_loopback()));
    }
}
