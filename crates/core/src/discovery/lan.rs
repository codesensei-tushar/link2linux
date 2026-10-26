//! [`LanDiscovery`]: mDNS browsing and the beacon listener behind the
//! [`Discovery`] trait.

use std::collections::{HashMap, VecDeque};
use std::future::pending;
use std::net::{IpAddr, SocketAddr, SocketAddrV6};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::sync::watch;

use super::beacon::{BEACON_PORT, BeaconSender, BeaconVerifier, Rejected};
use super::browser::{BrowserEvent, MdnsBrowser};
use super::listener::{BeaconListener, Datagram};
use super::presence::DiscoveryKeys;
use super::tracker::{PhoneTracker, TrackerEvent};
use super::{BeaconSighting, Discovery, DiscoveryError, DiscoveryEvent};

/// How often the clock-skew hint may be logged for one phone.
const SKEW_LOG_INTERVAL: Duration = Duration::from_secs(10 * 60);
/// Pause after a failed receive, so a persistent error does not spin.
const ERROR_PAUSE: Duration = Duration::from_millis(100);
/// The first wait before browsing mDNS again after a browse stopped, and
/// the longest; each stop in a row doubles it.
const BROWSE_RETRY_FIRST: Duration = Duration::from_secs(1);
const BROWSE_RETRY_MAX: Duration = Duration::from_secs(60);
/// A browse that ran this long before it stopped starts the waits over.
const BROWSE_STABLE: Duration = Duration::from_secs(10 * 60);

/// What [`LanDiscovery`] listens to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LanConfig {
    /// The UDP port for beacons; 27381 outside of tests.
    pub beacon_port: u16,
    /// Whether to browse mDNS.
    pub mdns: bool,
}

impl Default for LanConfig {
    fn default() -> Self {
        Self {
            beacon_port: BEACON_PORT,
            mdns: true,
        }
    }
}

/// Discovery on the local network with mDNS and beacons.
///
/// It works with whichever of the two the network allows: a failure of one
/// is logged and the other keeps running; [`start`](Discovery::start) fails
/// only when neither can run.
#[derive(Debug, Default)]
pub struct LanDiscovery {
    config: LanConfig,
    running: Option<Running>,
}

#[derive(Debug)]
struct Running {
    listener: Option<BeaconListener>,
    browser: Option<MdnsBrowser>,
    /// When to browse mDNS again, after the browse stopped.
    browse_again: Option<tokio::time::Instant>,
    restarts: BrowseRestarts,
    keys: watch::Receiver<DiscoveryKeys>,
    keys_closed: bool,
    tracker: PhoneTracker,
    verifier: BeaconVerifier,
    pending: VecDeque<DiscoveryEvent>,
    skew_logged: HashMap<String, Instant>,
}

impl LanDiscovery {
    /// Discovery with the given configuration, not yet started.
    #[must_use]
    pub fn new(config: LanConfig) -> Self {
        Self {
            config,
            running: None,
        }
    }

    /// The addresses the beacon sockets are bound to, empty when not
    /// started.
    #[must_use]
    pub fn beacon_addrs(&self) -> Vec<SocketAddr> {
        self.running
            .as_ref()
            .and_then(|r| r.listener.as_ref())
            .map(BeaconListener::local_addrs)
            .unwrap_or_default()
    }
}

impl Discovery for LanDiscovery {
    async fn start(&mut self, keys: watch::Receiver<DiscoveryKeys>) -> Result<(), DiscoveryError> {
        self.stop().await?;
        let listener = match BeaconListener::bind(self.config.beacon_port) {
            Ok(listener) => Some(listener),
            Err(e) => {
                tracing::warn!(%e, "cannot listen for beacons");
                None
            }
        };
        let browser = if self.config.mdns {
            match MdnsBrowser::start() {
                Ok(browser) => Some(browser),
                Err(e) => {
                    tracing::warn!(%e, "cannot browse mDNS");
                    None
                }
            }
        } else {
            None
        };
        if listener.is_none() && browser.is_none() {
            return Err(DiscoveryError::Unavailable(
                "neither mDNS nor the beacon port can be used".to_owned(),
            ));
        }
        let tracker = PhoneTracker::new(keys.borrow().clone());
        self.running = Some(Running {
            listener,
            browser,
            browse_again: None,
            restarts: BrowseRestarts::new(Instant::now()),
            keys,
            keys_closed: false,
            tracker,
            verifier: BeaconVerifier::new(),
            pending: VecDeque::new(),
            skew_logged: HashMap::new(),
        });
        Ok(())
    }

    async fn next_event(&mut self) -> Option<DiscoveryEvent> {
        let r = self.running.as_mut()?;
        loop {
            if let Some(event) = r.pending.pop_front() {
                return Some(event);
            }
            if r.listener.is_none() && r.browser.is_none() && r.browse_again.is_none() {
                return None;
            }
            tokio::select! {
                changed = r.keys.changed(), if !r.keys_closed => {
                    if changed.is_err() {
                        // The owner dropped the sender: keep the last keys.
                        r.keys_closed = true;
                    } else {
                        let keys = r.keys.borrow_and_update().clone();
                        r.pending.extend(r.tracker.set_keys(keys).into_iter().map(map_tracker));
                    }
                }
                event = browse(r.browser.as_ref()) => match event {
                    Some(BrowserEvent::Resolved(record)) => {
                        r.pending.extend(r.tracker.resolved(record).into_iter().map(map_tracker));
                    }
                    Some(BrowserEvent::Removed(instance)) => {
                        r.pending.extend(r.tracker.removed(&instance).map(map_tracker));
                    }
                    None => {
                        // mdns-sd ends a browse when its event queue fills,
                        // which a flood of announcements can cause.
                        if let Some(browser) = r.browser.take() {
                            browser.shutdown();
                        }
                        let wait = r.restarts.stopped(Instant::now());
                        tracing::warn!(?wait, "mDNS browsing stopped; starting it again");
                        r.browse_again = Some(tokio::time::Instant::now() + wait);
                    }
                },
                () = sleep_until(r.browse_again) => {
                    r.browse_again = None;
                    match MdnsBrowser::start() {
                        Ok(browser) => {
                            r.browser = Some(browser);
                            r.restarts.started(Instant::now());
                        }
                        Err(e) => {
                            let wait = r.restarts.stopped(Instant::now());
                            tracing::warn!(%e, ?wait, "cannot browse mDNS again yet");
                            r.browse_again = Some(tokio::time::Instant::now() + wait);
                        }
                    }
                }
                received = receive(r.listener.as_ref()) => match received {
                    Ok(datagram) => {
                        let event = r.handle_datagram(&datagram);
                        r.pending.extend(event);
                    }
                    Err(e) => {
                        tracing::debug!(%e, "beacon receive failed");
                        tokio::time::sleep(ERROR_PAUSE).await;
                    }
                },
            }
        }
    }

    async fn stop(&mut self) -> Result<(), DiscoveryError> {
        if let Some(running) = self.running.take()
            && let Some(browser) = running.browser
        {
            browser.shutdown();
        }
        Ok(())
    }
}

impl Running {
    fn handle_datagram(&mut self, datagram: &Datagram) -> Option<DiscoveryEvent> {
        let now_unix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let now = Instant::now();
        let verdict = {
            let keys = self.keys.borrow();
            let paired: Vec<(String, &[u8])> = keys
                .paired
                .iter()
                .map(|p| (p.phone_id.clone(), p.presence_key.as_bytes()))
                .collect();
            self.verifier.check(
                &datagram.data,
                &paired,
                keys.offer_secret.as_ref().map(|s| s.as_bytes()),
                now_unix,
                now,
            )
        };
        match verdict {
            Ok(accepted) => Some(DiscoveryEvent::Beacon(BeaconSighting {
                sender: accepted.sender,
                address: with_port(datagram.source, accepted.port),
            })),
            Err(Rejected::ClockSkew { sender, skew_secs }) => {
                let who = match sender {
                    BeaconSender::Paired(id) => id,
                    BeaconSender::Pairing => "the pairing offer".to_owned(),
                };
                let due = self
                    .skew_logged
                    .get(&who)
                    .is_none_or(|at| now.duration_since(*at) >= SKEW_LOG_INTERVAL);
                if due {
                    tracing::warn!(
                        phone = %who,
                        skew_secs,
                        "a beacon's clock is more than 15 minutes off; check the clock on the phone and on this computer"
                    );
                    self.skew_logged.insert(who, now);
                }
                None
            }
            Err(rejected) => {
                tracing::trace!(?rejected, "dropped a beacon");
                None
            }
        }
    }
}

/// The waits between mDNS browses, when a browse keeps stopping.
#[derive(Debug)]
struct BrowseRestarts {
    next: Duration,
    started: Instant,
}

impl BrowseRestarts {
    fn new(now: Instant) -> Self {
        Self {
            next: BROWSE_RETRY_FIRST,
            started: now,
        }
    }

    /// A browse started at `now`.
    fn started(&mut self, now: Instant) {
        self.started = now;
    }

    /// The browse stopped, or could not start, at `now`: how long to wait
    /// before the next one.
    fn stopped(&mut self, now: Instant) -> Duration {
        if now.saturating_duration_since(self.started) >= BROWSE_STABLE {
            self.next = BROWSE_RETRY_FIRST;
        }
        // Until a browse starts again, failures count from here.
        self.started = now;
        let wait = self.next;
        self.next = (self.next * 2).min(BROWSE_RETRY_MAX);
        wait
    }
}

fn with_port(source: SocketAddr, port: u16) -> SocketAddr {
    match source {
        SocketAddr::V4(v4) => SocketAddr::new(IpAddr::V4(*v4.ip()), port),
        // Keep the scope: a link-local source is only usable with its
        // interface.
        SocketAddr::V6(v6) => SocketAddr::V6(SocketAddrV6::new(*v6.ip(), port, 0, v6.scope_id())),
    }
}

fn map_tracker(event: TrackerEvent) -> DiscoveryEvent {
    match event {
        TrackerEvent::Found(s) => DiscoveryEvent::Found(s),
        TrackerEvent::Updated(s) => DiscoveryEvent::Updated(s),
        TrackerEvent::Lost { instance } => DiscoveryEvent::Lost { instance },
    }
}

async fn browse(browser: Option<&MdnsBrowser>) -> Option<BrowserEvent> {
    match browser {
        Some(browser) => browser.next().await,
        None => pending().await,
    }
}

/// Sleeps until `deadline`, or forever without one.
async fn sleep_until(deadline: Option<tokio::time::Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => pending().await,
    }
}

async fn receive(listener: Option<&BeaconListener>) -> std::io::Result<Datagram> {
    match listener {
        Some(listener) => listener.recv().await,
        None => pending().await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::beacon::Beacon;
    use crate::discovery::presence::{PairedKey, SecretBytes};
    use tokio::net::UdpSocket;

    const KEY: [u8; 32] = [3; 32];

    fn keys() -> DiscoveryKeys {
        DiscoveryKeys {
            paired: vec![PairedKey {
                phone_id: "phone-a".to_owned(),
                presence_key: SecretBytes::new(KEY.to_vec()),
            }],
            offer_secret: None,
        }
    }

    fn now_unix() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    async fn started(initial: DiscoveryKeys) -> (LanDiscovery, watch::Sender<DiscoveryKeys>, u16) {
        let (tx, rx) = watch::channel(initial);
        let mut discovery = LanDiscovery::new(LanConfig {
            beacon_port: 0,
            mdns: false,
        });
        discovery.start(rx).await.unwrap();
        let port = discovery.beacon_addrs()[0].port();
        (discovery, tx, port)
    }

    async fn send(port: u16, datagram: &[u8]) -> SocketAddr {
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        socket.send_to(datagram, ("127.0.0.1", port)).await.unwrap();
        socket.local_addr().unwrap()
    }

    async fn next(discovery: &mut LanDiscovery) -> Option<DiscoveryEvent> {
        tokio::time::timeout(Duration::from_secs(5), discovery.next_event())
            .await
            .ok()
            .flatten()
    }

    #[tokio::test]
    async fn a_verified_beacon_gives_a_dial_address() {
        let (mut discovery, _tx, port) = started(keys()).await;
        let source = send(port, &Beacon::seal(&KEY, false, 4000, now_unix(), [1; 8])).await;
        let Some(DiscoveryEvent::Beacon(sighting)) = next(&mut discovery).await else {
            panic!("expected a beacon")
        };
        assert_eq!(sighting.sender, BeaconSender::Paired("phone-a".to_owned()));
        assert_eq!(
            sighting.address,
            SocketAddr::new(source.ip(), 4000),
            "the source address at the beacon's port"
        );
    }

    #[tokio::test]
    async fn forgeries_and_replays_are_dropped() {
        let (mut discovery, _tx, port) = started(keys()).await;
        let good = Beacon::seal(&KEY, false, 4000, now_unix(), [1; 8]);
        send(
            port,
            &Beacon::seal(&[9; 32], false, 1111, now_unix(), [2; 8]),
        )
        .await;
        send(port, &good).await;
        send(port, &good).await; // replay
        send(port, b"junk").await;
        send(port, &Beacon::seal(&KEY, false, 5000, now_unix(), [3; 8])).await;
        let ports: Vec<u16> = {
            let mut out = Vec::new();
            for _ in 0..2 {
                let Some(DiscoveryEvent::Beacon(b)) = next(&mut discovery).await else {
                    panic!("expected a beacon")
                };
                out.push(b.address.port());
            }
            out
        };
        assert_eq!(ports, vec![4000, 5000]);
    }

    #[tokio::test]
    async fn new_keys_apply_to_later_beacons() {
        let (mut discovery, tx, port) = started(DiscoveryKeys::default()).await;
        send(port, &Beacon::seal(&KEY, false, 4000, now_unix(), [1; 8])).await;
        // Not known yet; the event loop sees the datagram and drops it.
        assert!(
            tokio::time::timeout(Duration::from_millis(300), discovery.next_event())
                .await
                .is_err()
        );
        tx.send(keys()).unwrap();
        send(port, &Beacon::seal(&KEY, false, 6000, now_unix(), [2; 8])).await;
        let Some(DiscoveryEvent::Beacon(b)) = next(&mut discovery).await else {
            panic!("expected a beacon")
        };
        assert_eq!(b.address.port(), 6000);
    }

    #[tokio::test]
    async fn stopped_discovery_ends_the_event_stream() {
        let (mut discovery, _tx, _port) = started(keys()).await;
        discovery.stop().await.unwrap();
        assert_eq!(discovery.next_event().await, None);
        assert!(discovery.beacon_addrs().is_empty());
    }

    #[test]
    fn browsing_again_waits_longer_each_time_until_a_browse_lasts() {
        let start = Instant::now();
        let mut restarts = BrowseRestarts::new(start);
        let waits: Vec<u64> = (0..8).map(|_| restarts.stopped(start).as_secs()).collect();
        assert_eq!(waits, [1, 2, 4, 8, 16, 32, 60, 60]);
        // A browse that ran for a while starts the waits over.
        let later = start + BROWSE_STABLE;
        restarts.started(later);
        assert_eq!(restarts.stopped(later + BROWSE_STABLE), BROWSE_RETRY_FIRST);
        assert_eq!(
            restarts.stopped(later + BROWSE_STABLE),
            2 * BROWSE_RETRY_FIRST
        );
    }

    /// The browse stops as it does when mdns-sd's event queue fills, and a
    /// new one starts.
    #[tokio::test]
    async fn browsing_starts_again_after_it_stops() {
        let (_tx, rx) = watch::channel(keys());
        let mut discovery = LanDiscovery::new(LanConfig {
            beacon_port: 0,
            mdns: true,
        });
        discovery.start(rx).await.unwrap();
        let running = discovery.running.as_ref().unwrap();
        let Some(browser) = running.browser.as_ref() else {
            // No interface here can do mDNS.
            return;
        };
        browser.stop_browse();
        let restarted = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                tokio::select! {
                    _ = discovery.next_event() => {}
                    () = tokio::time::sleep(Duration::from_millis(100)) => {}
                }
                let running = discovery.running.as_ref().unwrap();
                if running.browser.is_some() && running.browse_again.is_none() {
                    break;
                }
            }
        })
        .await;
        assert!(restarted.is_ok(), "browsing started again");
        discovery.stop().await.unwrap();
    }

    #[test]
    fn scope_ids_survive_with_port() {
        let source: SocketAddr = "[fe80::1%3]:50000".parse().unwrap_or_else(|_| {
            SocketAddr::V6(SocketAddrV6::new("fe80::1".parse().unwrap(), 50000, 0, 3))
        });
        let SocketAddr::V6(out) = with_port(source, 27380) else {
            panic!("IPv6 stays IPv6")
        };
        assert_eq!((out.port(), out.scope_id()), (27380, 3));
    }
}
