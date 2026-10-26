//! The desktop's TLS client and pairing against a stand-in phone over real
//! TCP on the loopback interface: QR code and SAS pairing end to end, the
//! failure rules of a QR offer, and the key pins on both sides.

mod support;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use link2linux_core::device::{DeviceInfo, DeviceKind};
use link2linux_core::identity::{CertificateFingerprint, Identity};
use link2linux_core::pairing::{
    MAX_FAILED_PROOFS, MemoryTrustStore, OFFER_LIFETIME, OfferStatus, PairedDevice, PairingBinding,
    PairingError, PairingSecret, PairingSuccess, PairingTimeouts, QrOffer, QrPayload, SasDecision,
    TrustStore, pair_by_qr, pair_by_sas, records,
};
use link2linux_core::transport::{Connection, PeerPin, TlsConnection, TlsDialer, TransportError};
use link2linux_protocol::v1::{KeyRotation, PairingMethod, PairingOutcome};
use link2linux_protocol::{Hello, Payload, Ping, Pong};
use support::{PhoneReport, PhoneTls, ScriptedPrompt, TestPhone, Wire, device_info, hello};
use tokio::time::Instant;

const LOCALHOST: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

/// Another loopback address, so a test phone can stand at its own address.
fn loopback(last: u8) -> IpAddr {
    Ipv4Addr::new(127, 0, 0, last).into()
}

struct Desktop {
    identity: Identity,
    dialer: TlsDialer,
    store: Mutex<MemoryTrustStore>,
}

impl Desktop {
    fn new() -> Self {
        let identity = Identity::generate().unwrap();
        let dialer = TlsDialer::new(&identity).unwrap();
        Self {
            identity,
            dialer,
            store: Mutex::default(),
        }
    }

    fn fingerprint(&self) -> CertificateFingerprint {
        self.identity.fingerprint()
    }

    fn hello(&self, method: PairingMethod) -> Hello {
        hello("desktop-1", "Desk", DeviceKind::Desktop, method)
    }

    fn offer(&self) -> Mutex<QrOffer> {
        let beacons = SocketAddr::new(LOCALHOST, 27381);
        Mutex::new(QrOffer::new(
            self.fingerprint(),
            "Desk",
            vec![beacons],
            None,
        ))
    }

    /// Dials to pair, derives the binding and exchanges `Hello`s.
    async fn dial_to_pair(
        &self,
        addr: SocketAddr,
        method: PairingMethod,
    ) -> (Wire<TlsConnection>, PairingBinding, DeviceInfo) {
        let connection = self.dialer.dial(addr, PeerPin::Pairing).await.unwrap();
        let binding = PairingBinding::from_connection(&connection, method).unwrap();
        let mut wire = Wire::new(connection);
        let phone_hello = wire.hello(self.hello(method)).await.unwrap();
        (wire, binding, device_info(&phone_hello))
    }

    fn records(&self) -> Vec<PairedDevice> {
        self.store.lock().unwrap().list()
    }
}

/// One QR code pairing attempt between `desktop` and `phone`, where the
/// phone proves `scanned`. Returns both sides' views.
async fn qr_attempt(
    desktop: &Desktop,
    phone: &TestPhone,
    offer: &Mutex<QrOffer>,
    scanned: &PairingSecret,
    prompt: &mut ScriptedPrompt,
) -> (Result<PairingSuccess, PairingError>, PhoneReport) {
    let desktop_side = async {
        let (mut wire, binding, peer) = desktop
            .dial_to_pair(phone.addr(), PairingMethod::QrCode)
            .await;
        let timeouts = PairingTimeouts::default();
        let result = pair_by_qr(
            &mut wire,
            &binding,
            &peer,
            offer,
            &desktop.store,
            prompt,
            &timeouts,
        )
        .await;
        wire.finish().await;
        result
    };
    let phone_side = async {
        let mut connection = phone
            .accept(PhoneTls::pinned(desktop.fingerprint()))
            .await
            .unwrap();
        let report = connection.pair_by_qr(phone, scanned).await;
        connection.wire.finish().await;
        report
    };
    tokio::join!(desktop_side, phone_side)
}

/// One SAS pairing attempt.
async fn sas_attempt(
    desktop: &Desktop,
    phone: &TestPhone,
    prompt: &mut ScriptedPrompt,
    tamper: bool,
    phone_decision: SasDecision,
) -> (Result<PairingSuccess, PairingError>, PhoneReport) {
    let desktop_side = async {
        let (mut wire, binding, peer) =
            desktop.dial_to_pair(phone.addr(), PairingMethod::Sas).await;
        let timeouts = PairingTimeouts::default();
        let result = pair_by_sas(
            &mut wire,
            &binding,
            &peer,
            &desktop.store,
            prompt,
            &timeouts,
        )
        .await;
        wire.finish().await;
        result
    };
    let phone_side = async {
        let mut connection = phone.accept(PhoneTls::sas_mode()).await.unwrap();
        let report = connection.pair_by_sas(phone, tamper, phone_decision).await;
        connection.wire.finish().await;
        report
    };
    tokio::join!(desktop_side, phone_side)
}

fn confirming() -> ScriptedPrompt {
    ScriptedPrompt::answering(SasDecision::Confirmed, true)
}

#[tokio::test]
async fn qr_code_pairing_end_to_end_then_a_pinned_session() {
    let desktop = Desktop::new();
    let phone = TestPhone::bind(LOCALHOST, "phone-1").await;
    let offer = desktop.offer();
    // The phone scans the code: it learns the desktop's key and the secret.
    let scanned = QrPayload::parse(offer.lock().unwrap().uri()).unwrap();
    assert_eq!(scanned.desktop, desktop.fingerprint());

    let desktop_side = async {
        let (mut wire, binding, peer) = desktop
            .dial_to_pair(phone.addr(), PairingMethod::QrCode)
            .await;
        let timeouts = PairingTimeouts::default();
        let mut prompt = confirming();
        let result = pair_by_qr(
            &mut wire,
            &binding,
            &peer,
            &offer,
            &desktop.store,
            &mut prompt,
            &timeouts,
        )
        .await;
        // The connection goes on as a session.
        wire.send_payload(Payload::Ping(Ping { nonce: 7 }))
            .await
            .unwrap();
        let pong = wire.recv_payload().await.unwrap();
        wire.finish().await;
        (result.unwrap(), *binding.ekm(), pong)
    };
    let phone_side = async {
        let mut connection = phone
            .accept(PhoneTls::pinned(scanned.desktop))
            .await
            .unwrap();
        let report = connection.pair_by_qr(&phone, &scanned.secret).await;
        let Some(Payload::Ping(ping)) = connection.wire.recv_payload().await.unwrap() else {
            panic!("expected a Ping");
        };
        let pong = Payload::Pong(Pong { nonce: ping.nonce });
        connection.wire.send_payload(pong).await.unwrap();
        let seen = connection.desktop;
        connection.wire.finish().await;
        (report, seen)
    };
    let ((success, desktop_ekm, pong), (report, seen_desktop)) =
        tokio::join!(desktop_side, phone_side);

    assert_eq!(report.desktop_outcome, Some(PairingOutcome::Accepted));
    // Both TLS stacks exported the same value for the same label and context.
    assert_eq!(report.ekm, desktop_ekm);
    assert_eq!(seen_desktop, desktop.fingerprint());
    assert_eq!(pong, Some(Payload::Pong(Pong { nonce: 7 })));
    assert_eq!(success.device.fingerprint, phone.fingerprint());
    assert_eq!(success.device.id, "phone-1");
    assert_eq!(success.device.kind, DeviceKind::Phone);
    assert!(!success.device.confirmed);
    assert!(success.replaced.is_empty());
    assert_eq!(desktop.records(), std::slice::from_ref(&success.device));
    assert_eq!(
        offer.lock().unwrap().status(Instant::now()),
        OfferStatus::Used
    );

    // The next connection is pinned both ways, and the Hello is checked
    // against the record found by key.
    let record = success.device;
    let desktop_side = async {
        let connection = desktop
            .dialer
            .dial(phone.addr(), record.pin())
            .await
            .unwrap();
        let presented = connection.peer_fingerprint();
        let mut wire = Wire::new(connection);
        let hello = wire
            .hello(desktop.hello(PairingMethod::Unspecified))
            .await
            .unwrap();
        let checked = records::check_peer(
            &*desktop.store.lock().unwrap(),
            &presented,
            &hello.device_id,
        );
        wire.finish().await;
        checked
    };
    let phone_side = async {
        let mut connection = phone
            .accept(PhoneTls::pinned(desktop.fingerprint()))
            .await
            .unwrap();
        connection.wire.hello(phone.hello()).await.unwrap();
        connection.wire.finish().await;
    };
    let (checked, ()) = tokio::join!(desktop_side, phone_side);
    assert_eq!(checked.unwrap().id, "phone-1");
}

#[tokio::test]
async fn sas_pairing_shows_the_same_code_on_both_screens() {
    let desktop = Desktop::new();
    let phone = TestPhone::bind(LOCALHOST, "phone-sas").await;
    let mut prompt = confirming();

    let (result, report) =
        sas_attempt(&desktop, &phone, &mut prompt, false, SasDecision::Confirmed).await;

    let success = result.unwrap();
    let phone_code = report.code.unwrap();
    assert_eq!(prompt.codes(), std::slice::from_ref(&phone_code));
    assert_eq!(phone_code.as_str().len(), 6);
    assert_eq!(report.desktop_outcome, Some(PairingOutcome::Accepted));
    assert_eq!(success.device.fingerprint, phone.fingerprint());
    assert_eq!(desktop.records().len(), 1);
}

#[tokio::test]
async fn sas_pairing_fails_on_a_broken_commitment_or_a_no() {
    let desktop = Desktop::new();
    let phone = TestPhone::bind(LOCALHOST, "phone-sas").await;

    // The phone reveals another nonce than it committed to.
    let mut prompt = confirming();
    let (result, report) =
        sas_attempt(&desktop, &phone, &mut prompt, true, SasDecision::Confirmed).await;
    assert!(
        matches!(result, Err(PairingError::VerificationFailed)),
        "{result:?}"
    );
    assert_eq!(
        report.desktop_outcome,
        Some(PairingOutcome::VerificationFailed)
    );
    assert!(
        prompt.codes().is_empty(),
        "no code is shown for a broken commitment"
    );

    // The desktop's user says the codes differ.
    let mut prompt = ScriptedPrompt::answering(SasDecision::Declined, true);
    let (result, report) =
        sas_attempt(&desktop, &phone, &mut prompt, false, SasDecision::Confirmed).await;
    assert!(matches!(result, Err(PairingError::Declined)), "{result:?}");
    assert_eq!(report.desktop_outcome, Some(PairingOutcome::Declined));

    // The phone's user says so.
    let mut prompt = confirming();
    let (result, _) =
        sas_attempt(&desktop, &phone, &mut prompt, false, SasDecision::Declined).await;
    assert!(
        matches!(
            result,
            Err(PairingError::Rejected(PairingOutcome::Declined))
        ),
        "{result:?}"
    );
    assert!(desktop.records().is_empty());
}

#[tokio::test]
async fn a_wrong_secret_fails_and_blocks_only_that_phone() {
    let desktop = Desktop::new();
    let offer = desktop.offer();
    let secret = offer.lock().unwrap().secret().clone();
    let impostor = TestPhone::bind(loopback(2), "impostor").await;
    let wrong = PairingSecret::from_bytes([0; 16]);

    let (result, _) = qr_attempt(&desktop, &impostor, &offer, &wrong, &mut confirming()).await;
    assert!(
        matches!(result, Err(PairingError::VerificationFailed)),
        "{result:?}"
    );
    {
        let offer = offer.lock().unwrap();
        assert_eq!(offer.failed_proofs(), 1);
        assert_eq!(offer.status(Instant::now()), OfferStatus::Active);
        assert!(!offer.may_dial(loopback(2)));
        assert!(offer.is_blocked(&impostor.fingerprint(), loopback(3)));
    }

    // The same phone again is refused before any proof is sent.
    let (result, report) = qr_attempt(&desktop, &impostor, &offer, &wrong, &mut confirming()).await;
    assert!(matches!(result, Err(PairingError::Blocked)), "{result:?}");
    assert_eq!(report.desktop_outcome, Some(PairingOutcome::NotExpected));
    assert_eq!(offer.lock().unwrap().failed_proofs(), 1);

    // The phone that really scanned the code still pairs with it.
    let phone = TestPhone::bind(LOCALHOST, "phone-1").await;
    let (result, report) = qr_attempt(&desktop, &phone, &offer, &secret, &mut confirming()).await;
    assert_eq!(result.unwrap().device.fingerprint, phone.fingerprint());
    assert_eq!(report.desktop_outcome, Some(PairingOutcome::Accepted));
    assert_eq!(desktop.records().len(), 1);
}

#[tokio::test]
async fn too_many_failed_proofs_withdraw_the_offer() {
    let desktop = Desktop::new();
    let offer = desktop.offer();
    let secret = offer.lock().unwrap().secret().clone();
    let wrong = PairingSecret::from_bytes([1; 16]);

    let attempts = u8::try_from(MAX_FAILED_PROOFS).unwrap();
    for n in 0..attempts {
        // Each impostor has its own key and address, so none is blocked.
        let impostor = TestPhone::bind(loopback(10 + n), "impostor").await;
        let (result, _) = qr_attempt(&desktop, &impostor, &offer, &wrong, &mut confirming()).await;
        assert!(
            matches!(result, Err(PairingError::VerificationFailed)),
            "{result:?}"
        );
    }
    assert_eq!(
        offer.lock().unwrap().status(Instant::now()),
        OfferStatus::Withdrawn
    );

    let phone = TestPhone::bind(LOCALHOST, "phone-1").await;
    let (result, report) = qr_attempt(&desktop, &phone, &offer, &secret, &mut confirming()).await;
    assert!(
        matches!(
            result,
            Err(PairingError::OfferUnavailable(OfferStatus::Withdrawn))
        ),
        "{result:?}"
    );
    assert_eq!(report.desktop_outcome, Some(PairingOutcome::NotExpected));
    assert!(desktop.records().is_empty());
}

#[tokio::test]
async fn an_expired_offer_is_not_expected() {
    let desktop = Desktop::new();
    let Some(created) = Instant::now().checked_sub(OFFER_LIFETIME + Duration::from_secs(1)) else {
        return; // The monotonic clock started less than five minutes ago.
    };
    let payload = QrPayload::parse(desktop.offer().lock().unwrap().uri()).unwrap();
    let secret = payload.secret.clone();
    let offer = Mutex::new(QrOffer::from_payload(payload, created));
    let phone = TestPhone::bind(LOCALHOST, "phone-1").await;

    let (result, report) = qr_attempt(&desktop, &phone, &offer, &secret, &mut confirming()).await;
    assert!(
        matches!(
            result,
            Err(PairingError::OfferUnavailable(OfferStatus::Expired))
        ),
        "{result:?}"
    );
    assert_eq!(report.desktop_outcome, Some(PairingOutcome::NotExpected));
}

#[tokio::test]
async fn re_pairing_over_another_key_asks_first() {
    let desktop = Desktop::new();
    let phone = TestPhone::bind(LOCALHOST, "phone-1").await;
    // An old record for the same device id, under a key the phone lost.
    let old = PairedDevice::new(
        "phone-1",
        "Old phone",
        DeviceKind::Phone,
        CertificateFingerprint([9; 32]),
        SystemTime::UNIX_EPOCH,
    );
    desktop.store.lock().unwrap().insert(old.clone()).unwrap();

    // The user keeps the old record: the pairing is declined.
    let offer = desktop.offer();
    let secret = offer.lock().unwrap().secret().clone();
    let mut refusing = ScriptedPrompt::answering(SasDecision::Confirmed, false);
    let (result, report) = qr_attempt(&desktop, &phone, &offer, &secret, &mut refusing).await;
    assert!(matches!(result, Err(PairingError::Declined)), "{result:?}");
    assert_eq!(report.desktop_outcome, Some(PairingOutcome::Declined));
    assert_eq!(desktop.records(), std::slice::from_ref(&old));
    assert_eq!(
        refusing.shown.lock().unwrap()[0].1,
        std::slice::from_ref(&old)
    );

    // The user agrees: the new record replaces the old one.
    let (result, _) = qr_attempt(&desktop, &phone, &offer, &secret, &mut confirming()).await;
    let success = result.unwrap();
    assert_eq!(success.replaced, [old]);
    assert_eq!(desktop.records(), [success.device]);
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
