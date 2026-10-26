//! What discovery has shown about each paired phone, and from it the
//! addresses one dial attempt races (`docs/protocol.md`, "Choosing what to
//! dial").
//!
//! The rules, all pure so they are tested without a network:
//!
//! - the phone's last known address goes first: it answered before. Last
//!   known addresses are dialed when they are on-link, private (an IPv4
//!   private range or an IPv6 unique local address), which a routed home
//!   or office network reaches, or loopback. A public address that is not
//!   on-link is not dialed: that would dial across the internet, towards
//!   whatever holds the address now;
//! - addresses from mDNS records and beacons are only hints. They are dialed
//!   when they are on-link for one of this computer's interfaces, and never
//!   otherwise, so a forged announcement cannot steer the desktop to an
//!   arbitrary host;
//! - the sources of fresh beacons come right after the first last known
//!   address: a beacon carries a tag only the phone can make, and a
//!   replayed one is refused. Then the other last known addresses, then at
//!   most [`MAX_MDNS_CANDIDATES`] mDNS addresses, newest record first, since
//!   a replayed presence tag can be announced under any number of names;
//! - a beacon is fresh evidence for [`BEACON_FRESH_FOR`]; an mDNS record
//!   stays until it is withdrawn;
//! - an address the user typed is not a candidate here: it is dialed as
//!   given by the pairing code.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};

use link2linux_core::discovery::onlink::{LocalAddress, is_on_link};
use link2linux_core::discovery::reconnect::ranked_plan;

/// How long a beacon's source address counts as a place to dial.
pub const BEACON_FRESH_FOR: Duration = Duration::from_secs(2 * 60);
/// The most mDNS records kept per phone, newest first. A phone announces
/// under one name at a time and renames itself now and then.
const MAX_MDNS_RECORDS: usize = 4;
/// The most beacon sources kept per phone, newest first.
const MAX_BEACONS: usize = 4;
/// The most mDNS addresses one plan dials.
pub const MAX_MDNS_CANDIDATES: usize = 6;

/// The addresses to try, each with the time after the start of the attempt
/// at which to start connecting.
pub type Plan = Vec<(SocketAddr, Duration)>;

#[derive(Debug, Default)]
struct PhoneEvidence {
    /// mDNS instance names and the addresses each announced, newest last.
    mdns: Vec<(String, Vec<SocketAddr>)>,
    /// Where beacons came from, newest last.
    beacons: Vec<(SocketAddr, Instant)>,
}

/// Discovery evidence per paired phone, by phone id.
#[derive(Debug, Default)]
pub struct Evidence {
    phones: HashMap<String, PhoneEvidence>,
}

impl Evidence {
    /// Remembers the addresses a phone announced in an mDNS record.
    pub fn mdns(&mut self, phone_id: &str, instance: &str, addresses: Vec<SocketAddr>) {
        let phone = self.phones.entry(phone_id.to_owned()).or_default();
        phone.mdns.retain(|(known, _)| known != instance);
        phone.mdns.push((instance.to_owned(), addresses));
        let extra = phone.mdns.len().saturating_sub(MAX_MDNS_RECORDS);
        phone.mdns.drain(..extra);
    }

    /// Forgets an mDNS record that was withdrawn.
    pub fn withdrawn(&mut self, instance: &str) {
        for phone in self.phones.values_mut() {
            phone.mdns.retain(|(known, _)| known != instance);
        }
    }

    /// Remembers where a phone's beacon came from.
    pub fn beacon(&mut self, phone_id: &str, address: SocketAddr, now: Instant) {
        let phone = self.phones.entry(phone_id.to_owned()).or_default();
        phone.beacons.retain(|(known, at)| {
            *known != address && now.saturating_duration_since(*at) <= BEACON_FRESH_FOR
        });
        phone.beacons.push((address, now));
        let extra = phone.beacons.len().saturating_sub(MAX_BEACONS);
        phone.beacons.drain(..extra);
    }

    /// Forgets everything about a phone, for example once it is unpaired.
    pub fn forget(&mut self, phone_id: &str) {
        self.phones.remove(phone_id);
    }

    /// The plan for one attempt to dial `phone_id`, whose record lists
    /// `last_known` (most recent first).
    #[must_use]
    pub fn plan(
        &self,
        phone_id: &str,
        last_known: &[SocketAddr],
        locals: &[LocalAddress],
        now: Instant,
    ) -> Plan {
        let evidence = self.phones.get(phone_id);
        let on_link = |address: &SocketAddr| is_on_link(*address, locals);
        let mut last_known = last_known
            .iter()
            .copied()
            .filter(|address| reachable_last_known(*address, locals));
        let first = last_known.next();
        let beacons = evidence
            .into_iter()
            .flat_map(|phone| phone.beacons.iter().rev())
            .filter(|(_, at)| now.saturating_duration_since(*at) <= BEACON_FRESH_FOR)
            .map(|(address, _)| *address)
            .filter(on_link);
        let mdns = evidence
            .into_iter()
            .flat_map(|phone| phone.mdns.iter().rev())
            .flat_map(|(_, addresses)| addresses)
            .copied()
            .filter(on_link)
            .take(MAX_MDNS_CANDIDATES);
        ranked_plan(
            first
                .into_iter()
                .chain(beacons)
                .chain(last_known)
                .chain(mdns),
        )
    }
}

/// Whether a last known address may be dialed now: it is on-link, a
/// private address, which a routed local network can reach, or loopback,
/// as for a phone emulator reached through a forwarded port.
fn reachable_last_known(address: SocketAddr, locals: &[LocalAddress]) -> bool {
    let ip = address.ip().to_canonical();
    is_on_link(address, locals)
        || ip.is_loopback()
        || match ip {
            IpAddr::V4(ip) => ip.is_private(),
            IpAddr::V6(ip) => ip.is_unique_local(),
        }
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr};

    use super::*;

    fn addr(last: u8, port: u16) -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, last)), port)
    }

    fn lan() -> Vec<LocalAddress> {
        vec![LocalAddress {
            ip: IpAddr::V4(Ipv4Addr::new(192, 168, 1, 5)),
            prefix_len: 24,
            interface_index: Some(2),
        }]
    }

    fn addresses(plan: &Plan) -> Vec<SocketAddr> {
        plan.iter().map(|(address, _)| *address).collect()
    }

    #[test]
    fn the_last_known_address_goes_first_and_others_follow() {
        let now = Instant::now();
        let mut evidence = Evidence::default();
        evidence.mdns("p", "l2l-1", vec![addr(20, 27380)]);
        evidence.beacon("p", addr(21, 4000), now);
        let plan = evidence.plan("p", &[addr(10, 27380), addr(11, 27380)], &lan(), now);
        assert_eq!(
            addresses(&plan),
            [
                addr(10, 27380),
                addr(21, 4000),
                addr(11, 27380),
                addr(20, 27380)
            ]
        );
        assert_eq!(plan[0].1, Duration::ZERO, "the first starts at once");
        assert!(plan[1].1 > plan[0].1, "the rest are staggered");
    }

    #[test]
    fn a_last_known_address_off_link_is_dialed_only_when_private() {
        let routed = [
            SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 9, 9, 9)), 27380),
            "[fd12:3456::7]:27380".parse().unwrap(),
            "127.0.0.1:27380".parse().unwrap(),
        ];
        let public = [
            SocketAddr::new(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 9)), 27380),
            "[2001:db8:9::7]:27380".parse().unwrap(),
        ];
        let known: Vec<SocketAddr> = public.iter().chain(&routed).copied().collect();
        let plan = Evidence::default().plan("p", &known, &lan(), Instant::now());
        assert_eq!(addresses(&plan), routed);

        // On-link, a public address is dialed too.
        let lan = [LocalAddress {
            ip: IpAddr::V4(Ipv4Addr::new(203, 0, 113, 5)),
            prefix_len: 24,
            interface_index: Some(2),
        }];
        let plan = Evidence::default().plan("p", &public[..1], &lan, Instant::now());
        assert_eq!(addresses(&plan), public[..1]);
    }

    #[test]
    fn fresh_beacons_come_before_a_flood_of_mdns_records() {
        let now = Instant::now();
        let mut evidence = Evidence::default();
        // A replayed presence tag, announced under many names.
        for i in 0..20 {
            evidence.mdns("p", &format!("a{i:02}"), vec![addr(100 + i, 22)]);
        }
        evidence.beacon("p", addr(21, 4000), now);
        let plan = addresses(&evidence.plan("p", &[addr(10, 27380)], &lan(), now));
        assert_eq!(plan[..2], [addr(10, 27380), addr(21, 4000)]);
        // Only the newest records are kept.
        assert_eq!(
            plan[2..],
            [addr(119, 22), addr(118, 22), addr(117, 22), addr(116, 22)]
        );
    }

    #[test]
    fn mdns_addresses_are_capped_and_beacons_kept_few() {
        let now = Instant::now();
        let mut evidence = Evidence::default();
        evidence.mdns("p", "l2l-1", (30..40).map(|i| addr(i, 27380)).collect());
        for i in 0..10 {
            evidence.beacon("p", addr(50 + i, 4000), now);
        }
        let plan = addresses(&evidence.plan("p", &[], &lan(), now));
        assert_eq!(plan.len(), MAX_BEACONS + MAX_MDNS_CANDIDATES);
        assert_eq!(
            plan[..MAX_BEACONS],
            [
                addr(59, 4000),
                addr(58, 4000),
                addr(57, 4000),
                addr(56, 4000)
            ]
        );
    }

    #[test]
    fn hints_from_off_link_hosts_are_never_dialed() {
        let now = Instant::now();
        let mut evidence = Evidence::default();
        let forged = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)), 27380);
        evidence.mdns("p", "l2l-1", vec![forged, addr(20, 27380)]);
        evidence.beacon("p", forged, now);
        let plan = evidence.plan("p", &[], &lan(), now);
        assert_eq!(addresses(&plan), [addr(20, 27380)]);
    }

    #[test]
    fn a_beacon_goes_stale_and_a_withdrawn_record_is_forgotten() {
        let start = Instant::now();
        let mut evidence = Evidence::default();
        evidence.beacon("p", addr(21, 4000), start);
        evidence.mdns("p", "l2l-1", vec![addr(20, 27380)]);
        let later = start + BEACON_FRESH_FOR + Duration::from_secs(1);
        assert_eq!(
            addresses(&evidence.plan("p", &[], &lan(), later)),
            [addr(20, 27380)]
        );
        evidence.withdrawn("l2l-1");
        assert!(evidence.plan("p", &[], &lan(), later).is_empty());
    }

    #[test]
    fn evidence_is_kept_per_phone_and_can_be_forgotten() {
        let now = Instant::now();
        let mut evidence = Evidence::default();
        evidence.mdns("a", "l2l-a", vec![addr(20, 1)]);
        evidence.mdns("b", "l2l-b", vec![addr(30, 1)]);
        assert_eq!(
            addresses(&evidence.plan("a", &[], &lan(), now)),
            [addr(20, 1)]
        );
        evidence.forget("a");
        assert!(evidence.plan("a", &[], &lan(), now).is_empty());
        assert_eq!(
            addresses(&evidence.plan("b", &[], &lan(), now)),
            [addr(30, 1)]
        );
    }

    #[test]
    fn duplicates_are_dialed_once() {
        let now = Instant::now();
        let mut evidence = Evidence::default();
        evidence.mdns("p", "l2l-1", vec![addr(10, 27380)]);
        evidence.beacon("p", addr(10, 27380), now);
        let plan = evidence.plan("p", &[addr(10, 27380)], &lan(), now);
        assert_eq!(plan.len(), 1);
    }
}
