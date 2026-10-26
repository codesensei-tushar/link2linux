//! Desktop pairing on the session layer: QR code pairing over two sessions
//! on a loopback TLS connection, the phone's confirmation and presence key
//! that follow it, and pinned reconnects that check the peer's device id.

mod support;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Mutex;

use link2linux_core::device::{DeviceInfo, DeviceKind};
use link2linux_core::identity::{CertificateFingerprint, Identity};
use link2linux_core::pairing::crypto::{self, Role};
use link2linux_core::pairing::{
    MemoryTrustStore, PairingBinding, PairingTimeouts, PresenceKey, QrOffer, QrPayload,
    SasDecision, TrustStore, pair_by_qr, records,
};
use link2linux_core::session::{Session, SessionConfig, SessionError, SessionEvent, SessionStart};
use link2linux_core::transport::{Connection, PeerPin, TlsDialer};
use link2linux_protocol::Payload;
use link2linux_protocol::v1::{
    PairingMethod, PairingOutcome, PairingProof, PairingResult, PresenceUpdate,
};
use support::{PhoneTls, ScriptedPrompt, TestPhone};
use tokio::io::{AsyncRead, AsyncWrite};

const LOCALHOST: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);
const PRESENCE_KEY: [u8; 32] = [0x5a; 32];

fn desktop_info() -> DeviceInfo {
    DeviceInfo::new("desktop-1", "Desk", DeviceKind::Desktop, [])
}

/// The next pairing message on a session.
async fn next_message<S: AsyncRead + AsyncWrite + Unpin>(session: &mut Session<S>) -> Payload {
    loop {
        match session.next_event().await.unwrap() {
            Some(SessionEvent::Message { payload, .. }) => return payload,
            Some(_) => {}
            None => panic!("the session ended"),
        }
    }
}

/// Runs a session until the peer closes it.
async fn run_out<S: AsyncRead + AsyncWrite + Unpin>(mut session: Session<S>) {
    while let Ok(Some(_)) = session.next_event().await {}
}

/// The phone's side, on a session: proves the scanned secret, accepts the
/// desktop's proof, and starts the session proper once the desktop
/// accepted.
async fn phone_pairs(phone: &TestPhone, scanned: &QrPayload) {
    let connection = phone
        .accept(PhoneTls::pinned(scanned.desktop))
        .await
        .unwrap();
    let ekm = connection.ekm_pair();
    let mut start = SessionStart::new(DeviceInfo::new(
        &phone.id,
        &phone.name,
        DeviceKind::Phone,
        [],
    ));
    start.presence = PresenceUpdate {
        presence_key: PRESENCE_KEY.to_vec(),
        ..PresenceUpdate::default()
    };
    let stream = connection.wire.into_inner();
    let mut session = Session::handshake(stream, start, SessionConfig::phone())
        .await
        .unwrap();
    assert_eq!(session.pairing_method(), Some(PairingMethod::QrCode));

    let mac = crypto::pairing_proof(&scanned.secret, Role::Phone, &ekm);
    session
        .send(Payload::PairingProof(PairingProof { mac: mac.to_vec() }))
        .await
        .unwrap();
    let Payload::PairingProof(proof) = next_message(&mut session).await else {
        panic!("expected the desktop's proof");
    };
    assert!(crypto::verify_pairing_proof(
        &scanned.secret,
        Role::Desktop,
        &ekm,
        &proof.mac
    ));
    let accepted = PairingResult {
        outcome: PairingOutcome::Accepted.into(),
    };
    session
        .send(Payload::PairingResult(accepted))
        .await
        .unwrap();
    let Payload::PairingResult(verdict) = next_message(&mut session).await else {
        panic!("expected the desktop's verdict");
    };
    assert_eq!(verdict.outcome(), PairingOutcome::Accepted);
    // The phone stores the pairing here, then starts the session proper.
    session.finish_pairing().await.unwrap();
    run_out(session).await;
}

/// The phone's side of a later connection from a paired desktop.
async fn phone_answers(phone: &TestPhone, desktop: CertificateFingerprint) {
    let connection = phone.accept(PhoneTls::pinned(desktop)).await.unwrap();
    let start = SessionStart::new(DeviceInfo::new(
        &phone.id,
        &phone.name,
        DeviceKind::Phone,
        [],
    ));
    let stream = connection.wire.into_inner();
    if let Ok(session) = Session::handshake(stream, start, SessionConfig::phone()).await {
        run_out(session).await;
    }
}

#[tokio::test]
async fn qr_code_pairing_runs_on_sessions_and_the_phone_confirms_it() {
    let identity = Identity::generate().unwrap();
    let dialer = TlsDialer::new(&identity).unwrap();
    let store = Mutex::new(MemoryTrustStore::default());
    let beacons = SocketAddr::new(LOCALHOST, 27381);
    let offer = Mutex::new(QrOffer::new(
        identity.fingerprint(),
        "Desk",
        vec![beacons],
        None,
    ));
    let scanned = QrPayload::parse(offer.lock().unwrap().uri()).unwrap();
    let phone = TestPhone::bind(LOCALHOST, "phone-1").await;

    let desktop_side = async {
        let connection = dialer.dial(phone.addr(), PeerPin::Pairing).await.unwrap();
        let binding = PairingBinding::from_connection(&connection, PairingMethod::QrCode).unwrap();
        let mut start = SessionStart::new(desktop_info());
        start.pairing = Some(PairingMethod::QrCode);
        let mut session = Session::handshake(connection, start, SessionConfig::default())
            .await
            .unwrap();
        let peer = session.peer().clone();
        let mut prompt = ScriptedPrompt::answering(SasDecision::Confirmed, true);
        let timeouts = PairingTimeouts::default();
        let success = pair_by_qr(
            &mut session,
            &binding,
            &peer,
            &offer,
            &store,
            &mut prompt,
            &timeouts,
        )
        .await
        .unwrap();
        session.finish_pairing().await.unwrap();

        // The phone's FeatureStatus says it stored the pairing too; its
        // PresenceUpdate brings the key it made for this desktop.
        let fingerprint = success.device.fingerprint;
        loop {
            match session.next_event().await.unwrap() {
                Some(SessionEvent::FeatureStatus(_)) => {
                    let mut store = store.lock().unwrap();
                    assert!(records::confirm(&mut *store, &fingerprint).unwrap());
                }
                Some(SessionEvent::Presence(presence)) => {
                    let mut store = store.lock().unwrap();
                    records::set_presence_key(&mut *store, &fingerprint, &presence.presence_key)
                        .unwrap();
                    break;
                }
                Some(_) => {}
                None => panic!("the phone left"),
            }
        }
        session.close().await.unwrap();
        fingerprint
    };
    let (fingerprint, ()) = tokio::join!(desktop_side, phone_pairs(&phone, &scanned));

    let record = store.lock().unwrap().get(&fingerprint).unwrap();
    assert_eq!(record.id, "phone-1");
    assert!(record.confirmed);
    assert_eq!(record.presence_key, Some(PresenceKey(PRESENCE_KEY)));

    // Later connections find the record by the key the phone presents and
    // make the session check that its Hello carries the record's device id.
    for (expected_id, accepted) in [("phone-1", true), ("phone-2", false)] {
        let desktop_side = async {
            let connection = dialer.dial(phone.addr(), record.pin()).await.unwrap();
            let found = store
                .lock()
                .unwrap()
                .get(&connection.peer_fingerprint())
                .unwrap();
            assert_eq!(found.id, "phone-1");
            let mut start = SessionStart::new(desktop_info());
            start.expected_peer_id = Some(expected_id.to_owned());
            match Session::handshake(connection, start, SessionConfig::default()).await {
                Ok(session) => {
                    session.close().await.unwrap();
                    true
                }
                Err(SessionError::UnexpectedPeer { .. }) => false,
                Err(other) => panic!("unexpected error {other:?}"),
            }
        };
        let (connected, ()) =
            tokio::join!(desktop_side, phone_answers(&phone, identity.fingerprint()));
        assert_eq!(connected, accepted, "{expected_id}");
    }
}
