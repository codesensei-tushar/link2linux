//! A stand-in phone for loopback tests: a rustls TLS server with the
//! phone's certificate rules, the envelope stream on top of it, and the
//! phone's side of both pairing methods.

#![allow(dead_code, reason = "each test file uses part of the support code")]

use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};

use futures_util::{SinkExt, StreamExt};
use link2linux_core::device::{DeviceInfo, DeviceKind};
use link2linux_core::identity::{CertificateFingerprint, Identity};
use link2linux_core::pairing::crypto::{self, Role};
use link2linux_core::pairing::{
    PairedDevice, PairingChannel, PairingCode, PairingError, PairingPrompt, PairingSecret,
    SasDecision,
};
use link2linux_core::transport::{ALPN_PROTOCOL, PeerPin, TransportError};
use link2linux_protocol::v1::{
    PairingMethod, PairingOutcome, PairingProof, PairingResult, SasCommit, SasNonce,
};
use link2linux_protocol::{Envelope, EnvelopeCodec, FrameError, Hello, PROTOCOL_VERSION, Payload};
use rustls::crypto::{WebPkiSupportedAlgorithms, verify_tls13_signature};
use rustls::pki_types::{CertificateDer, UnixTime};
use rustls::server::NoServerSessionStorage;
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{
    CertificateError, DigitallySignedStruct, DistinguishedName, Error, ServerConfig,
    SignatureScheme,
};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::server::TlsStream;
use tokio_util::codec::Framed;

/// Envelopes on a stream: numbers what it sends, checks what it receives.
pub struct Wire<S> {
    framed: Framed<S, EnvelopeCodec>,
    next_seq: u64,
    last_peer_seq: u64,
}

impl<S: AsyncRead + AsyncWrite + Unpin + Send> Wire<S> {
    pub fn new(stream: S) -> Self {
        Self {
            framed: Framed::new(stream, EnvelopeCodec::default()),
            next_seq: 1,
            last_peer_seq: 0,
        }
    }

    pub async fn send_payload(&mut self, payload: Payload) -> Result<(), FrameError> {
        let envelope = Envelope {
            seq: self.next_seq,
            payload: Some(payload),
            ..Envelope::default()
        };
        self.next_seq += 1;
        self.framed.send(&envelope).await
    }

    pub async fn recv_payload(&mut self) -> Result<Option<Payload>, FrameError> {
        let Some(envelope) = self.framed.next().await.transpose()? else {
            return Ok(None);
        };
        assert!(
            envelope.seq > self.last_peer_seq,
            "sequence numbers must grow"
        );
        self.last_peer_seq = envelope.seq;
        Ok(Some(envelope.payload.expect("a known payload")))
    }

    /// Sends this side's `Hello` and returns the peer's.
    pub async fn hello(&mut self, hello: Hello) -> Result<Hello, FrameError> {
        self.send_payload(Payload::Hello(hello)).await?;
        match self.recv_payload().await? {
            Some(Payload::Hello(hello)) => Ok(hello),
            other => panic!("expected Hello, got {other:?}"),
        }
    }

    /// Ends the connection gracefully: closes this side, then reads until
    /// the peer closes too, so no unread data turns the close into a reset
    /// that could destroy the peer's last messages.
    pub async fn finish(mut self) {
        let _ = self.framed.close().await;
        let drain = async { while let Some(Ok(_)) = self.framed.next().await {} };
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), drain).await;
    }
}

impl<S> Wire<S> {
    /// The stream underneath, for a test that runs a session on it.
    pub fn into_inner(self) -> S {
        self.framed.into_inner()
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin + Send> PairingChannel for Wire<S> {
    async fn send(&mut self, payload: Payload) -> Result<(), PairingError> {
        self.send_payload(payload)
            .await
            .map_err(|error| PairingError::Connection(error.to_string()))
    }

    async fn recv(&mut self) -> Result<Payload, PairingError> {
        match self.recv_payload().await {
            Ok(Some(payload)) => Ok(payload),
            Ok(None) => Err(PairingError::Closed),
            Err(error) => Err(PairingError::Connection(error.to_string())),
        }
    }
}

/// The transport error inside a frame error, as the session layer would
/// classify it.
pub fn transport_error(error: FrameError) -> TransportError {
    match error {
        FrameError::Io(io) => TransportError::from(io),
        other => TransportError::Io(io::Error::other(other.to_string())),
    }
}

pub fn hello(id: &str, name: &str, kind: DeviceKind, method: PairingMethod) -> Hello {
    Hello {
        protocol_version: PROTOCOL_VERSION,
        device_id: id.into(),
        device_name: name.into(),
        kind: kind.into(),
        pairing_method: method.into(),
        ..Hello::default()
    }
}

pub fn device_info(hello: &Hello) -> DeviceInfo {
    DeviceInfo::new(
        hello.device_id.clone(),
        hello.device_name.clone(),
        hello.kind(),
        [],
    )
}

/// The phone's client certificate check, as on Android: the key must be
/// pinned (or anything goes in SAS mode), and the CertificateVerify
/// signature is always verified.
#[derive(Debug)]
struct PhoneVerifier {
    pin: PeerPin,
    algorithms: WebPkiSupportedAlgorithms,
}

impl ClientCertVerifier for PhoneVerifier {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> Result<ClientCertVerified, Error> {
        let fingerprint = CertificateFingerprint::of_certificate(end_entity)
            .map_err(|_| Error::InvalidCertificate(CertificateError::BadEncoding))?;
        if self.pin.accepts(&fingerprint) {
            Ok(ClientCertVerified::assertion())
        } else {
            Err(Error::InvalidCertificate(
                CertificateError::ApplicationVerificationFailure,
            ))
        }
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, Error> {
        Err(Error::General("TLS 1.2 is not supported".into()))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, Error> {
        verify_tls13_signature(message, cert, dss, &self.algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![SignatureScheme::ECDSA_NISTP256_SHA256]
    }
}

/// How the test phone's TLS server is set up.
#[derive(Debug, Clone)]
pub struct PhoneTls {
    /// Which desktop keys it accepts.
    pub pin: PeerPin,
    /// Whether it selects `l2l/1`; a phone that does not tests the
    /// desktop's ALPN check.
    pub alpn: bool,
}

impl PhoneTls {
    pub fn pinned(desktop: CertificateFingerprint) -> Self {
        Self {
            pin: PeerPin::from(desktop),
            alpn: true,
        }
    }

    pub fn sas_mode() -> Self {
        Self {
            pin: PeerPin::Pairing,
            alpn: true,
        }
    }
}

/// A listening test phone.
pub struct TestPhone {
    pub identity: Identity,
    pub name: String,
    pub id: String,
    listener: TcpListener,
}

/// A phone-side TLS connection after the handshake.
pub struct PhoneConnection {
    pub wire: Wire<TlsStream<TcpStream>>,
    pub desktop: CertificateFingerprint,
    pub phone: CertificateFingerprint,
    ekm_pair: [u8; 32],
    ekm_sas: [u8; 32],
}

impl TestPhone {
    /// A new phone with a fresh key, listening on `ip` at a free port.
    pub async fn bind(ip: IpAddr, id: &str) -> Self {
        Self::bind_with(ip, id, Identity::generate().unwrap()).await
    }

    pub async fn bind_with(ip: IpAddr, id: &str, identity: Identity) -> Self {
        Self {
            identity,
            name: format!("Phone {id}"),
            id: id.into(),
            listener: TcpListener::bind((ip, 0)).await.unwrap(),
        }
    }

    pub fn addr(&self) -> SocketAddr {
        self.listener.local_addr().unwrap()
    }

    pub fn fingerprint(&self) -> CertificateFingerprint {
        self.identity.fingerprint()
    }

    /// Accepts one connection and runs the TLS handshake.
    pub async fn accept(&self, tls: PhoneTls) -> io::Result<PhoneConnection> {
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let verifier = PhoneVerifier {
            pin: tls.pin,
            algorithms: provider.signature_verification_algorithms,
        };
        let mut config = ServerConfig::builder_with_provider(provider.clone())
            .with_protocol_versions(&[&rustls::version::TLS13])
            .unwrap()
            .with_client_cert_verifier(Arc::new(verifier))
            .with_single_cert(
                vec![self.identity.certificate().clone()],
                self.identity.private_key(),
            )
            .unwrap();
        if tls.alpn {
            config.alpn_protocols = vec![ALPN_PROTOCOL.to_vec()];
        }
        config.send_tls13_tickets = 0;
        config.session_storage = Arc::new(NoServerSessionStorage {});

        let (tcp, _) = self.listener.accept().await?;
        let stream = tokio_rustls::TlsAcceptor::from(Arc::new(config))
            .accept(tcp)
            .await?;
        let (_, connection) = stream.get_ref();
        if tls.alpn {
            assert_eq!(connection.alpn_protocol(), Some(ALPN_PROTOCOL));
        }
        let desktop =
            CertificateFingerprint::of_certificate(&connection.peer_certificates().unwrap()[0])
                .unwrap();
        let phone = self.fingerprint();
        let context = crypto::exporter_context(&desktop, &phone);
        let export = |label: &str| {
            connection
                .export_keying_material([0u8; 32], label.as_bytes(), Some(&context))
                .unwrap()
        };
        let ekm_pair = export(crypto::PAIR_EXPORTER_LABEL);
        let ekm_sas = export(crypto::SAS_EXPORTER_LABEL);
        Ok(PhoneConnection {
            wire: Wire::new(stream),
            desktop,
            phone,
            ekm_pair,
            ekm_sas,
        })
    }

    pub fn hello(&self) -> Hello {
        hello(
            &self.id,
            &self.name,
            DeviceKind::Phone,
            PairingMethod::Unspecified,
        )
    }
}

impl PhoneConnection {
    /// The exporter value for QR code pairing on this connection.
    pub fn ekm_pair(&self) -> [u8; 32] {
        self.ekm_pair
    }
}

/// What the test phone saw of a pairing.
#[derive(Debug)]
pub struct PhoneReport {
    /// The desktop's `Hello`.
    pub desktop_hello: Hello,
    /// The desktop's verdict, if one arrived.
    pub desktop_outcome: Option<PairingOutcome>,
    /// The exporter value the phone derived.
    pub ekm: [u8; 32],
    /// The SAS code the phone showed.
    pub code: Option<PairingCode>,
}

impl PhoneConnection {
    /// The phone's side of QR pairing, with the secret it scanned.
    pub async fn pair_by_qr(&mut self, phone: &TestPhone, secret: &PairingSecret) -> PhoneReport {
        let desktop_hello = self.wire.hello(phone.hello()).await.unwrap();
        assert_eq!(desktop_hello.pairing_method(), PairingMethod::QrCode);
        let ekm = self.ekm_pair;

        // The phone's proof goes out right after Hello.
        let mac = crypto::pairing_proof(secret, Role::Phone, &ekm);
        self.wire
            .send_payload(Payload::PairingProof(PairingProof { mac: mac.to_vec() }))
            .await
            .unwrap();

        let mut desktop_outcome = None;
        match self.wire.recv_payload().await.unwrap() {
            Some(Payload::PairingProof(proof)) => {
                let outcome =
                    if crypto::verify_pairing_proof(secret, Role::Desktop, &ekm, &proof.mac) {
                        PairingOutcome::Accepted
                    } else {
                        PairingOutcome::VerificationFailed
                    };
                self.send_result(outcome).await;
                if outcome == PairingOutcome::Accepted {
                    desktop_outcome = self.read_result().await;
                }
            }
            Some(Payload::PairingResult(result)) => desktop_outcome = Some(result.outcome()),
            other => panic!("unexpected {other:?}"),
        }
        PhoneReport {
            desktop_hello,
            desktop_outcome,
            ekm,
            code: None,
        }
    }

    /// The phone's side of SAS pairing. With `tamper`, it reveals another
    /// nonce than the one it committed to. `decision` is its user's answer.
    pub async fn pair_by_sas(
        &mut self,
        phone: &TestPhone,
        tamper: bool,
        decision: SasDecision,
    ) -> PhoneReport {
        let desktop_hello = self.wire.hello(phone.hello()).await.unwrap();
        assert_eq!(desktop_hello.pairing_method(), PairingMethod::Sas);
        let ekm = self.ekm_sas;

        let phone_nonce = [0x5a; 32];
        let commitment = crypto::sas_commitment(&ekm, &phone_nonce);
        self.wire
            .send_payload(Payload::SasCommit(SasCommit {
                commitment: commitment.to_vec(),
            }))
            .await
            .unwrap();
        let desktop_nonce: [u8; 32] = match self.wire.recv_payload().await.unwrap() {
            Some(Payload::SasNonce(nonce)) => nonce.nonce.try_into().unwrap(),
            other => panic!("expected SasNonce, got {other:?}"),
        };
        let revealed = if tamper { [0xa5; 32] } else { phone_nonce };
        self.wire
            .send_payload(Payload::SasNonce(SasNonce {
                nonce: revealed.to_vec(),
            }))
            .await
            .unwrap();

        let code = crypto::sas_code(&ekm, &desktop_nonce, &phone_nonce);
        let desktop_outcome = if tamper {
            self.read_result().await
        } else {
            let outcome = match decision {
                SasDecision::Confirmed => PairingOutcome::Accepted,
                SasDecision::Declined => PairingOutcome::Declined,
            };
            self.send_result(outcome).await;
            self.read_result().await
        };
        PhoneReport {
            desktop_hello,
            desktop_outcome,
            ekm,
            code: Some(code),
        }
    }

    async fn send_result(&mut self, outcome: PairingOutcome) {
        let result = PairingResult {
            outcome: outcome.into(),
        };
        self.wire
            .send_payload(Payload::PairingResult(result))
            .await
            .unwrap();
    }

    /// The desktop's next `PairingResult`, skipping its proof; `None` if
    /// the connection ends first.
    async fn read_result(&mut self) -> Option<PairingOutcome> {
        loop {
            match self.wire.recv_payload().await {
                Ok(Some(Payload::PairingResult(result))) => return Some(result.outcome()),
                Ok(Some(Payload::PairingProof(_))) => {}
                Ok(Some(other)) => panic!("unexpected {other:?}"),
                Ok(None) | Err(_) => return None,
            }
        }
    }
}

/// What a [`ScriptedPrompt`] was shown: each code (empty when it was asked
/// about replacing records) with the records the pairing would replace.
pub type Shown = Arc<Mutex<Vec<(PairingCode, Vec<PairedDevice>)>>>;

/// A [`PairingPrompt`] with fixed answers that records what it was shown.
#[derive(Debug, Clone)]
pub struct ScriptedPrompt {
    pub decision: SasDecision,
    pub replace: bool,
    pub shown: Shown,
}

impl ScriptedPrompt {
    pub fn answering(decision: SasDecision, replace: bool) -> Self {
        Self {
            decision,
            replace,
            shown: Arc::default(),
        }
    }

    pub fn codes(&self) -> Vec<PairingCode> {
        self.shown
            .lock()
            .unwrap()
            .iter()
            .map(|(code, _)| code.clone())
            .collect()
    }
}

impl PairingPrompt for ScriptedPrompt {
    async fn confirm_code(
        &mut self,
        code: &PairingCode,
        _phone: &DeviceInfo,
        replaces: &[PairedDevice],
    ) -> SasDecision {
        self.shown
            .lock()
            .unwrap()
            .push((code.clone(), replaces.to_vec()));
        self.decision
    }

    async fn confirm_replace(&mut self, _phone: &DeviceInfo, replaces: &[PairedDevice]) -> bool {
        self.shown
            .lock()
            .unwrap()
            .push((PairingCode::new(""), replaces.to_vec()));
        self.replace
    }
}
