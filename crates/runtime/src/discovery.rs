//! Discovery, the network watcher and the banner response pump: the
//! background tasks that feed the rest of the runtime.

use std::collections::BTreeSet;
use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use link2linux_core::discovery::beacon::BeaconSender;
use link2linux_core::discovery::onlink::local_addresses;
use link2linux_core::discovery::{
    Discovery, DiscoveryEvent, LanConfig, LanDiscovery, Match, Sighting,
};
use tokio::time;

use crate::link::{Command, lock};
use crate::pairing;
use crate::shared::Core;

/// How often the machine's addresses are compared to notice a change.
const NETWORK_POLL: Duration = Duration::from_secs(5);
/// How long to wait after the sink failed before asking it again.
const SINK_RETRY: Duration = Duration::from_secs(10);

/// Browses and listens until the runtime stops, and routes what it finds.
pub(crate) async fn run_discovery(core: Arc<Core>, config: LanConfig) {
    let mut discovery = LanDiscovery::new(config);
    if let Err(error) = discovery.start(core.keys.subscribe()).await {
        tracing::warn!(%error, "discovery is unavailable; phones are found by address only");
        return;
    }
    if let Some(bound) = discovery.beacon_addrs().first() {
        core.beacon_port.store(bound.port(), Ordering::Relaxed);
    }
    loop {
        tokio::select! {
            () = core.cancel.cancelled() => break,
            event = discovery.next_event() => match event {
                Some(event) => route(&core, event),
                None => break,
            },
        }
    }
    let _ = discovery.stop().await;
}

/// Sends one discovery event to whoever cares.
pub(crate) fn route(core: &Arc<Core>, event: DiscoveryEvent) {
    match event {
        DiscoveryEvent::Found(sighting) | DiscoveryEvent::Updated(sighting) => {
            sighted(core, &sighting);
        }
        DiscoveryEvent::Lost { instance } => {
            lock(&core.evidence).withdrawn(&instance);
            pairing::phone_listed(core, &instance, None);
        }
        DiscoveryEvent::Beacon(beacon) => match beacon.sender {
            BeaconSender::Paired(phone_id) => {
                lock(&core.evidence).beacon(&phone_id, beacon.address, Instant::now());
                wake(core, &phone_id);
            }
            BeaconSender::Pairing => pairing::offer_seen(core, &[beacon.address]),
        },
    }
}

fn sighted(core: &Arc<Core>, sighting: &Sighting) {
    let record = &sighting.record;
    for matched in &sighting.matches {
        match matched {
            Match::Paired { phone_id } => {
                lock(&core.evidence).mdns(phone_id, &record.instance, record.addresses.clone());
                wake(core, phone_id);
            }
            Match::Offer => pairing::offer_seen(core, &record.addresses),
            Match::PairingMode => {
                pairing::phone_listed(core, &record.instance, Some(record.addresses.clone()));
            }
        }
    }
}

/// Tells a phone's task that discovery saw it.
fn wake(core: &Core, phone_id: &str) {
    if let Some(slot) = lock(&core.slots).get(phone_id) {
        slot.wake.notify_one();
    }
}

/// Tells every phone's task when this machine's addresses change, so
/// their reconnect schedules start over.
pub(crate) async fn watch_network(core: Arc<Core>) {
    let snapshot = || -> BTreeSet<(IpAddr, u8)> {
        local_addresses()
            .unwrap_or_default()
            .into_iter()
            .map(|local| (local.ip, local.prefix_len))
            .collect()
    };
    let mut known = snapshot();
    loop {
        tokio::select! {
            () = core.cancel.cancelled() => return,
            () = time::sleep(NETWORK_POLL) => {}
        }
        let now = snapshot();
        if now != known {
            known = now;
            for slot in lock(&core.slots).values() {
                let _ = slot.commands.send(Command::NetworkChanged);
            }
        }
    }
}

/// Passes what the user does with banners to every phone's task, which
/// knows whether the banner is its own.
pub(crate) async fn pump_responses(core: Arc<Core>) {
    loop {
        let response = tokio::select! {
            () = core.cancel.cancelled() => return,
            response = core.presenter.next_response() => response,
        };
        match response {
            Ok(response) => {
                for slot in lock(&core.slots).values() {
                    let _ = slot.commands.send(Command::Response(response.clone()));
                }
            }
            Err(error) => {
                tracing::debug!(%error, "no banner responses available");
                tokio::select! {
                    () = core.cancel.cancelled() => return,
                    () = time::sleep(SINK_RETRY) => {}
                }
            }
        }
    }
}
