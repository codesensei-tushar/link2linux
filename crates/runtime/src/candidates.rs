//! What discovery has shown about each paired phone, and from it the
//! addresses one dial attempt races (`docs/protocol.md`, "Choosing what to
//! dial").
//!
//! The rules, all pure so they are tested without a network:
//!
//! - the phone's last known address goes first and is dialed as stored: it
//!   answered before;
//! - addresses from mDNS records and beacons are only hints. They are dialed
//!   when they are on-link for one of this computer's interfaces, and never
//!   otherwise, so a forged announcement cannot steer the desktop to an
//!   arbitrary host;
//! - a beacon is fresh evidence for [`BEACON_FRESH_FOR`]; an mDNS record
//!   stays until it is withdrawn;
//! - an address the user typed is not a candidate here: it is dialed as
//!   given by the pairing code.

use std::collections::{BTreeMap, HashMap};
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use link2linux_core::discovery::onlink::{LocalAddress, is_on_link};
use link2linux_core::discovery::reconnect::dial_plan;

/// How long a beacon's source address counts as a place to dial.
pub const BEACON_FRESH_FOR: Duration = Duration::from_secs(2 * 60);

/// The addresses to try, each with the time after the start of the attempt
/// at which to start connecting.
pub type Plan = Vec<(SocketAddr, Duration)>;

#[derive(Debug, Default)]
struct PhoneEvidence {
    /// mDNS instance name to the addresses it announced.
    mdns: BTreeMap<String, Vec<SocketAddr>>,
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
        self.phones
            .entry(phone_id.to_owned())
            .or_default()
            .mdns
            .insert(instance.to_owned(), addresses);
    }

    /// Forgets an mDNS record that was withdrawn.
    pub fn withdrawn(&mut self, instance: &str) {
        for phone in self.phones.values_mut() {
            phone.mdns.remove(instance);
        }
    }

    /// Remembers where a phone's beacon came from.
    pub fn beacon(&mut self, phone_id: &str, address: SocketAddr, now: Instant) {
        let phone = self.phones.entry(phone_id.to_owned()).or_default();
        phone.beacons.retain(|(known, _)| *known != address);
        phone.beacons.push((address, now));
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
        let mdns: Vec<SocketAddr> = last_known
            .iter()
            .skip(1)
            .copied()
            .chain(
                evidence
                    .into_iter()
                    .flat_map(|phone| phone.mdns.values().flatten())
                    .copied()
                    .filter(on_link),
            )
            .collect();
        let beacons: Vec<SocketAddr> = evidence
            .into_iter()
            .flat_map(|phone| phone.beacons.iter().rev())
            .filter(|(_, at)| now.saturating_duration_since(*at) <= BEACON_FRESH_FOR)
            .map(|(address, _)| *address)
            .filter(on_link)
            .collect();
        dial_plan(last_known.first().copied(), &mdns, &beacons)
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
                addr(11, 27380),
                addr(20, 27380),
                addr(21, 4000)
            ]
        );
        assert_eq!(plan[0].1, Duration::ZERO, "the first starts at once");
        assert!(plan[1].1 > plan[0].1, "the rest are staggered");
    }

    #[test]
    fn the_last_known_address_skips_the_on_link_check() {
        let far = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 9, 9, 9)), 27380);
        let plan = Evidence::default().plan("p", &[far], &lan(), Instant::now());
        assert_eq!(addresses(&plan), [far]);
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
