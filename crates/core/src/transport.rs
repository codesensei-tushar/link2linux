//! The secure byte stream a session runs on.
//!
//! Devices talk over TLS 1.3 on the local network, with certificates on
//! both sides. Each device presents its own self-signed certificate and
//! accepts a peer only by the fingerprint of its key: one stored at pairing,
//! or, while pairing, any key, which is then recorded. The TLS
//! CertificateVerify signature is verified in every case. A [`Connection`]
//! is the stream a session then runs on.
//!
//! The desktop always dials the phone and is the TLS client, through a
//! [`Connector`]: [`TlsDialer`] is the real one. It offers only TLS 1.3,
//! never resumes a session or sends early data, sends no server name (it
//! dials an IP address), and fails unless the phone selected the ALPN
//! protocol [`ALPN_PROTOCOL`]. A [`Listener`] is the phone's side, for tests
//! and tools that stand in for a phone.
//!
//! In TLS 1.3 the client finishes its handshake before the server has
//! checked the client's certificate. When the phone refuses this desktop's
//! key, the dial therefore succeeds and the refusal arrives as an error on
//! the first read, normally while reading the phone's `Hello`. Pass such
//! errors through [`TransportError::from`] to tell a refusal
//! ([`TransportError::RejectedByPeer`]) from other failures. Only such a
//! refusal counts: it is encrypted with keys of a handshake that checked
//! the phone's key. An alert during the handshake is unauthenticated, so a
//! dial reports it as an ordinary TLS failure.

mod tls;
mod verifier;

use std::io;
use std::net::SocketAddr;

use rustls::AlertDescription;
use tokio::io::{AsyncRead, AsyncWrite};

pub use self::tls::{DialTimeouts, TlsConnection, TlsDialer};
use crate::identity::CertificateFingerprint;

/// The ALPN protocol ID both sides offer and require.
pub const ALPN_PROTOCOL: &[u8] = b"l2l/1";

/// An authenticated, encrypted stream to a peer.
pub trait Connection: AsyncRead + AsyncWrite + Unpin + Send + 'static {
    /// Fingerprint of the certificate the peer presented.
    fn peer_fingerprint(&self) -> CertificateFingerprint;

    /// Fingerprint of the certificate this side presented.
    fn local_fingerprint(&self) -> CertificateFingerprint;

    /// The peer's network address.
    fn peer_addr(&self) -> SocketAddr;

    /// Fills `output` from the TLS keying material exporter (RFC 8446,
    /// section 7.5) with `label` and `context`.
    ///
    /// # Errors
    ///
    /// Fails if the TLS stack cannot export keying material.
    fn export_keying_material(
        &self,
        label: &str,
        context: &[u8],
        output: &mut [u8],
    ) -> Result<(), TransportError>;
}

/// Which keys a dial accepts from the peer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerPin {
    /// A paired peer: its current key, and the next one if it announced a
    /// key rotation.
    Paired {
        /// The key in its pairing record.
        current: CertificateFingerprint,
        /// The key it announced with `KeyRotation`, if any.
        next: Option<CertificateFingerprint>,
    },
    /// A dial to pair: any P-256 key is accepted and recorded in the
    /// connection.
    Pairing,
}

impl PeerPin {
    /// Whether a peer presenting `fingerprint` is accepted.
    #[must_use]
    pub fn accepts(&self, fingerprint: &CertificateFingerprint) -> bool {
        match self {
            Self::Paired { current, next } => {
                current == fingerprint || next.as_ref() == Some(fingerprint)
            }
            Self::Pairing => true,
        }
    }
}

impl From<CertificateFingerprint> for PeerPin {
    fn from(current: CertificateFingerprint) -> Self {
        Self::Paired {
            current,
            next: None,
        }
    }
}

/// The step of a dial that ran out of time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialStep {
    /// Opening the TCP connection.
    Connect,
    /// The TLS handshake.
    Handshake,
}

/// Why a connection could not be set up, or failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TransportError {
    /// The peer's key is not one this dial accepts.
    #[error("untrusted peer certificate {0}")]
    Untrusted(CertificateFingerprint),
    /// The peer's certificate does not hold a P-256 key in the canonical
    /// encoding.
    #[error("the peer's certificate has no usable P-256 key")]
    UnsupportedPeerKey,
    /// The peer refused this side's certificate: for the desktop, the phone
    /// does not (or no longer) trust its key. Only a read or write after the
    /// handshake reports it, never a dial.
    #[error("the peer refused this device's certificate ({0})")]
    RejectedByPeer(String),
    /// The peer did not select the `l2l/1` protocol.
    #[error("the peer does not speak the link2linux protocol (ALPN)")]
    NoApplicationProtocol,
    /// A step of the dial took too long.
    #[error("{0:?} timed out")]
    Timeout(DialStep),
    /// The keying material exporter failed.
    #[error("TLS exporter failed: {0}")]
    Exporter(String),
    /// Any other TLS failure.
    #[error("TLS failed: {0}")]
    Tls(String),
    /// A socket error.
    #[error(transparent)]
    Io(io::Error),
}

impl From<rustls::Error> for TransportError {
    fn from(error: rustls::Error) -> Self {
        use rustls::{CertificateError, Error};
        match error {
            Error::AlertReceived(alert) if is_certificate_alert(alert) => {
                Self::RejectedByPeer(format!("{alert:?}"))
            }
            Error::AlertReceived(AlertDescription::NoApplicationProtocol)
            | Error::NoApplicationProtocol => Self::NoApplicationProtocol,
            Error::InvalidCertificate(CertificateError::BadEncoding) => Self::UnsupportedPeerKey,
            other => Self::Tls(other.to_string()),
        }
    }
}

impl From<io::Error> for TransportError {
    /// Unwraps a TLS error carried in an I/O error, as reads and writes on
    /// a [`TlsConnection`] report them.
    fn from(error: io::Error) -> Self {
        let is_tls = error
            .get_ref()
            .is_some_and(|inner| inner.downcast_ref::<rustls::Error>().is_some());
        if !is_tls {
            return Self::Io(error);
        }
        let inner = error.into_inner().expect("checked above");
        match inner.downcast::<rustls::Error>() {
            Ok(tls) => Self::from(*tls),
            Err(other) => Self::Io(io::Error::new(io::ErrorKind::InvalidData, other)),
        }
    }
}

impl TransportError {
    /// Whether `error`, from a read or write on a [`TlsConnection`], is the
    /// peer refusing this side's certificate, as [`TransportError::from`]
    /// would classify it.
    #[must_use]
    pub fn is_refusal(error: &io::Error) -> bool {
        error
            .get_ref()
            .and_then(|inner| inner.downcast_ref::<rustls::Error>())
            .is_some_and(|tls| {
                matches!(tls, rustls::Error::AlertReceived(alert) if is_certificate_alert(*alert))
            })
    }
}

/// Alerts a TLS stack sends when it refuses the peer's certificate.
fn is_certificate_alert(alert: AlertDescription) -> bool {
    matches!(
        alert,
        AlertDescription::BadCertificate
            | AlertDescription::UnsupportedCertificate
            | AlertDescription::CertificateRevoked
            | AlertDescription::CertificateExpired
            | AlertDescription::CertificateUnknown
            | AlertDescription::CertificateRequired
            | AlertDescription::UnknownCA
            | AlertDescription::AccessDenied
    )
}

/// Opens connections to peers.
pub trait Connector: Send + Sync {
    /// The connection type this connector produces.
    type Connection: Connection;

    /// Connects to `addr` and accepts the peer only if its key is one that
    /// `pin` accepts.
    fn connect(
        &self,
        addr: SocketAddr,
        pin: PeerPin,
    ) -> impl Future<Output = Result<Self::Connection, TransportError>> + Send;
}

/// Accepts connections from peers.
pub trait Listener: Send {
    /// The connection type this listener produces.
    type Connection: Connection;

    /// The address this listener is bound to, for announcing it.
    ///
    /// # Errors
    ///
    /// Fails if the socket cannot report its address.
    fn local_addr(&self) -> Result<SocketAddr, TransportError>;

    /// Waits for the next peer whose certificate `is_trusted` accepts.
    fn accept<F>(
        &mut self,
        is_trusted: F,
    ) -> impl Future<Output = Result<Self::Connection, TransportError>> + Send
    where
        F: Fn(&CertificateFingerprint) -> bool + Send;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pins_accept_current_and_next_keys() {
        let current = CertificateFingerprint([1; 32]);
        let next = CertificateFingerprint([2; 32]);
        let other = CertificateFingerprint([3; 32]);

        let pin = PeerPin::from(current);
        assert!(pin.accepts(&current) && !pin.accepts(&next));
        let rotating = PeerPin::Paired {
            current,
            next: Some(next),
        };
        assert!(rotating.accepts(&current) && rotating.accepts(&next));
        assert!(!rotating.accepts(&other));
        assert!(PeerPin::Pairing.accepts(&other));
    }

    #[test]
    fn tls_errors_inside_io_errors_are_classified() {
        let refused = io::Error::new(
            io::ErrorKind::InvalidData,
            rustls::Error::AlertReceived(AlertDescription::BadCertificate),
        );
        assert!(TransportError::is_refusal(&refused));
        assert!(matches!(
            TransportError::from(refused),
            TransportError::RejectedByPeer(_)
        ));
        let alpn = io::Error::new(
            io::ErrorKind::InvalidData,
            rustls::Error::NoApplicationProtocol,
        );
        assert!(!TransportError::is_refusal(&alpn));
        assert!(matches!(
            TransportError::from(alpn),
            TransportError::NoApplicationProtocol
        ));
        let plain = io::Error::from(io::ErrorKind::ConnectionReset);
        assert!(!TransportError::is_refusal(&plain));
        assert!(matches!(TransportError::from(plain), TransportError::Io(_)));
    }
}
