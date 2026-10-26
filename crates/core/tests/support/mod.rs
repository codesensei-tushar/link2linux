//! A stand-in phone for loopback tests: a rustls TLS server with the
//! phone's certificate rules, and the envelope stream on top of it.

#![allow(dead_code, reason = "each test file uses part of the support code")]

use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use link2linux_core::device::{DeviceInfo, DeviceKind};
use link2linux_core::identity::{CertificateFingerprint, Identity};
use link2linux_core::pairing::crypto;
use link2linux_core::pairing::{PairingChannel, PairingError};
use link2linux_core::transport::{ALPN_PROTOCOL, PeerPin, TransportError};
use link2linux_protocol::v1::PairingMethod;
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
