//! The pairing state machine's data: the phase, what the UI is shown for it,
//! the phones found in pairing mode, and how errors map to what the user
//! reads.

use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use link2linux_core::discovery::LocalAddress;
use link2linux_core::discovery::onlink::is_on_link;
use link2linux_core::identity::CertificateFingerprint;
use link2linux_core::pairing::{OfferStatus, PairingError, PairingOutcome, QrOffer};
use tokio::sync::oneshot;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::view::{
    BackendError, Device, DiscoveredPhone, PairingFailure, PairingMethod, PairingState, QrImage,
};

/// The port phones listen on unless the user types another.
pub const DEFAULT_PHONE_PORT: u16 = 27380;
/// A new QR offer replaces the old one this long before it expires.
pub const OFFER_REFRESH_BEFORE: Duration = Duration::from_secs(20);
/// Offers shown before giving up with `expired`.
pub const MAX_OFFERS: u8 = 3;
/// How long a code comparison can take.
pub const SAS_LIFETIME: Duration = Duration::from_secs(60);
/// Code comparisons allowed per activation.
pub const SAS_MAX_ATTEMPTS: u8 = 3;
/// Most pairing connections an activation has open at once. Anyone on the
/// network can repeat the pairing tag of a QR offer, so every sighting of
/// it may lead to a host that stalls.
pub const MAX_CONNECTIONS: usize = 4;

/// Where a pairing activation stands.
#[derive(Debug)]
pub enum Phase {
    Idle,
    /// The QR code ([`PairingCtl::code`]) is showing.
    Qr {
        /// Why the last connection the user asked for ended.
        last_failure: Option<PairingFailure>,
    },
    Searching {
        last_failure: Option<PairingFailure>,
    },
    Connecting {
        method: PairingMethod,
        phone: Option<String>,
    },
    Verifying {
        method: PairingMethod,
        phone: Option<String>,
    },
    Sas {
        code: String,
        phone: String,
        expires: Instant,
        confirmed_here: bool,
    },
    Paired(Device),
    Failed {
        method: PairingMethod,
        reason: PairingFailure,
    },
}

/// The QR code of an activation. It stays while connections run, since a
/// failed one does not end the offer.
#[derive(Debug)]
pub struct Code {
    pub svg: QrImage,
    pub expires: Instant,
    /// Which offer of [`MAX_OFFERS`] this is.
    pub number: u8,
}

impl Phase {
    /// Whether a new activation may start.
    pub fn is_startable(&self) -> bool {
        matches!(self, Self::Idle | Self::Paired(_) | Self::Failed { .. })
    }
}

/// What led to a pairing connection, from the weakest sign that the phone
/// that scanned the QR code is there to the strongest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Lead {
    /// An mDNS record with the offer's pairing tag. The tag is sent in
    /// clear, so anyone on the network can repeat it.
    Announcement,
    /// A pairing beacon. Its tag verified with the offer's secret, and the
    /// replay check admits each beacon once.
    Beacon,
    /// The user picked or typed the phone.
    Asked,
}

/// A pairing connection of the current activation.
#[derive(Debug)]
pub struct Attempt {
    pub id: u64,
    pub lead: Lead,
    /// The addresses it dials, and once connected only the one it reached.
    pub addresses: Vec<IpAddr>,
    /// The key the peer proved in the TLS handshake.
    pub peer: Option<CertificateFingerprint>,
    /// The peer proved it knows the QR code's secret.
    pub proven: bool,
    /// Ends this connection alone.
    pub cancel: CancellationToken,
}

/// Everything about the current activation.
pub struct PairingCtl {
    /// Changes whenever an activation ends, so late results of an old one
    /// are dropped.
    pub generation: u64,
    pub phase: Phase,
    pub attempts_left: u8,
    /// Answers the question "do the codes match?".
    pub answer: Option<oneshot::Sender<bool>>,
    /// Ends the tasks of the activation.
    pub token: CancellationToken,
    /// The QR offer being shown.
    pub offer: Option<Arc<Mutex<QrOffer>>>,
    /// Its code.
    pub code: Option<Code>,
    /// The pairing connections, oldest first: up to [`MAX_CONNECTIONS`]
    /// for a QR offer, one for a code comparison.
    pub attempts: Vec<Attempt>,
    /// The id of the next connection.
    next_attempt: u64,
}

impl Default for PairingCtl {
    fn default() -> Self {
        Self {
            generation: 0,
            phase: Phase::Idle,
            attempts_left: SAS_MAX_ATTEMPTS,
            answer: None,
            token: CancellationToken::new(),
            offer: None,
            code: None,
            attempts: Vec::new(),
            next_attempt: 0,
        }
    }
}

impl PairingCtl {
    /// Ends the current activation and starts a new generation.
    pub fn reset(&mut self) {
        self.generation += 1;
        self.token.cancel();
        self.token = CancellationToken::new();
        self.answer = None;
        self.offer = None;
        self.code = None;
        self.attempts.clear();
    }

    /// What the UI is shown at `now`.
    pub fn view(&self, now: Instant, desktop_name: &str, phones: &PhoneList) -> PairingState {
        let left = |expires: Instant| {
            u32::try_from(expires.saturating_duration_since(now).as_millis()).unwrap_or(u32::MAX)
        };
        match &self.phase {
            Phase::Idle => PairingState::Idle,
            Phase::Qr { last_failure } => match &self.code {
                Some(code) => PairingState::QrOffer {
                    svg: code.svg.clone(),
                    offer_number: code.number,
                    desktop_name: desktop_name.to_owned(),
                    expires_in_ms: left(code.expires),
                    lifetime_ms: u32::try_from(
                        link2linux_core::pairing::OFFER_LIFETIME.as_millis(),
                    )
                    .unwrap_or(u32::MAX),
                    last_failure: *last_failure,
                },
                None => PairingState::Idle,
            },
            Phase::Searching { last_failure } => PairingState::Searching {
                phones: phones.phones(),
                attempts_left: self.attempts_left,
                last_failure: *last_failure,
            },
            Phase::Connecting { method, phone } => PairingState::Connecting {
                method: *method,
                phone_name: phone.clone(),
            },
            Phase::Verifying { method, phone } => PairingState::Verifying {
                method: *method,
                phone_name: phone.clone(),
            },
            Phase::Sas {
                code,
                phone,
                expires,
                confirmed_here,
            } => PairingState::SasCode {
                code: code.clone(),
                phone_name: phone.clone(),
                expires_in_ms: left(*expires),
                lifetime_ms: u32::try_from(SAS_LIFETIME.as_millis()).unwrap_or(u32::MAX),
                confirmed_here: *confirmed_here,
            },
            Phase::Paired(device) => PairingState::Paired {
                device: device.clone(),
            },
            Phase::Failed { method, reason } => PairingState::Failed {
                method: *method,
                reason: *reason,
            },
        }
    }

    /// Whether a pairing connection is open to `address`, or dials it.
    pub fn is_dialing(&self, address: IpAddr) -> bool {
        self.attempts
            .iter()
            .any(|attempt| attempt.addresses.contains(&address))
    }

    /// The addresses of a phone that announced the QR offer's tag that get
    /// a pairing connection: on-link ones, unless a connection to them is
    /// open or a peer there failed for this offer. Connections to others
    /// may be running, since anyone can copy the tag.
    pub fn offer_targets(
        &self,
        addresses: &[SocketAddr],
        locals: &[LocalAddress],
    ) -> Vec<SocketAddr> {
        let Some(offer) = &self.offer else {
            return Vec::new();
        };
        let Ok(offer) = offer.lock() else {
            return Vec::new();
        };
        addresses
            .iter()
            .copied()
            .filter(|address| {
                is_on_link(*address, locals)
                    && !self.is_dialing(address.ip())
                    && offer.may_dial(address.ip())
            })
            .collect()
    }

    /// Whether a connection that `lead` led to may open: below
    /// [`MAX_CONNECTIONS`], or by closing the oldest connection whose peer
    /// proved nothing and that weaker evidence led to. So copies of the
    /// pairing tag can hold every slot against each other, but not against
    /// the beacons of the phone that scanned the code. Closing one counts
    /// as no failure: its peer may be that phone too.
    pub fn make_room(&mut self, lead: Lead) -> bool {
        if self.attempts.len() < MAX_CONNECTIONS {
            return true;
        }
        let Some(oldest) = self
            .attempts
            .iter()
            .position(|attempt| !attempt.proven && attempt.lead < lead)
        else {
            return false;
        };
        self.attempts.remove(oldest).cancel.cancel();
        true
    }

    /// Records a new connection to `addresses`: its id, and the token that
    /// ends it alone.
    pub fn add_attempt(&mut self, lead: Lead, addresses: Vec<IpAddr>) -> (u64, CancellationToken) {
        let id = self.next_attempt;
        self.next_attempt += 1;
        let cancel = self.token.child_token();
        self.attempts.push(Attempt {
            id,
            lead,
            addresses,
            peer: None,
            proven: false,
            cancel: cancel.clone(),
        });
        (id, cancel)
    }

    /// Connection `id` reached `peer` at `address`, which frees the other
    /// addresses it dialed. False when it is to close instead: it was
    /// ended, or another connection already reached the same phone. A
    /// phone takes the latest connection for the pairing and declines the
    /// ones before, so it keeps the first only while it sees no second.
    pub fn connected(&mut self, id: u64, address: IpAddr, peer: CertificateFingerprint) -> bool {
        if self
            .attempts
            .iter()
            .any(|attempt| attempt.id != id && attempt.peer == Some(peer))
        {
            return false;
        }
        let Some(attempt) = self.attempts.iter_mut().find(|attempt| attempt.id == id) else {
            return false;
        };
        attempt.addresses = vec![address];
        attempt.peer = Some(peer);
        true
    }

    /// The peer of connection `id` proved it knows the QR code's secret.
    pub fn proven(&mut self, id: u64) {
        if let Some(attempt) = self.attempts.iter_mut().find(|attempt| attempt.id == id) {
            attempt.proven = true;
        }
    }

    /// Connection `id` ended. `None` when it was closed before.
    pub fn finished(&mut self, id: u64) -> Option<Attempt> {
        let index = self.attempts.iter().position(|attempt| attempt.id == id)?;
        Some(self.attempts.remove(index))
    }

    /// A code comparison attempt ended. Go back to the list while attempts
    /// remain; the user has to start again once they are used up.
    pub fn back_to_search(&mut self, reason: PairingFailure) {
        self.phase = if self.attempts_left == 0 {
            Phase::Failed {
                method: PairingMethod::Sas,
                reason: PairingFailure::TooManyAttempts,
            }
        } else {
            Phase::Searching {
                last_failure: Some(reason),
            }
        };
    }
}

/// Phones that announce pairing mode, by mDNS instance name.
#[derive(Debug, Default)]
pub struct PhoneList {
    entries: Vec<(String, Vec<SocketAddr>)>,
}

impl PhoneList {
    /// Adds a phone or updates its addresses.
    pub fn upsert(&mut self, instance: &str, addresses: Vec<SocketAddr>) {
        match self.entries.iter_mut().find(|(known, _)| known == instance) {
            Some((_, known)) => *known = addresses,
            None => self.entries.push((instance.to_owned(), addresses)),
        }
    }

    /// Removes a phone that stopped announcing. Returns whether it was
    /// listed.
    pub fn remove(&mut self, instance: &str) -> bool {
        let before = self.entries.len();
        self.entries.retain(|(known, _)| known != instance);
        self.entries.len() != before
    }

    /// The phones, for showing. The instance name is what the phone shows
    /// on its own screen, so it is both the id and the name.
    pub fn phones(&self) -> Vec<DiscoveredPhone> {
        self.entries
            .iter()
            .map(|(instance, addresses)| DiscoveredPhone {
                id: instance.clone(),
                name: instance.clone(),
                address: addresses
                    .first()
                    .map(|address| address.to_string())
                    .unwrap_or_default(),
            })
            .collect()
    }

    /// The addresses of the phone with this id.
    pub fn addresses(&self, id: &str) -> Option<Vec<SocketAddr>> {
        self.entries
            .iter()
            .find(|(instance, _)| instance == id)
            .map(|(_, addresses)| addresses.clone())
    }
}

/// Reads an address the user typed: `192.168.1.20`, `192.168.1.20:27380`,
/// `fd00::2` or `[fd00::2]:27380`. The default port is 27380.
pub fn parse_address(text: &str) -> Result<SocketAddr, BackendError> {
    let text = text.trim();
    if let Ok(address) = text.parse::<SocketAddr>() {
        return Ok(address);
    }
    text.trim_start_matches('[')
        .trim_end_matches(']')
        .parse::<IpAddr>()
        .map(|ip| SocketAddr::new(ip, DEFAULT_PHONE_PORT))
        .map_err(|_| BackendError::InvalidAddress)
}

/// What the user reads for an error of pairing `method`.
pub fn failure_of(error: &PairingError, method: PairingMethod) -> PairingFailure {
    match error {
        PairingError::Declined => match method {
            // The user said the codes differ.
            PairingMethod::Sas => PairingFailure::VerificationFailed,
            PairingMethod::QrCode => PairingFailure::Rejected,
        },
        PairingError::TimedOut => PairingFailure::TimedOut,
        PairingError::ReplaceDeclined => PairingFailure::AlreadyPaired,
        PairingError::Rejected(outcome) => match outcome {
            PairingOutcome::Declined => PairingFailure::DeclinedByPhone,
            PairingOutcome::VerificationFailed => PairingFailure::VerificationFailed,
            PairingOutcome::NotExpected => PairingFailure::NotExpected,
            PairingOutcome::TimedOut => PairingFailure::TimedOut,
            PairingOutcome::Unsupported => PairingFailure::Unsupported,
            _ => PairingFailure::Rejected,
        },
        PairingError::VerificationFailed => PairingFailure::VerificationFailed,
        PairingError::OfferUnavailable(OfferStatus::Expired) => PairingFailure::Expired,
        PairingError::OfferUnavailable(_) | PairingError::Blocked => PairingFailure::NotExpected,
        PairingError::Exporter(_) => PairingFailure::Unsupported,
        PairingError::Closed
        | PairingError::Connection(_)
        | PairingError::OutOfOrder(_)
        | PairingError::Protocol(_) => PairingFailure::ConnectionLost,
        _ => PairingFailure::Rejected,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_addresses_get_the_default_port() {
        let ok = |text: &str| parse_address(text).unwrap().to_string();
        assert_eq!(ok("192.168.1.20"), "192.168.1.20:27380");
        assert_eq!(ok(" 192.168.1.20:4000 "), "192.168.1.20:4000");
        assert_eq!(ok("fd00::2"), "[fd00::2]:27380");
        assert_eq!(ok("[fd00::2]"), "[fd00::2]:27380");
        assert_eq!(ok("[fd00::2]:5"), "[fd00::2]:5");
        for bad in ["", "phone.local", "192.168.1", "1.2.3.4:x"] {
            assert_eq!(
                parse_address(bad),
                Err(BackendError::InvalidAddress),
                "{bad}"
            );
        }
    }

    #[test]
    fn errors_map_to_what_the_user_reads() {
        use PairingFailure as F;
        use PairingMethod as M;
        let of = failure_of;
        assert_eq!(of(&PairingError::Declined, M::Sas), F::VerificationFailed);
        assert_eq!(of(&PairingError::Declined, M::QrCode), F::Rejected);
        assert_eq!(
            of(&PairingError::Rejected(PairingOutcome::Declined), M::Sas),
            F::DeclinedByPhone
        );
        assert_eq!(
            of(
                &PairingError::Rejected(PairingOutcome::NotExpected),
                M::QrCode
            ),
            F::NotExpected
        );
        assert_eq!(
            of(
                &PairingError::OfferUnavailable(OfferStatus::Expired),
                M::QrCode
            ),
            F::Expired
        );
        assert_eq!(of(&PairingError::Blocked, M::QrCode), F::NotExpected);
        assert_eq!(of(&PairingError::ReplaceDeclined, M::Sas), F::AlreadyPaired);
        assert_eq!(of(&PairingError::Closed, M::Sas), F::ConnectionLost);
        assert_eq!(
            of(&PairingError::VerificationFailed, M::QrCode),
            F::VerificationFailed
        );
        assert_eq!(
            of(&PairingError::Exporter("x".into()), M::QrCode),
            F::Unsupported
        );
    }

    #[test]
    fn the_phone_list_tracks_announcements() {
        let a: SocketAddr = "192.168.1.9:27380".parse().unwrap();
        let b: SocketAddr = "192.168.1.10:27380".parse().unwrap();
        let mut list = PhoneList::default();
        list.upsert("l2l-1", vec![a]);
        list.upsert("l2l-2", vec![b]);
        list.upsert("l2l-1", vec![b]);
        let phones = list.phones();
        assert_eq!(phones.len(), 2);
        assert_eq!(phones[0].address, "192.168.1.10:27380");
        assert_eq!(list.addresses("l2l-2"), Some(vec![b]));
        assert!(list.remove("l2l-1") && !list.remove("l2l-1"));
        assert_eq!(list.addresses("l2l-1"), None);
    }

    #[test]
    fn attempts_run_out_after_the_last_comparison() {
        let mut ctl = PairingCtl {
            attempts_left: 1,
            ..PairingCtl::default()
        };
        ctl.back_to_search(PairingFailure::TimedOut);
        assert!(matches!(
            ctl.phase,
            Phase::Searching {
                last_failure: Some(PairingFailure::TimedOut)
            }
        ));
        ctl.attempts_left = 0;
        ctl.back_to_search(PairingFailure::TimedOut);
        assert!(matches!(
            ctl.phase,
            Phase::Failed {
                reason: PairingFailure::TooManyAttempts,
                ..
            }
        ));
    }

    #[test]
    fn sightings_of_the_offer_are_dialed_while_others_connect() {
        use link2linux_core::identity::CertificateFingerprint;

        let at = |last: u8| SocketAddr::from(([192, 168, 1, last], 27380));
        let locals = [LocalAddress {
            ip: IpAddr::from([192, 168, 1, 2]),
            prefix_len: 24,
            interface_index: None,
        }];
        let mut ctl = PairingCtl::default();
        assert!(ctl.offer_targets(&[at(9)], &locals).is_empty());

        let offer = QrOffer::new(CertificateFingerprint([1; 32]), "Desk", Vec::new(), None);
        ctl.offer = Some(Arc::new(Mutex::new(offer)));
        // One peer is still connecting, another failed: a third is dialed.
        ctl.add_attempt(Lead::Announcement, vec![at(9).ip()]);
        let failed = CertificateFingerprint([2; 32]);
        if let Some(offer) = &ctl.offer {
            offer
                .lock()
                .unwrap()
                .record_failed_proof(failed, at(10).ip());
        }
        let off_link = SocketAddr::from(([10, 0, 0, 7], 27380));
        let seen = [at(9), at(10), at(11), off_link];
        assert_eq!(ctl.offer_targets(&seen, &locals), [at(11)]);
    }

    #[test]
    fn a_reset_starts_a_new_generation_and_cancels_the_old_tasks() {
        let mut ctl = PairingCtl::default();
        let old = ctl.token.clone();
        let (_, connection) = ctl.add_attempt(Lead::Beacon, vec![IpAddr::from([192, 168, 1, 9])]);
        ctl.reset();
        assert!(old.is_cancelled() && !ctl.token.is_cancelled());
        assert!(connection.is_cancelled());
        assert_eq!(ctl.generation, 1);
        assert!(ctl.attempts.is_empty());
    }

    fn ip(last: u8) -> IpAddr {
        IpAddr::from([192, 168, 1, last])
    }

    #[test]
    fn copied_announcements_hold_no_more_than_their_slots() {
        let mut ctl = PairingCtl::default();
        let mut tokens = Vec::new();
        for last in 10..10 + MAX_CONNECTIONS as u8 {
            assert!(ctl.make_room(Lead::Announcement));
            tokens.push(ctl.add_attempt(Lead::Announcement, vec![ip(last)]));
        }
        // A copied tag waits for a free slot.
        assert!(!ctl.make_room(Lead::Announcement));
        assert_eq!(ctl.attempts.len(), MAX_CONNECTIONS);

        // A beacon of the phone that scanned the code closes the oldest
        // connection an announcement led to, whose peer proved nothing.
        ctl.proven(tokens[0].0);
        assert!(ctl.make_room(Lead::Beacon));
        assert!(!tokens[0].1.is_cancelled(), "a proven peer stays");
        assert!(tokens[1].1.is_cancelled());
        assert!(!ctl.is_dialing(ip(11)));
        assert_eq!(ctl.attempts.len(), MAX_CONNECTIONS - 1);
        ctl.add_attempt(Lead::Beacon, vec![ip(20)]);
        assert!(ctl.make_room(Lead::Beacon));
        ctl.add_attempt(Lead::Beacon, vec![ip(21)]);
        assert!(ctl.make_room(Lead::Beacon));
        ctl.add_attempt(Lead::Beacon, vec![ip(22)]);
        // Beacons do not close each other.
        assert!(!ctl.make_room(Lead::Beacon));
        assert!(!ctl.make_room(Lead::Announcement));

        // A connection that ends frees its slot.
        let first_beacon = ctl.attempts[1].id;
        assert!(ctl.finished(first_beacon).is_some());
        assert!(ctl.finished(first_beacon).is_none());
        assert!(ctl.make_room(Lead::Announcement));
    }

    #[test]
    fn a_connection_frees_what_it_did_not_reach_and_one_phone_gets_one() {
        let at = |last: u8| SocketAddr::new(ip(last), 27380);
        let locals = [LocalAddress {
            ip: ip(2),
            prefix_len: 24,
            interface_index: None,
        }];
        let mut ctl = PairingCtl {
            offer: Some(Arc::new(Mutex::new(QrOffer::new(
                CertificateFingerprint([1; 32]),
                "Desk",
                Vec::new(),
                None,
            )))),
            ..PairingCtl::default()
        };
        // A forged record lists the phone's address after its own.
        let (forged, _) = ctl.add_attempt(Lead::Announcement, vec![ip(8), ip(9)]);
        assert!(ctl.offer_targets(&[at(9)], &locals).is_empty());
        // Its own address answered first: the phone's is free again.
        assert!(ctl.connected(forged, ip(8), CertificateFingerprint([8; 32])));
        assert_eq!(ctl.offer_targets(&[at(9)], &locals), [at(9)]);

        // The phone, reached at two of its addresses: the later connection
        // closes, so the phone does not decline the first.
        let phone = CertificateFingerprint([9; 32]);
        let (v4, _) = ctl.add_attempt(Lead::Beacon, vec![ip(9)]);
        let (v6, _) = ctl.add_attempt(Lead::Announcement, vec![ip(19)]);
        assert!(ctl.connected(v4, ip(9), phone));
        assert!(!ctl.connected(v6, ip(19), phone));
        // Once the first ended, another may reach the phone.
        assert!(ctl.finished(v4).is_some());
        assert!(ctl.connected(v6, ip(19), phone));
        // One that was closed before it connected is not revived.
        assert!(ctl.finished(v6).is_some());
        assert!(!ctl.connected(v6, ip(19), phone));
    }
}
