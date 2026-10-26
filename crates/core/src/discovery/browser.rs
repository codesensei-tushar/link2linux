//! Browsing for phones with mDNS.
//!
//! Uses the `mdns-sd` crate's own mDNS responder thread, so the desktop
//! does not depend on a system mDNS daemon. The browser turns the crate's
//! events into [`BrowserEvent`]s; deciding what matters is
//! [`PhoneTracker`](super::tracker::PhoneTracker)'s job.

use std::net::{IpAddr, Ipv6Addr, SocketAddr, SocketAddrV6};

use mdns_sd::{Receiver, ResolvedService, ScopedIp, ServiceDaemon, ServiceEvent};

use super::DiscoveryError;
use super::presence::TxtRecord;
use super::tracker::PhoneRecord;

/// The DNS-SD service type phones announce.
pub const SERVICE_TYPE: &str = "_link2linux._tcp.local.";

/// What the network said about a phone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrowserEvent {
    /// A phone resolved: a new announcement, or an updated one.
    Resolved(PhoneRecord),
    /// A phone's service was withdrawn, by instance name.
    Removed(String),
}

/// A running mDNS browse for [`SERVICE_TYPE`].
pub struct MdnsBrowser {
    daemon: ServiceDaemon,
    events: Receiver<ServiceEvent>,
}

impl std::fmt::Debug for MdnsBrowser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MdnsBrowser").finish_non_exhaustive()
    }
}

impl MdnsBrowser {
    /// Starts a responder thread and browses on all usable interfaces.
    ///
    /// # Errors
    ///
    /// [`DiscoveryError::Unavailable`] if the responder cannot start, for
    /// example when no interface can do multicast.
    pub fn start() -> Result<Self, DiscoveryError> {
        let daemon = ServiceDaemon::new().map_err(unavailable)?;
        Self::with_daemon(daemon)
    }

    /// Browses with an existing daemon.
    ///
    /// # Errors
    ///
    /// [`DiscoveryError::Unavailable`] if the browse cannot start.
    pub fn with_daemon(daemon: ServiceDaemon) -> Result<Self, DiscoveryError> {
        let events = daemon.browse(SERVICE_TYPE).map_err(unavailable)?;
        Ok(Self { daemon, events })
    }

    /// Waits for the next phone event. Returns `None` once the browse has
    /// stopped. Safe to cancel.
    pub async fn next(&self) -> Option<BrowserEvent> {
        loop {
            match self.events.recv_async().await.ok()? {
                ServiceEvent::ServiceResolved(service) => {
                    if let Some(record) = phone_record(&service) {
                        return Some(BrowserEvent::Resolved(record));
                    }
                }
                ServiceEvent::ServiceRemoved(_, fullname) => {
                    return Some(BrowserEvent::Removed(instance_name(&fullname).to_owned()));
                }
                ServiceEvent::SearchStopped(_) => return None,
                _ => {}
            }
        }
    }

    /// Stops browsing and the responder thread.
    pub fn shutdown(&self) {
        // The daemon may already be down; nothing more to do then.
        let _ = self.daemon.stop_browse(SERVICE_TYPE);
        let _ = self.daemon.shutdown();
    }
}

fn unavailable(error: mdns_sd::Error) -> DiscoveryError {
    DiscoveryError::Unavailable(format!("mDNS: {error}"))
}

/// `l2l-3f9a12bc._link2linux._tcp.local.` to `l2l-3f9a12bc`.
fn instance_name(fullname: &str) -> &str {
    fullname
        .strip_suffix(SERVICE_TYPE)
        .map_or(fullname, |name| name.trim_end_matches('.'))
}

fn phone_record(service: &ResolvedService) -> Option<PhoneRecord> {
    if service.port == 0 {
        return None;
    }
    let mut addresses: Vec<SocketAddr> = service
        .addresses
        .iter()
        .filter_map(|ip| socket_addr(ip, service.port))
        .collect();
    if addresses.is_empty() {
        return None;
    }
    addresses.sort();
    let txt = TxtRecord::parse(
        service
            .txt_properties
            .iter()
            .map(|property| (property.key(), property.val_str())),
    );
    Some(PhoneRecord {
        instance: instance_name(&service.fullname).to_owned(),
        addresses,
        txt,
    })
}

fn socket_addr(ip: &ScopedIp, port: u16) -> Option<SocketAddr> {
    match ip {
        ScopedIp::V4(_) => Some(SocketAddr::new(ip.to_ip_addr(), port)),
        ScopedIp::V6(v6) => {
            let addr: Ipv6Addr = *v6.addr();
            // A link-local address is only meaningful with its interface.
            let scope = if addr.is_unicast_link_local() {
                v6.scope_id().index
            } else {
                0
            };
            Some(SocketAddr::V6(SocketAddrV6::new(addr, port, 0, scope)))
        }
        _ => None,
    }
    .filter(|addr| {
        !addr.ip().is_unspecified() && !matches!(addr.ip(), IpAddr::V4(v4) if v4.is_broadcast())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mdns_sd::ServiceInfo;
    use std::time::Duration;

    #[test]
    fn instance_names_lose_the_service_type() {
        assert_eq!(
            instance_name("l2l-3f9a12bc._link2linux._tcp.local."),
            "l2l-3f9a12bc"
        );
        assert_eq!(instance_name("odd"), "odd");
    }

    /// A phone announced by one responder is found by another, over the
    /// loopback interface.
    #[tokio::test]
    #[ignore = "needs multicast on the loopback interface"]
    async fn finds_an_announced_phone_on_loopback() {
        let announcer = ServiceDaemon::new().unwrap();
        let info = ServiceInfo::new(
            SERVICE_TYPE,
            "l2l-test0001",
            "l2l-test-host.local.",
            "127.0.0.1",
            27380,
            &[("v", "1"), ("pm", "1"), ("extra", "ignored")][..],
        )
        .unwrap();
        announcer.register(info).unwrap();

        let browser = MdnsBrowser::start().unwrap();
        let found = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let Some(BrowserEvent::Resolved(record)) = browser.next().await
                    && record.instance == "l2l-test0001"
                {
                    return record;
                }
            }
        })
        .await
        .expect("the phone resolves within 10 s");
        assert_eq!(found.addresses, vec!["127.0.0.1:27380".parse().unwrap()]);
        assert_eq!(found.txt.version, Some(1));
        assert!(found.txt.pairing_mode);
        browser.shutdown();
        let _ = announcer.shutdown();
    }
}
