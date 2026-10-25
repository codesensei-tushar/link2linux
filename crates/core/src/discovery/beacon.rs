//! The 40-byte UDP beacon a phone sends to port 27381, and the checks a
//! desktop makes before it dials the sender.
//!
//! ```text
//!  0  4  magic "L2LB"
//!  4  1  version (1)
//!  5  1  flags: bit 0 = the tag is a pairing tag; other bits are ignored
//!  6  2  the phone's TCP port
//!  8  8  the phone's clock, Unix seconds
//! 16  8  nonce
//! 24 16  tag = HMAC-SHA256(key, "l2l-beacon-v1" || bytes 0..24)[..16]
//! ```
//!
//! A beacon only ever leads to a dial attempt; see `docs/protocol.md`,
//! section "Beacons".

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use aws_lc_rs::{constant_time, hmac};

/// Length of a v1 beacon. A longer datagram carries bytes the tag does not
/// cover, so it is rejected.
pub const BEACON_LEN: usize = 40;
/// UDP port the desktop listens on.
pub const BEACON_PORT: u16 = 27381;
/// How far a beacon's clock may be from the desktop's, in seconds.
pub const TIME_WINDOW_SECS: u64 = 15 * 60;
/// How long an accepted tag is remembered. Accepted timestamps are at
/// most 15 minutes ahead of the desktop's clock, so "15 minutes past the
/// timestamp" is at most 30 minutes from now; remembering for 30 minutes
/// covers it, and by monotonic time, so a clock that steps backwards cannot
/// readmit a replay.
pub const REPLAY_RETENTION: Duration = Duration::from_secs(30 * 60);
/// The most tags the replay cache holds. Only verified beacons enter it.
pub const REPLAY_CAPACITY: usize = 8192;

const MAGIC: &[u8; 4] = b"L2LB";
const VERSION: u8 = 1;
const LABEL: &[u8] = b"l2l-beacon-v1";
const SIGNED_LEN: usize = 24;
const FLAG_PAIRING: u8 = 1;

/// Why a datagram is not a beacon this desktop understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum BeaconError {
    /// Not exactly 40 bytes.
    #[error("beacon is {0} bytes, not 40")]
    Length(usize),
    /// Does not start with `L2LB`.
    #[error("not a beacon: wrong magic")]
    Magic,
    /// A version this desktop does not know.
    #[error("unknown beacon version {0}")]
    Version(u8),
    /// The port is zero.
    #[error("beacon port is zero")]
    Port,
}

/// A beacon with a well-formed layout. Its tag is not checked yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Beacon {
    raw: [u8; BEACON_LEN],
}

impl Beacon {
    /// Parses a datagram: exact length, magic, version 1 and a nonzero
    /// port. Reserved flag bits are ignored; the tag covers them.
    pub fn parse(datagram: &[u8]) -> Result<Self, BeaconError> {
        let raw: [u8; BEACON_LEN] = datagram
            .try_into()
            .map_err(|_| BeaconError::Length(datagram.len()))?;
        if &raw[..4] != MAGIC {
            return Err(BeaconError::Magic);
        }
        if raw[4] != VERSION {
            return Err(BeaconError::Version(raw[4]));
        }
        let beacon = Self { raw };
        if beacon.port() == 0 {
            return Err(BeaconError::Port);
        }
        Ok(beacon)
    }

    /// Builds a beacon sealed with `key`. A phone does this; the desktop
    /// uses it in tests.
    #[must_use]
    pub fn seal(
        key: &[u8],
        pairing: bool,
        port: u16,
        unix_time: u64,
        nonce: [u8; 8],
    ) -> [u8; BEACON_LEN] {
        let mut raw = [0; BEACON_LEN];
        raw[..4].copy_from_slice(MAGIC);
        raw[4] = VERSION;
        raw[5] = u8::from(pairing);
        raw[6..8].copy_from_slice(&port.to_be_bytes());
        raw[8..16].copy_from_slice(&unix_time.to_be_bytes());
        raw[16..24].copy_from_slice(&nonce);
        let tag = compute_tag(key, &raw[..SIGNED_LEN]);
        raw[SIGNED_LEN..].copy_from_slice(&tag);
        raw
    }

    /// Whether the tag is a pairing tag (flag bit 0).
    #[must_use]
    pub fn is_pairing(&self) -> bool {
        self.raw[5] & FLAG_PAIRING != 0
    }

    /// The phone's TCP port.
    #[must_use]
    pub fn port(&self) -> u16 {
        u16::from_be_bytes([self.raw[6], self.raw[7]])
    }

    /// The phone's clock, Unix seconds.
    #[must_use]
    pub fn unix_time(&self) -> u64 {
        u64::from_be_bytes(self.raw[8..16].try_into().expect("8 bytes"))
    }

    /// The tag, which names this beacon in the replay cache.
    #[must_use]
    pub fn tag(&self) -> [u8; 16] {
        self.raw[SIGNED_LEN..].try_into().expect("16 bytes")
    }

    /// Whether the tag was made with `key`, compared in constant time.
    #[must_use]
    pub fn verify(&self, key: &[u8]) -> bool {
        let expected = compute_tag(key, &self.raw[..SIGNED_LEN]);
        constant_time::verify_slices_are_equal(&expected, &self.raw[SIGNED_LEN..]).is_ok()
    }
}

fn compute_tag(key: &[u8], signed: &[u8]) -> [u8; 16] {
    let mut context = hmac::Context::with_key(&hmac::Key::new(hmac::HMAC_SHA256, key));
    context.update(LABEL);
    context.update(signed);
    let mac = context.sign();
    mac.as_ref()[..16].try_into().expect("16 bytes")
}

/// Who a verified beacon is from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BeaconSender<K> {
    /// A paired phone, named by the caller's identifier for its key.
    Paired(K),
    /// A phone that scanned the active QR offer.
    Pairing,
}

/// A beacon that passed every check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Accepted<K> {
    /// Who sent it.
    pub sender: BeaconSender<K>,
    /// The phone's TCP port; the dial goes to the datagram's source
    /// address at this port.
    pub port: u16,
}

/// Why a beacon was dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rejected<K> {
    /// Not a well-formed beacon.
    Malformed(BeaconError),
    /// The tag matches none of the keys offered. Anyone can send these.
    UnknownTag,
    /// The tag verifies but the timestamp is outside the window: the
    /// phone's clock, or this desktop's, is off. Worth a hint in the log.
    ClockSkew {
        /// Who sent it.
        sender: BeaconSender<K>,
        /// The phone's clock minus the desktop's, in seconds.
        skew_secs: i64,
    },
    /// This tag was accepted before.
    Replay,
}

/// Verifies beacons and remembers the tags it accepted.
#[derive(Debug, Default)]
pub struct BeaconVerifier {
    seen: HashMap<[u8; 16], Instant>,
    order: VecDeque<[u8; 16]>,
}

impl BeaconVerifier {
    /// An empty verifier.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Checks one datagram.
    ///
    /// `keys` are the presence keys of the paired phones, each with the
    /// caller's identifier; `pairing_secret` is the secret of the QR offer
    /// being shown, if any. `now_unix` is the desktop's wall clock and
    /// `now` its monotonic clock; replay memory expires by `now` only.
    ///
    /// # Errors
    ///
    /// Returns why the beacon must be dropped.
    pub fn check<K: Clone>(
        &mut self,
        datagram: &[u8],
        keys: &[(K, &[u8])],
        pairing_secret: Option<&[u8]>,
        now_unix: u64,
        now: Instant,
    ) -> Result<Accepted<K>, Rejected<K>> {
        let beacon = Beacon::parse(datagram).map_err(Rejected::Malformed)?;
        let sender = if beacon.is_pairing() {
            pairing_secret
                .filter(|secret| beacon.verify(secret))
                .map(|_| BeaconSender::Pairing)
        } else {
            // Every key is checked, so timing does not tell which matched.
            keys.iter().fold(None, |found, (id, key)| {
                let ok = beacon.verify(key);
                found.or(ok.then(|| BeaconSender::Paired(id.clone())))
            })
        }
        .ok_or(Rejected::UnknownTag)?;

        let skew = i128::from(beacon.unix_time()) - i128::from(now_unix);
        if skew.unsigned_abs() > u128::from(TIME_WINDOW_SECS) {
            let skew_secs =
                i64::try_from(skew).unwrap_or(if skew < 0 { i64::MIN } else { i64::MAX });
            return Err(Rejected::ClockSkew { sender, skew_secs });
        }

        self.expire(now);
        let tag = beacon.tag();
        if self.seen.contains_key(&tag) {
            return Err(Rejected::Replay);
        }
        if self.order.len() >= REPLAY_CAPACITY
            && let Some(oldest) = self.order.pop_front()
        {
            self.seen.remove(&oldest);
        }
        self.seen.insert(tag, now);
        self.order.push_back(tag);
        Ok(Accepted {
            sender,
            port: beacon.port(),
        })
    }

    /// The number of tags remembered.
    #[must_use]
    pub fn remembered(&self) -> usize {
        self.seen.len()
    }

    fn expire(&mut self, now: Instant) {
        // Insertion order is expiry order: every tag lives equally long.
        while let Some(tag) = self.order.front() {
            let age = self
                .seen
                .get(tag)
                .map_or(REPLAY_RETENTION, |at| now.saturating_duration_since(*at));
            if age < REPLAY_RETENTION {
                break;
            }
            let tag = self.order.pop_front().expect("front exists");
            self.seen.remove(&tag);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::testvec::Vector;
    use proptest::prelude::*;

    const KEY: &[u8] = &[7; 32];
    const SECRET: &[u8] = &[9; 16];
    const NOW: u64 = 1_793_318_400;

    type Verifier = BeaconVerifier;

    fn check(
        v: &mut Verifier,
        datagram: &[u8],
        now_unix: u64,
        now: Instant,
    ) -> Result<Accepted<u8>, Rejected<u8>> {
        v.check(datagram, &[(1u8, KEY)], Some(SECRET), now_unix, now)
    }

    #[test]
    fn matches_the_pairing_vector() {
        let p = Vector::pairing();
        let key = p.bytes("presence_key");
        let beacon = Beacon::seal(&key, false, 27380, 1_793_318_400, p.array("beacon_nonce"));
        assert_eq!(beacon.to_vec(), p.bytes("beacon"));
        let secret = p.bytes("secret");
        let beacon = Beacon::seal(&secret, true, 27380, 1_793_318_400, p.array("beacon_nonce"));
        assert_eq!(beacon.to_vec(), p.bytes("beacon_pairing"));
    }

    #[test]
    fn follows_the_beacon_vector() {
        let v = Vector::parse(include_str!("../../../../proto/testdata/v1/beacon.txt"));
        let p = Vector::pairing();
        assert_eq!(v.bytes("accept_paired"), p.bytes("beacon"));
        assert_eq!(v.bytes("accept_pairing"), p.bytes("beacon_pairing"));
        let key = v.bytes("presence_key");
        let secret = v.bytes("secret");
        let now: u64 = v.text("now").parse().unwrap();
        let mono = Instant::now();
        for name in v.names() {
            if name.starts_with("accept_")
                || name.starts_with("reject_")
                || name.starts_with("skew_")
            {
                let mut verifier = Verifier::new();
                let result = verifier.check(
                    &v.bytes(name),
                    &[(1u8, key.as_slice())],
                    Some(secret.as_slice()),
                    now,
                    mono,
                );
                match name.split('_').next().unwrap() {
                    "accept" => assert!(result.is_ok(), "{name}: {result:?}"),
                    "reject" => assert!(
                        matches!(result, Err(Rejected::Malformed(_) | Rejected::UnknownTag)),
                        "{name}: {result:?}"
                    ),
                    _ => assert!(
                        matches!(result, Err(Rejected::ClockSkew { .. })),
                        "{name}: {result:?}"
                    ),
                }
            }
        }
    }

    #[test]
    fn parse_rejects_bad_layouts() {
        let good = Beacon::seal(KEY, false, 27380, NOW, [1; 8]);
        assert!(Beacon::parse(&good).is_ok());
        assert_eq!(Beacon::parse(&good[..39]), Err(BeaconError::Length(39)));
        let mut longer = good.to_vec();
        longer.push(0);
        assert_eq!(Beacon::parse(&longer), Err(BeaconError::Length(41)));
        let mut bad = good;
        bad[0] = b'X';
        assert_eq!(Beacon::parse(&bad), Err(BeaconError::Magic));
        let mut bad = good;
        bad[4] = 2;
        assert_eq!(Beacon::parse(&bad), Err(BeaconError::Version(2)));
        assert_eq!(
            Beacon::parse(&Beacon::seal(KEY, false, 0, NOW, [1; 8])),
            Err(BeaconError::Port)
        );
    }

    #[test]
    fn reserved_flag_bits_are_covered_by_the_tag() {
        let good = Beacon::seal(KEY, false, 27380, NOW, [1; 8]);
        let mut flipped = good;
        flipped[5] |= 0x80;
        let beacon = Beacon::parse(&flipped).unwrap();
        assert!(!beacon.is_pairing());
        assert!(!beacon.verify(KEY), "the tag covers the flags");
    }

    #[test]
    fn accepts_once_then_calls_it_a_replay() {
        let mut v = Verifier::new();
        let now = Instant::now();
        let datagram = Beacon::seal(KEY, false, 4000, NOW, [1; 8]);
        assert_eq!(
            check(&mut v, &datagram, NOW, now),
            Ok(Accepted {
                sender: BeaconSender::Paired(1),
                port: 4000
            })
        );
        assert_eq!(check(&mut v, &datagram, NOW, now), Err(Rejected::Replay));
        assert_eq!(v.remembered(), 1);
    }

    #[test]
    fn pairing_beacons_need_the_offer_secret() {
        let mut v = Verifier::new();
        let now = Instant::now();
        let datagram = Beacon::seal(SECRET, true, 4000, NOW, [2; 8]);
        assert_eq!(
            check(&mut v, &datagram, NOW, now).map(|a| a.sender),
            Ok(BeaconSender::Pairing)
        );
        let datagram = Beacon::seal(SECRET, true, 4000, NOW, [3; 8]);
        let none: &[(u8, &[u8])] = &[(1, KEY)];
        assert_eq!(
            v.check(&datagram, none, None, NOW, now),
            Err(Rejected::UnknownTag)
        );
    }

    #[test]
    fn the_window_is_inclusive_and_skew_is_reported() {
        let mut v = Verifier::new();
        let now = Instant::now();
        for (i, offset) in [-900i64, 900].into_iter().enumerate() {
            let datagram = Beacon::seal(
                KEY,
                false,
                4000,
                NOW.saturating_add_signed(offset),
                [i as u8; 8],
            );
            assert!(check(&mut v, &datagram, NOW, now).is_ok());
        }
        let late = Beacon::seal(KEY, false, 4000, NOW + 901, [9; 8]);
        assert_eq!(
            check(&mut v, &late, NOW, now),
            Err(Rejected::ClockSkew {
                sender: BeaconSender::Paired(1),
                skew_secs: 901
            })
        );
        let early = Beacon::seal(KEY, false, 4000, NOW - 901, [9; 8]);
        assert!(matches!(
            check(&mut v, &early, NOW, now),
            Err(Rejected::ClockSkew {
                skew_secs: -901,
                ..
            })
        ));
        // Skewed beacons are not remembered.
        assert_eq!(v.remembered(), 2);
    }

    #[test]
    fn a_clock_stepping_back_does_not_readmit_a_replay() {
        let mut v = Verifier::new();
        let t0 = Instant::now();
        let datagram = Beacon::seal(KEY, false, 4000, NOW, [1; 8]);
        assert!(check(&mut v, &datagram, NOW, t0).is_ok());
        // Wall clock jumps back 10 minutes, then 20 minutes pass.
        let later = t0 + Duration::from_secs(20 * 60);
        assert_eq!(
            check(&mut v, &datagram, NOW - 600 + 1200, later),
            Err(Rejected::Replay)
        );
        // Past the retention the tag is forgotten (and by then the
        // timestamp is out of the window anyway).
        let much_later = t0 + REPLAY_RETENTION + Duration::from_secs(1);
        assert!(check(&mut v, &datagram, NOW, much_later).is_ok());
    }

    #[test]
    fn the_cache_is_bounded() {
        let mut v = Verifier::new();
        let now = Instant::now();
        for i in 0..(REPLAY_CAPACITY + 10) as u64 {
            let datagram = Beacon::seal(KEY, false, 4000, NOW, i.to_be_bytes());
            assert!(check(&mut v, &datagram, NOW, now).is_ok());
        }
        assert_eq!(v.remembered(), REPLAY_CAPACITY);
    }

    #[test]
    fn unverified_beacons_never_enter_the_cache() {
        let mut v = Verifier::new();
        let now = Instant::now();
        for i in 0..100u64 {
            let datagram = Beacon::seal(&[0; 32], false, 4000, NOW, i.to_be_bytes());
            assert_eq!(
                check(&mut v, &datagram, NOW, now),
                Err(Rejected::UnknownTag)
            );
        }
        assert_eq!(v.remembered(), 0);
    }

    proptest! {
        #[test]
        fn arbitrary_bytes_never_panic_or_verify(data in proptest::collection::vec(any::<u8>(), 0..64)) {
            let mut v = Verifier::new();
            let result = check(&mut v, &data, NOW, Instant::now());
            prop_assert!(result.is_err());
        }

        #[test]
        fn sealed_beacons_round_trip(port in 1u16.., time in any::<u64>(), nonce in any::<[u8; 8]>(), pairing in any::<bool>()) {
            let raw = Beacon::seal(KEY, pairing, port, time, nonce);
            let beacon = Beacon::parse(&raw).unwrap();
            prop_assert_eq!(beacon.port(), port);
            prop_assert_eq!(beacon.unix_time(), time);
            prop_assert_eq!(beacon.is_pairing(), pairing);
            prop_assert!(beacon.verify(KEY));
            prop_assert!(!beacon.verify(SECRET));
        }
    }
}
