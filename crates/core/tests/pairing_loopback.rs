//! The desktop's TLS client against a stand-in phone over real TCP on the
//! loopback interface: the key pins on both sides, ALPN, and key rotation.

mod support;

use std::net::{IpAddr, Ipv4Addr};
use std::time::SystemTime;

use link2linux_core::device::DeviceKind;
use link2linux_core::identity::{CertificateFingerprint, Identity};
use link2linux_core::pairing::{MemoryTrustStore, PairedDevice, TrustStore, records};
use link2linux_core::transport::{Connection, PeerPin, TlsDialer, TransportError};
use link2linux_protocol::Hello;
use link2linux_protocol::v1::{KeyRotation, PairingMethod};
use support::{PhoneTls, TestPhone, Wire, hello};

const LOCALHOST: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

struct Desktop {
    identity: Identity,
    dialer: TlsDialer,
}

impl Desktop {
    fn new() -> Self {
        let identity = Identity::generate().unwrap();
        let dialer = TlsDialer::new(&identity).unwrap();
        Self { identity, dialer }
    }

    fn fingerprint(&self) -> CertificateFingerprint {
        self.identity.fingerprint()
    }

    fn hello(&self, method: PairingMethod) -> Hello {
        hello("desktop-1", "Desk", DeviceKind::Desktop, method)
    }
}

#[tokio::test]
async fn the_desktop_refuses_a_phone_with_another_key() {
    let desktop = Desktop::new();
    let paired = Identity::generate().unwrap().fingerprint();
    let phone = TestPhone::bind(LOCALHOST, "stranger").await;

    let desktop_side = desktop.dialer.dial(phone.addr(), PeerPin::from(paired));
    let phone_side = phone.accept(PhoneTls::pinned(desktop.fingerprint()));
    let (dialed, accepted) = tokio::join!(desktop_side, phone_side);

    match dialed {
        Err(TransportError::Untrusted(presented)) => assert_eq!(presented, phone.fingerprint()),
        other => panic!("expected Untrusted, got {other:?}"),
    }
    assert!(accepted.is_err());
}

#[tokio::test]
async fn the_phone_refuses_a_desktop_it_did_not_pin() {
    let desktop = Desktop::new();
    let other_desktop = Identity::generate().unwrap().fingerprint();
    let phone = TestPhone::bind(LOCALHOST, "phone-1").await;

    let desktop_side = async {
        // TLS 1.3 lets the client finish before the server has checked its
        // certificate, so the refusal may only show on the first read.
        let connection = desktop.dialer.dial(phone.addr(), PeerPin::Pairing).await?;
        let mut wire = Wire::new(connection);
        wire.hello(desktop.hello(PairingMethod::QrCode))
            .await
            .map_err(support::transport_error)
    };
    let phone_side = phone.accept(PhoneTls::pinned(other_desktop));
    let (desktop_result, accepted) = tokio::join!(desktop_side, phone_side);

    assert!(
        matches!(desktop_result, Err(TransportError::RejectedByPeer(_))),
        "{desktop_result:?}"
    );
    assert!(accepted.is_err());
}

#[tokio::test]
async fn the_desktop_requires_the_alpn_protocol() {
    let desktop = Desktop::new();
    let phone = TestPhone::bind(LOCALHOST, "phone-1").await;
    let tls = PhoneTls {
        alpn: false,
        ..PhoneTls::sas_mode()
    };

    let (dialed, _) = tokio::join!(
        desktop.dialer.dial(phone.addr(), PeerPin::Pairing),
        phone.accept(tls)
    );
    assert!(
        matches!(dialed, Err(TransportError::NoApplicationProtocol)),
        "{dialed:?}"
    );
}

#[tokio::test]
async fn a_rotated_key_is_accepted_once_announced_and_then_replaces_the_old_one() {
    let desktop = Desktop::new();
    let old_key = TestPhone::bind(LOCALHOST, "phone-1").await;
    let new_key = TestPhone::bind(LOCALHOST, "phone-1").await;
    let mut record = PairedDevice::new(
        "phone-1",
        "Pixel",
        DeviceKind::Phone,
        old_key.fingerprint(),
        SystemTime::now(),
    );
    record.confirmed = true;
    let mut store = MemoryTrustStore::default();
    store.insert(record).unwrap();

    // Before the announcement, the new key is a stranger.
    let pin = store.get(&old_key.fingerprint()).unwrap().pin();
    let (dialed, _) = tokio::join!(
        desktop.dialer.dial(new_key.addr(), pin),
        new_key.accept(PhoneTls::pinned(desktop.fingerprint()))
    );
    assert!(
        matches!(dialed, Err(TransportError::Untrusted(_))),
        "{dialed:?}"
    );

    let rotation = KeyRotation {
        next_spki_sha256: new_key.fingerprint().as_bytes().to_vec(),
    };
    records::accept_key_rotation(&mut store, &old_key.fingerprint(), &rotation).unwrap();

    let pin = store.get(&old_key.fingerprint()).unwrap().pin();
    let (dialed, accepted) = tokio::join!(
        desktop.dialer.dial(new_key.addr(), pin),
        new_key.accept(PhoneTls::pinned(desktop.fingerprint()))
    );
    let connection = dialed.unwrap();
    let presented = connection.peer_fingerprint();
    let record = records::note_connected(&mut store, &presented, connection.peer_addr()).unwrap();
    assert_eq!(record.fingerprint, new_key.fingerprint());
    assert_eq!(record.next_fingerprint, None);
    drop((connection, accepted));

    // The old key is no longer accepted.
    let pin = record.pin();
    let (dialed, _) = tokio::join!(
        desktop.dialer.dial(old_key.addr(), pin),
        old_key.accept(PhoneTls::pinned(desktop.fingerprint()))
    );
    assert!(
        matches!(dialed, Err(TransportError::Untrusted(_))),
        "{dialed:?}"
    );
}
