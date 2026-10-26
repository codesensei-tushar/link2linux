//! The desktop's check of the phone's certificate: the key's fingerprint
//! must be pinned, and the TLS 1.3 CertificateVerify signature must verify.
//!
//! Nothing else in the certificate matters: there is no chain, no name and
//! no date to check. The signature check is never skipped, not even while
//! pairing, when any key is accepted: it is what proves that the peer holds
//! the private key of the fingerprint it is pinned or recorded under.

use std::sync::{Mutex, PoisonError};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{WebPkiSupportedAlgorithms, verify_tls13_signature};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{CertificateError, DigitallySignedStruct, Error, PeerMisbehaved, SignatureScheme};

use super::PeerPin;
use crate::identity::CertificateFingerprint;

/// The only signature scheme either side uses.
pub(crate) const SIGNATURE_SCHEME: SignatureScheme = SignatureScheme::ECDSA_NISTP256_SHA256;

/// A [`ServerCertVerifier`] for one dial: it accepts the keys of a
/// [`PeerPin`], and remembers the fingerprint the server presented so the
/// dialer can report it.
#[derive(Debug)]
pub(crate) struct PinningVerifier {
    pin: PeerPin,
    algorithms: WebPkiSupportedAlgorithms,
    presented: Mutex<Option<CertificateFingerprint>>,
}

impl PinningVerifier {
    pub(crate) fn new(pin: PeerPin, algorithms: WebPkiSupportedAlgorithms) -> Self {
        Self {
            pin,
            algorithms,
            presented: Mutex::new(None),
        }
    }

    /// The fingerprint of the certificate the server presented, if it got
    /// that far and its key was a canonical P-256 key.
    pub(crate) fn presented(&self) -> Option<CertificateFingerprint> {
        *self
            .presented
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

impl ServerCertVerifier for PinningVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        let fingerprint = CertificateFingerprint::of_certificate(end_entity)
            .map_err(|_| Error::InvalidCertificate(CertificateError::BadEncoding))?;
        *self
            .presented
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(fingerprint);
        if self.pin.accepts(&fingerprint) {
            Ok(ServerCertVerified::assertion())
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
    ) -> Result<HandshakeSignatureValid, Error> {
        // TLS 1.2 is never negotiated; refuse in case it ever is.
        Err(Error::General("TLS 1.2 is not supported".into()))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        if dss.scheme != SIGNATURE_SCHEME {
            return Err(Error::PeerMisbehaved(
                PeerMisbehaved::SignedHandshakeWithUnadvertisedSigScheme,
            ));
        }
        verify_tls13_signature(message, cert, dss, &self.algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![SIGNATURE_SCHEME]
    }
}
