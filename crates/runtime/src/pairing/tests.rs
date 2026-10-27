//! QR pairing connections against copied pairing tags and a phone reached
//! at two addresses.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use link2linux_core::discovery::LocalAddress;
use link2linux_core::identity::{CertificateFingerprint, Identity};
use link2linux_core::pairing::crypto::{self, Role};
use link2linux_platform::{
    DesktopNotification, NotificationCapabilities, NotificationId, NotificationResponse,
    NotificationSink, PlatformError,
};
use link2linux_protocol::Payload;
use link2linux_protocol::v1::{PairingOutcome, PairingProof, PairingResult};
use tokio::runtime::Handle;

use super::state::MAX_CONNECTIONS;
use super::*;
use crate::notify::{Presenter, ServiceFactory};
use crate::paths::DataDir;
use crate::shared::{RuntimeConfig, Shell};
use crate::test_phone::{PhoneTls, TestPhone};
use crate::view::{Device, EventSink};

const WAIT: Duration = Duration::from_secs(20);

struct NoPopups;

impl NotificationSink for NoPopups {
    async fn show(
        &self,
        _: &DesktopNotification,
        _: Option<NotificationId>,
    ) -> Result<NotificationId, PlatformError> {
        Ok(NotificationId(1))
    }

    async fn close(&self, _: NotificationId) -> Result<(), PlatformError> {
        Ok(())
    }

    async fn capabilities(&self) -> Result<NotificationCapabilities, PlatformError> {
        Ok(NotificationCapabilities::default())
    }

    async fn next_response(&self) -> Result<NotificationResponse, PlatformError> {
        std::future::pending().await
    }
}

struct NoEvents;

impl EventSink for NoEvents {
    fn pairing_changed(&self, _: &PairingState) {}
    fn devices_changed(&self, _: &[Device]) {}
}

/// A desktop without discovery, on `handle`, whose network is `locals`.
fn desktop(handle: Handle, locals: Vec<LocalAddress>) -> (tempfile::TempDir, Arc<Core>) {
    let data = tempfile::tempdir().unwrap();
    let mut config = RuntimeConfig::new(DataDir::new(data.path().join("l2l")));
    config.discovery = None;
    config.desktop_name = Some("Desk".into());
    config.extra_locals = locals;
    let presenter: Arc<dyn Presenter> = Arc::new(NoPopups);
    let shell = Shell {
        identity: Identity::generate().unwrap(),
        store_key: None,
        drivers: Arc::new(ServiceFactory::new(presenter.clone(), None)),
        presenter,
        clipboard: None,
        files: None,
        calls: None,
        events: Arc::new(NoEvents),
    };
    let core = Core::open(handle, &config, shell).unwrap();
    (data, core)
}

#[test]
fn copies_of_the_tag_open_few_connections_and_leave_the_phone_one() {
    // The connections are never polled, so nothing leaves this machine.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let _entered = rt.enter();
    let local = LocalAddress {
        ip: IpAddr::V4(Ipv4Addr::new(10, 77, 0, 1)),
        prefix_len: 16,
        interface_index: None,
    };
    let (_data, core) = desktop(rt.handle().clone(), vec![local]);
    start(&core, PairingMethod::QrCode).unwrap();
    let at = |third: u8, last: u8| SocketAddr::from(([10, 77, third, last], 27380));
    let dialing = |address: SocketAddr| lock(&core.pairing).is_dialing(address.ip());
    let open = || lock(&core.pairing).attempts.len();

    // A forged record lists the phone's address after its own, and its
    // own address answers first.
    let (forged, phone) = (at(9, 8), at(9, 9));
    offer_seen(&core, Lead::Announcement, &[forged, phone]);
    let first = lock(&core.pairing).attempts[0].id;
    assert!(lock(&core.pairing).connected(first, forged.ip(), CertificateFingerprint([8; 32])));
    // The phone's own beacon still gets it a connection.
    offer_seen(&core, Lead::Beacon, &[phone]);
    assert!(dialing(phone));
    assert_eq!(open(), 2);

    // Copies of the tag, each with an address of its own.
    for i in 0..500u32 {
        let address = at(2 + (i / 250) as u8, 1 + (i % 250) as u8);
        offer_seen(&core, Lead::Announcement, &[address]);
    }
    assert_eq!(open(), MAX_CONNECTIONS);

    // Another beacon closes the oldest copy, which proved nothing.
    let other = at(9, 10);
    offer_seen(&core, Lead::Beacon, &[other]);
    assert!(dialing(other) && dialing(phone) && !dialing(forged));
    assert_eq!(open(), MAX_CONNECTIONS);

    // A connection that ends frees its slot for the next sighting.
    let copy = lock(&core.pairing)
        .attempts
        .iter()
        .find(|attempt| attempt.lead == Lead::Announcement)
        .map(|attempt| attempt.id)
        .unwrap();
    assert!(lock(&core.pairing).finished(copy).is_some());
    offer_seen(&core, Lead::Announcement, &[at(4, 1)]);
    assert!(dialing(at(4, 1)));
    assert_eq!(open(), MAX_CONNECTIONS);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_phone_reached_at_two_addresses_pairs_on_the_first() {
    let (_data, core) = desktop(Handle::current(), Vec::new());
    start(&core, PairingMethod::QrCode).unwrap();
    let (secret, desktop) = {
        let ctl = lock(&core.pairing);
        let offer = lock(ctl.offer.as_ref().unwrap());
        (offer.secret().clone(), offer.payload().desktop)
    };

    // One phone, one key, reachable at two addresses (its IPv4 and IPv6
    // ones on a real network).
    let identity = Identity::generate().unwrap();
    let copy = || Identity::from_pkcs8_pem(&identity.to_pkcs8_pem()).unwrap();
    let at_a = TestPhone::bind_with(IpAddr::V4(Ipv4Addr::LOCALHOST), "phone-1", copy()).await;
    let at_b =
        TestPhone::bind_with(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 3)), "phone-1", copy()).await;
    let (addr_a, addr_b) = (at_a.addr(), at_b.addr());
    let (b_current, a_hears) = tokio::sync::oneshot::channel::<bool>();

    let secret_a = secret.clone();
    let phone_a = tokio::spawn(async move {
        let mut c = at_a.accept(PhoneTls::pinned(desktop)).await.unwrap();
        c.wire.hello(at_a.hello()).await.unwrap();
        let mac = crypto::pairing_proof(&secret_a, Role::Phone, &c.ekm_pair());
        c.wire
            .send_payload(Payload::PairingProof(PairingProof { mac: mac.to_vec() }))
            .await
            .unwrap();
        let proof = c.wire.recv_payload().await.unwrap();
        assert!(matches!(proof, Some(Payload::PairingProof(_))));
        // As the app does: once a newer connection is current, an older
        // one is declined at approval.
        let newer = matches!(tokio::time::timeout(WAIT, a_hears).await, Ok(Ok(true)));
        let outcome = if newer {
            PairingOutcome::Declined
        } else {
            PairingOutcome::Accepted
        };
        let result = PairingResult {
            outcome: outcome.into(),
        };
        c.wire
            .send_payload(Payload::PairingResult(result))
            .await
            .unwrap();
        let verdict = tokio::time::timeout(WAIT, c.wire.recv_payload()).await;
        c.wire.finish().await;
        (outcome, verdict.ok().and_then(Result::ok).flatten())
    });
    let secret_b = secret.clone();
    let phone_b = tokio::spawn(async move {
        let mut c = at_b.accept(PhoneTls::pinned(desktop)).await.unwrap();
        // The connection becomes the current one once the desktop's Hello
        // arrives.
        let _ = c.wire.send_payload(Payload::Hello(at_b.hello())).await;
        let hello = tokio::time::timeout(WAIT, c.wire.recv_payload()).await;
        let current = matches!(hello, Ok(Ok(Some(Payload::Hello(_)))));
        let _ = b_current.send(current);
        if !current {
            return false;
        }
        let mac = crypto::pairing_proof(&secret_b, Role::Phone, &c.ekm_pair());
        let _ = c
            .wire
            .send_payload(Payload::PairingProof(PairingProof { mac: mac.to_vec() }))
            .await;
        let _ = c.wire.recv_payload().await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        let accepted = PairingResult {
            outcome: PairingOutcome::Accepted.into(),
        };
        let _ = c.wire.send_payload(Payload::PairingResult(accepted)).await;
        true
    });

    let dial = |address: SocketAddr| {
        let mut ctl = lock(&core.pairing);
        let plan = dial_plan(None, &[address], &[]);
        begin_connection(
            &core,
            &mut ctl,
            PairingMethod::QrCode,
            plan,
            None,
            Lead::Beacon,
        );
    };
    dial(addr_a);
    // The phone's second address is sighted once the first connection
    // reached it.
    tokio::time::timeout(WAIT, async {
        while !lock(&core.pairing)
            .attempts
            .iter()
            .any(|attempt| attempt.peer.is_some())
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    dial(addr_b);

    let last = tokio::time::timeout(WAIT, async {
        loop {
            let now = state(&core);
            if matches!(
                now,
                PairingState::Paired { .. } | PairingState::Failed { .. }
            ) {
                return now;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let (a_said, a_heard) = tokio::time::timeout(WAIT, phone_a).await.unwrap().unwrap();
    let b_was_current = tokio::time::timeout(WAIT, phone_b).await.unwrap().unwrap();
    core.cancel.cancel();

    assert!(matches!(last, PairingState::Paired { .. }), "{last:?}");
    assert!(
        !b_was_current,
        "the second connection closed before its Hello"
    );
    assert_eq!(a_said, PairingOutcome::Accepted);
    assert!(
        matches!(a_heard, Some(Payload::PairingResult(ref r)) if r.outcome() == PairingOutcome::Accepted),
        "{a_heard:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_peer_that_answers_first_frees_the_addresses_it_was_dialed_with() {
    let (_data, core) = desktop(Handle::current(), Vec::new());
    start(&core, PairingMethod::QrCode).unwrap();
    let desktop_key = {
        let ctl = lock(&core.pairing);
        lock(ctl.offer.as_ref().unwrap()).payload().desktop
    };
    let forged = TestPhone::bind(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 4)), "forged").await;
    let phone = TestPhone::bind(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 5)), "phone-1").await;
    let (forged_at, phone_at) = (forged.addr(), phone.addr());
    // The forged peer completes TLS at once, then stalls.
    let stall = tokio::spawn(async move {
        let _connection = forged.accept(PhoneTls::pinned(desktop_key)).await.unwrap();
        std::future::pending::<()>().await;
    });
    let dial = |plan, lead| {
        let mut ctl = lock(&core.pairing);
        begin_connection(&core, &mut ctl, PairingMethod::QrCode, plan, None, lead);
    };

    // A forged record lists the phone's address after its own.
    dial(
        dial_plan(None, &[forged_at, phone_at], &[]),
        Lead::Announcement,
    );
    assert!(lock(&core.pairing).is_dialing(phone_at.ip()));
    // Its own address answered first: the phone's is free while it stalls.
    tokio::time::timeout(WAIT, async {
        while lock(&core.pairing).is_dialing(phone_at.ip()) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(lock(&core.pairing).is_dialing(forged_at.ip()));

    // So the phone's own sighting reaches it.
    dial(dial_plan(None, &[phone_at], &[]), Lead::Beacon);
    let reached = tokio::time::timeout(WAIT, phone.accept(PhoneTls::pinned(desktop_key))).await;
    stall.abort();
    core.cancel.cancel();
    assert!(matches!(reached, Ok(Ok(_))));
}
