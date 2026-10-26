//! When to dial a paired phone again.
//!
//! One [`Reconnector`] per paired phone is a pure state machine: the caller
//! tells it what happened (a session started or ended, the network changed,
//! discovery saw the phone) and asks [`poll`](Reconnector::poll) what to do
//! now. It never reads a clock or sleeps, so the policy of `docs/protocol.md`
//! ("Transport") is testable without waiting:
//!
//! - after a failed attempt the delay is 1, 2, 4, 8, 16, 32, then 60 seconds,
//!   each varied by up to 20 % either way;
//! - the schedule restarts when the network changes, on resume from sleep and
//!   after a session of more than a minute;
//! - discovery evidence triggers an attempt at once, but never more than one
//!   attempt per phone every 2 seconds;
//! - the phone shows as "reconnecting" for 2 minutes after a session ends,
//!   then "disconnected", while attempts continue;
//! - a `Goodbye` reason can stop attempts until the user reconnects.
//!
//! Dialing itself races the candidate addresses of [`dial_plan`].

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use link2linux_protocol::v1::GoodbyeReason;

/// Delay after the n-th failed attempt in a row (index n - 1; the last
/// entry repeats).
pub const BACKOFF: [Duration; 7] = [
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(4),
    Duration::from_secs(8),
    Duration::from_secs(16),
    Duration::from_secs(32),
    Duration::from_secs(60),
];
/// The delay varies by this share either way.
pub const JITTER: f64 = 0.2;
/// A session longer than this restarts the schedule.
pub const STABLE_SESSION: Duration = Duration::from_secs(60);
/// The shortest time between two attempts triggered by new evidence.
pub const MIN_DIAL_INTERVAL: Duration = Duration::from_secs(2);
/// How long a phone shows as "reconnecting" after its session ends.
pub const RECONNECTING_FOR: Duration = Duration::from_secs(2 * 60);
/// The gap between starting connections to successive candidate addresses.
pub const CANDIDATE_STAGGER: Duration = Duration::from_millis(250);
/// The most candidate addresses one attempt races.
pub const MAX_CANDIDATES: usize = 12;

/// How a session ended, as far as the reconnect policy cares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionEnd {
    /// The connection dropped, or `Goodbye` said `UNSPECIFIED` or nothing.
    Lost,
    /// `SHUTDOWN` or `PROTOCOL_ERROR`: reconnect with the usual backoff.
    Backoff,
    /// `USER_DISCONNECT`: no attempts until the user reconnects.
    UserDisconnect,
    /// `VERSION_MISMATCH`: no attempts until the user reconnects, which
    /// they do after updating an app.
    VersionMismatch,
    /// `UNPAIRED`: the record is gone; stop for good.
    Unpaired,
    /// `REPLACED`: a newer session exists, so nothing changes.
    Replaced,
}

impl SessionEnd {
    /// The policy for a received `Goodbye` reason, or for none.
    #[must_use]
    pub fn from_goodbye(reason: Option<GoodbyeReason>) -> Self {
        match reason {
            None | Some(GoodbyeReason::Unspecified) => Self::Lost,
            Some(GoodbyeReason::Shutdown | GoodbyeReason::ProtocolError) => Self::Backoff,
            Some(GoodbyeReason::UserDisconnect) => Self::UserDisconnect,
            Some(GoodbyeReason::VersionMismatch) => Self::VersionMismatch,
            Some(GoodbyeReason::Unpaired) => Self::Unpaired,
            Some(GoodbyeReason::Replaced) => Self::Replaced,
        }
    }
}

/// Why attempts are stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Halt {
    /// The phone's user disconnected.
    UserDisconnect,
    /// The apps speak no common protocol version.
    VersionMismatch,
    /// The phone removed the pairing.
    Unpaired,
}

/// What to show for a phone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// A session is up.
    Connected,
    /// No session; attempts run, and the last one ended less than 2
    /// minutes ago (or the desktop has just started).
    Reconnecting,
    /// No session for longer; attempts continue unless halted.
    Disconnected(Option<Halt>),
}

/// What the caller should do now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Poll {
    /// Start a dial attempt now. The caller reports the outcome with
    /// [`attempt_failed`](Reconnector::attempt_failed) or
    /// [`connected`](Reconnector::connected).
    Dial,
    /// Ask again at this time, or earlier when something happens.
    WaitUntil(Instant),
    /// Nothing to do until something happens: a session is up, an attempt
    /// is running, or attempts are halted.
    Idle,
}

/// Reconnect state for one paired phone.
#[derive(Debug)]
pub struct Reconnector {
    rng: SplitMix64,
    failures: u32,
    connected_since: Option<Instant>,
    in_flight: bool,
    next_attempt: Option<Instant>,
    wake: bool,
    last_dial: Option<Instant>,
    down_since: Instant,
    halt: Option<Halt>,
}

impl Reconnector {
    /// A phone with no session, to be dialed at once. `now` is when the
    /// desktop (or the pairing) started.
    #[must_use]
    pub fn new(now: Instant) -> Self {
        Self::with_seed(now, random_seed())
    }

    /// Like [`new`](Self::new) with a fixed jitter seed, for tests.
    #[must_use]
    pub fn with_seed(now: Instant, seed: u64) -> Self {
        Self {
            rng: SplitMix64(seed),
            failures: 0,
            connected_since: None,
            in_flight: false,
            next_attempt: Some(now),
            wake: false,
            last_dial: None,
            down_since: now,
            halt: None,
        }
    }

    /// What to do now.
    pub fn poll(&mut self, now: Instant) -> Poll {
        if self.connected_since.is_some() || self.in_flight || self.halt.is_some() {
            return Poll::Idle;
        }
        let due = if self.wake {
            Some(now)
        } else {
            self.next_attempt
        };
        let Some(due) = due else {
            return Poll::Idle;
        };
        // New evidence is rate limited; a scheduled attempt already waited.
        let earliest = match (self.wake, self.last_dial) {
            (true, Some(last)) => (last + MIN_DIAL_INTERVAL).max(due),
            _ => due,
        };
        if now >= earliest {
            self.in_flight = true;
            self.wake = false;
            self.next_attempt = None;
            self.last_dial = Some(now);
            Poll::Dial
        } else {
            Poll::WaitUntil(earliest)
        }
    }

    /// The dial attempt failed (no candidate completed TLS with the
    /// expected key). The next one waits for the backoff.
    pub fn attempt_failed(&mut self, now: Instant) {
        self.in_flight = false;
        self.schedule_after_failure(now);
    }

    /// A session is up.
    pub fn connected(&mut self, now: Instant) {
        self.in_flight = false;
        self.wake = false;
        self.next_attempt = None;
        self.connected_since = Some(now);
        self.halt = None;
    }

    /// The session ended.
    pub fn session_ended(&mut self, now: Instant, end: SessionEnd) {
        match end {
            SessionEnd::Replaced => return,
            SessionEnd::UserDisconnect => self.halt = Some(Halt::UserDisconnect),
            SessionEnd::VersionMismatch => self.halt = Some(Halt::VersionMismatch),
            SessionEnd::Unpaired => self.halt = Some(Halt::Unpaired),
            SessionEnd::Lost | SessionEnd::Backoff => {}
        }
        let lasted = self
            .connected_since
            .take()
            .map_or(Duration::ZERO, |since| now.saturating_duration_since(since));
        self.in_flight = false;
        self.wake = false;
        self.down_since = now;
        if lasted > STABLE_SESSION {
            // A stable session: the last known address first, at once.
            self.failures = 0;
            self.next_attempt = Some(now);
        } else {
            self.schedule_after_failure(now);
        }
    }

    /// The desktop started, resumed from sleep, or its network changed:
    /// restart the schedule and dial at once.
    pub fn network_changed(&mut self, now: Instant) {
        self.failures = 0;
        if self.connected_since.is_none() {
            self.next_attempt = Some(now);
        }
    }

    /// Discovery saw the phone: a matching mDNS record or beacon. Triggers
    /// an attempt at once, subject to the 2-second limit. Ignored while
    /// connected, halted or already dialing.
    pub fn evidence(&mut self) {
        if self.connected_since.is_none() && !self.in_flight && self.halt.is_none() {
            self.wake = true;
        }
    }

    /// The user asked to reconnect: clears a halt and dials at once, as
    /// if the network had just changed.
    pub fn user_reconnect(&mut self, now: Instant) {
        if self.halt == Some(Halt::Unpaired) {
            return;
        }
        self.halt = None;
        self.failures = 0;
        self.next_attempt = Some(now);
        self.wake = false;
        self.last_dial = None;
    }

    /// What to show for the phone.
    #[must_use]
    pub fn status(&self, now: Instant) -> Status {
        if self.connected_since.is_some() {
            Status::Connected
        } else if self.halt.is_none()
            && now.saturating_duration_since(self.down_since) < RECONNECTING_FOR
        {
            Status::Reconnecting
        } else {
            Status::Disconnected(self.halt)
        }
    }

    /// The failures in a row since the schedule last restarted.
    #[must_use]
    pub fn failures(&self) -> u32 {
        self.failures
    }

    fn schedule_after_failure(&mut self, now: Instant) {
        self.failures = self.failures.saturating_add(1);
        let index = (self.failures as usize - 1).min(BACKOFF.len() - 1);
        let factor = 1.0 - JITTER + 2.0 * JITTER * self.rng.next_unit();
        self.next_attempt = Some(now + BACKOFF[index].mul_f64(factor));
    }
}

/// The addresses one attempt races, in order, with the time after the
/// start of the attempt at which to start each connection. The last known
/// address goes first, then the phone's mDNS addresses, then the source of
/// a beacon; duplicates are dropped. See [`ranked_plan`] for another order.
#[must_use]
pub fn dial_plan(
    last_known: Option<SocketAddr>,
    from_mdns: &[SocketAddr],
    from_beacon: &[SocketAddr],
) -> Vec<(SocketAddr, Duration)> {
    ranked_plan(
        last_known
            .iter()
            .chain(from_mdns)
            .chain(from_beacon)
            .copied(),
    )
}

/// Like [`dial_plan`], for candidates the caller ranked: the first
/// [`MAX_CANDIDATES`] of `candidates`, best first, without duplicates.
#[must_use]
pub fn ranked_plan(
    candidates: impl IntoIterator<Item = SocketAddr>,
) -> Vec<(SocketAddr, Duration)> {
    let mut seen = Vec::new();
    for addr in candidates {
        if !seen.contains(&addr) && seen.len() < MAX_CANDIDATES {
            seen.push(addr);
        }
    }
    seen.into_iter()
        .enumerate()
        .map(|(i, addr)| {
            (
                addr,
                CANDIDATE_STAGGER * u32::try_from(i).unwrap_or(u32::MAX),
            )
        })
        .collect()
}

/// SplitMix64: small, fast and good enough for jitter. Not for secrets.
#[derive(Debug)]
struct SplitMix64(u64);

impl SplitMix64 {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A number in `[0, 1)`.
    fn next_unit(&mut self) -> f64 {
        // The top 53 bits fill an f64 mantissa exactly.
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// A seed that differs per process and per call, from the standard
/// library's randomly keyed hasher.
fn random_seed() -> u64 {
    RandomState::new().build_hasher().finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    /// Fails an attempt `n` times in a row and returns the delays.
    fn delays(seed: u64, n: usize) -> Vec<Duration> {
        let t0 = Instant::now();
        let mut r = Reconnector::with_seed(t0, seed);
        let mut now = t0;
        let mut out = Vec::new();
        for _ in 0..n {
            assert_eq!(r.poll(now), Poll::Dial);
            r.attempt_failed(now);
            let Poll::WaitUntil(at) = r.poll(now) else {
                panic!("expected a wait")
            };
            out.push(at - now);
            now = at;
        }
        out
    }

    #[test]
    fn dials_at_once_when_new() {
        let t0 = Instant::now();
        let mut r = Reconnector::new(t0);
        assert_eq!(r.status(t0), Status::Reconnecting);
        assert_eq!(r.poll(t0), Poll::Dial);
        assert_eq!(r.poll(t0), Poll::Idle, "one attempt at a time");
    }

    #[test]
    fn backoff_follows_the_table_within_jitter() {
        for seed in 0..50 {
            let got = delays(seed, 9);
            for (i, delay) in got.iter().enumerate() {
                let base = BACKOFF[i.min(6)].as_secs_f64();
                let ratio = delay.as_secs_f64() / base;
                assert!(
                    (0.8..=1.2).contains(&ratio),
                    "seed {seed}, failure {}: {delay:?} vs {base}",
                    i + 1
                );
            }
        }
    }

    #[test]
    fn jitter_is_not_constant() {
        let a = delays(1, 3);
        let b = delays(2, 3);
        assert_ne!(a, b);
    }

    #[test]
    fn splitmix_matches_the_reference_sequence() {
        // First outputs of SplitMix64 seeded with 0 (from the reference
        // implementation).
        let mut rng = SplitMix64(0);
        assert_eq!(rng.next_u64(), 0xE220_A839_7B1D_CDAF);
        assert_eq!(rng.next_u64(), 0x6E78_9E6A_A1B9_65F4);
        let mut rng = SplitMix64(7);
        for _ in 0..1000 {
            let u = rng.next_unit();
            assert!((0.0..1.0).contains(&u));
        }
    }

    #[test]
    fn evidence_dials_at_once_but_not_twice_in_two_seconds() {
        let t0 = Instant::now();
        let mut r = Reconnector::with_seed(t0, 1);
        assert_eq!(r.poll(t0), Poll::Dial);
        r.attempt_failed(t0);
        // The backoff says about 1 s; evidence cuts the wait but the
        // 2-second limit since the last dial holds.
        let now = t0 + Duration::from_millis(300);
        r.evidence();
        assert_eq!(r.poll(now), Poll::WaitUntil(t0 + MIN_DIAL_INTERVAL));
        assert_eq!(r.poll(t0 + MIN_DIAL_INTERVAL), Poll::Dial);
        r.attempt_failed(t0 + MIN_DIAL_INTERVAL);
        // Evidence far from the last dial is acted on immediately.
        let later = t0 + secs(30);
        r.evidence();
        assert_eq!(r.poll(later), Poll::Dial);
    }

    #[test]
    fn evidence_does_not_restart_the_backoff() {
        let t0 = Instant::now();
        let mut r = Reconnector::with_seed(t0, 1);
        for i in 0..3 {
            let now = t0 + secs(100 * i);
            r.evidence();
            assert_eq!(r.poll(now), Poll::Dial);
            r.attempt_failed(now);
        }
        assert_eq!(r.failures(), 3);
    }

    #[test]
    fn evidence_is_ignored_while_connected_or_dialing() {
        let t0 = Instant::now();
        let mut r = Reconnector::with_seed(t0, 1);
        assert_eq!(r.poll(t0), Poll::Dial);
        r.evidence();
        r.connected(t0);
        r.evidence();
        assert_eq!(r.poll(t0 + secs(10)), Poll::Idle);
        assert_eq!(r.status(t0), Status::Connected);
    }

    #[test]
    fn a_long_session_restarts_the_schedule() {
        let t0 = Instant::now();
        let mut r = Reconnector::with_seed(t0, 1);
        for _ in 0..4 {
            assert_eq!(r.poll(t0 + secs(1000)), Poll::Dial);
            r.attempt_failed(t0);
        }
        assert_eq!(r.failures(), 4);
        r.connected(t0);
        let end = t0 + secs(61);
        r.session_ended(end, SessionEnd::Lost);
        assert_eq!(r.failures(), 0);
        assert_eq!(r.poll(end), Poll::Dial, "last known address first, at once");
        r.attempt_failed(end);
        let Poll::WaitUntil(at) = r.poll(end) else {
            panic!()
        };
        assert!(at - end <= Duration::from_millis(1200));
    }

    #[test]
    fn a_short_session_counts_as_a_failure() {
        let t0 = Instant::now();
        let mut r = Reconnector::with_seed(t0, 1);
        r.poll(t0);
        r.connected(t0);
        r.session_ended(t0 + secs(5), SessionEnd::Lost);
        assert_eq!(r.failures(), 1);
        assert!(matches!(r.poll(t0 + secs(5)), Poll::WaitUntil(_)));
    }

    #[test]
    fn network_change_and_resume_restart_the_schedule() {
        let t0 = Instant::now();
        let mut r = Reconnector::with_seed(t0, 1);
        for _ in 0..5 {
            assert_eq!(r.poll(t0 + secs(1000)), Poll::Dial);
            r.attempt_failed(t0);
        }
        assert!(matches!(r.poll(t0 + secs(1)), Poll::WaitUntil(_)));
        let now = t0 + secs(2);
        r.network_changed(now);
        assert_eq!(r.failures(), 0);
        assert_eq!(r.poll(now), Poll::Dial);
    }

    #[test]
    fn reconnecting_turns_into_disconnected_after_two_minutes() {
        let t0 = Instant::now();
        let mut r = Reconnector::with_seed(t0, 1);
        r.poll(t0);
        r.connected(t0);
        let end = t0 + secs(300);
        r.session_ended(end, SessionEnd::Lost);
        assert_eq!(r.status(end + secs(119)), Status::Reconnecting);
        assert_eq!(r.status(end + secs(120)), Status::Disconnected(None));
        // Attempts continue while disconnected.
        assert!(matches!(r.poll(end + secs(500)), Poll::Dial));
    }

    #[test]
    fn goodbye_reasons_decide_what_happens_next() {
        use GoodbyeReason as G;
        let table = [
            (None, SessionEnd::Lost),
            (Some(G::Unspecified), SessionEnd::Lost),
            (Some(G::Shutdown), SessionEnd::Backoff),
            (Some(G::ProtocolError), SessionEnd::Backoff),
            (Some(G::UserDisconnect), SessionEnd::UserDisconnect),
            (Some(G::VersionMismatch), SessionEnd::VersionMismatch),
            (Some(G::Unpaired), SessionEnd::Unpaired),
            (Some(G::Replaced), SessionEnd::Replaced),
        ];
        for (reason, end) in table {
            assert_eq!(SessionEnd::from_goodbye(reason), end);
        }
    }

    #[test]
    fn halted_phones_are_not_dialed_until_the_user_reconnects() {
        let t0 = Instant::now();
        for (end, halt) in [
            (SessionEnd::UserDisconnect, Halt::UserDisconnect),
            (SessionEnd::VersionMismatch, Halt::VersionMismatch),
        ] {
            let mut r = Reconnector::with_seed(t0, 1);
            r.poll(t0);
            r.connected(t0);
            let end_at = t0 + secs(100);
            r.session_ended(end_at, end);
            r.evidence();
            r.network_changed(end_at + secs(1));
            assert_eq!(r.poll(end_at + secs(1000)), Poll::Idle);
            assert_eq!(r.status(end_at), Status::Disconnected(Some(halt)));
            r.user_reconnect(end_at + secs(2000));
            assert_eq!(r.poll(end_at + secs(2000)), Poll::Dial);
        }
    }

    #[test]
    fn unpaired_stops_for_good_and_replaced_changes_nothing() {
        let t0 = Instant::now();
        let mut r = Reconnector::with_seed(t0, 1);
        r.poll(t0);
        r.connected(t0);
        r.session_ended(t0 + secs(5), SessionEnd::Replaced);
        assert_eq!(r.status(t0 + secs(5)), Status::Connected);
        r.session_ended(t0 + secs(100), SessionEnd::Unpaired);
        r.user_reconnect(t0 + secs(200));
        assert_eq!(r.poll(t0 + secs(300)), Poll::Idle);
    }

    #[test]
    fn dial_plan_orders_dedupes_and_staggers() {
        let a: SocketAddr = "192.168.1.5:27380".parse().unwrap();
        let b: SocketAddr = "[fe80::1]:27380".parse().unwrap();
        let c: SocketAddr = "192.168.1.6:27380".parse().unwrap();
        let plan = dial_plan(Some(a), &[b, a], &[c, b]);
        assert_eq!(
            plan,
            vec![
                (a, Duration::ZERO),
                (b, Duration::from_millis(250)),
                (c, Duration::from_millis(500)),
            ]
        );
        assert!(dial_plan(None, &[], &[]).is_empty());
        let many: Vec<SocketAddr> = (1..=40)
            .map(|i| format!("10.0.0.{i}:1").parse().unwrap())
            .collect();
        assert_eq!(dial_plan(None, &many, &[]).len(), MAX_CANDIDATES);
        assert_eq!(ranked_plan(many).len(), MAX_CANDIDATES);
    }
}
