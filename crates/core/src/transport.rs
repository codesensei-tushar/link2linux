//! The secure byte stream a session runs on.
//!
//! Devices talk over TLS 1.3 on the local network. Each device presents its
//! own self-signed certificate and accepts a peer only if the peer's key
//! matches a fingerprint stored at pairing (or, while pairing, the one being
//! confirmed). A [`Connection`] is the stream a session then runs on.
//!
//! The desktop always dials the phone, through a [`Connector`]. A
//! [`Listener`] is the phone's side, for tests and tools that stand in for
//! a phone.
//!
//! Stub: the [`Connector`] and [`Listener`] traits are in place; the TLS
//! implementation comes next.

use std::net::SocketAddr;

use tokio::io::{AsyncRead, AsyncWrite};

use crate::pairing::CertificateFingerprint;

/// An authenticated, encrypted stream to a peer.
pub trait Connection: AsyncRead + AsyncWrite + Unpin + Send + 'static {
    /// Fingerprint of the certificate the peer presented.
    fn peer_fingerprint(&self) -> CertificateFingerprint;

    /// The peer's network address.
    fn peer_addr(&self) -> SocketAddr;
}

/// Why a connection could not be set up.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TransportError {
    /// The peer's certificate is not the one stored for it.
    #[error("untrusted peer certificate {0}")]
    Untrusted(CertificateFingerprint),
    /// The TLS handshake failed.
    #[error("TLS handshake failed: {0}")]
    Tls(String),
    /// A socket error.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Opens connections to peers.
pub trait Connector: Send + Sync {
    /// The connection type this connector produces.
    type Connection: Connection;

    /// Connects to `addr` and accepts the peer only if its certificate has
    /// the `expected` fingerprint.
    fn connect(
        &self,
        addr: SocketAddr,
        expected: CertificateFingerprint,
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
