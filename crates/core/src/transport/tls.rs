//! The desktop's TLS client.

use std::collections::VecDeque;
use std::fmt;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use rustls::client::{ResolvesClientCert, Resumption};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::ServerName;
use rustls::sign::{CertifiedKey, SingleCertAndKey};
use rustls::{CertificateError, ClientConfig};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio::task::JoinSet;
use tokio::time;
use tokio_rustls::client::TlsStream;

use super::verifier::PinningVerifier;
use super::{ALPN_PROTOCOL, Connection, Connector, DialStep, PeerPin, TransportError};
use crate::identity::{CertificateFingerprint, Identity};

/// Time between starting dials to the next candidate address.
const DIAL_STAGGER: Duration = Duration::from_millis(250);

/// How long bytes written to the phone may stay unacknowledged before the
/// connection counts as gone. Without it, a write to a phone that left the
/// network, such as file chunks or the last `Goodbye`, waits for the
/// kernel's retransmissions, about 15 minutes.
const USER_TIMEOUT: Duration = Duration::from_secs(30);

/// Sets up the socket of a connection to the phone: frames go out at once,
/// and writes fail after [`USER_TIMEOUT`].
fn tune(tcp: &TcpStream) -> io::Result<()> {
    tcp.set_nodelay(true)?;
    socket2::SockRef::from(tcp).set_tcp_user_timeout(Some(USER_TIMEOUT))
}

/// How long each step of a dial may take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DialTimeouts {
    /// Opening the TCP connection: 5 seconds by default.
    pub connect: Duration,
    /// The TLS handshake: 10 seconds by default.
    pub handshake: Duration,
}

impl Default for DialTimeouts {
    fn default() -> Self {
        Self {
            connect: Duration::from_secs(5),
            handshake: Duration::from_secs(10),
        }
    }
}

/// Dials phones over TLS 1.3 with this desktop's identity.
///
/// Each dial builds its own client configuration around a verifier for that
/// dial's [`PeerPin`]: TLS 1.3 only, no resumption, no early data, no
/// server name, ALPN `l2l/1`, and this desktop's certificate as the client
/// certificate. Copies share the key and the crypto provider.
#[derive(Clone)]
pub struct TlsDialer {
    provider: Arc<CryptoProvider>,
    client_cert: Arc<dyn ResolvesClientCert>,
    fingerprint: CertificateFingerprint,
    timeouts: DialTimeouts,
}

impl TlsDialer {
    /// A dialer that presents `identity`.
    ///
    /// # Errors
    ///
    /// Fails if rustls cannot load the identity's key.
    pub fn new(identity: &Identity) -> Result<Self, TransportError> {
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let certified = CertifiedKey::from_der(
            vec![identity.certificate().clone()],
            identity.private_key(),
            &provider,
        )?;
        Ok(Self {
            provider,
            client_cert: Arc::new(SingleCertAndKey::from(certified)),
            fingerprint: identity.fingerprint(),
            timeouts: DialTimeouts::default(),
        })
    }

    /// Changes the timeouts.
    #[must_use]
    pub fn with_timeouts(mut self, timeouts: DialTimeouts) -> Self {
        self.timeouts = timeouts;
        self
    }

    /// Connects to `addr` and runs the TLS handshake, accepting the phone's
    /// key only if `pin` does.
    ///
    /// The dialer trusts the caller with the address: one taken from an
    /// mDNS record or a beacon must first pass
    /// [`is_on_link`](crate::discovery::is_on_link); one the user typed is
    /// dialed as given.
    ///
    /// # Errors
    ///
    /// Fails with [`TransportError::Timeout`] when the TCP connection or the
    /// handshake takes too long, [`TransportError::Untrusted`] when the
    /// phone's key is not pinned, [`TransportError::NoApplicationProtocol`]
    /// when the phone did not select `l2l/1`, and with other variants for
    /// other TLS and socket errors.
    pub async fn dial(
        &self,
        addr: SocketAddr,
        pin: PeerPin,
    ) -> Result<TlsConnection, TransportError> {
        let tcp = time::timeout(self.timeouts.connect, TcpStream::connect(addr))
            .await
            .map_err(|_| TransportError::Timeout(DialStep::Connect))??;
        self.handshake(tcp, pin).await
    }

    /// Runs the TLS handshake on an open TCP connection.
    ///
    /// # Errors
    ///
    /// As for [`TlsDialer::dial`].
    pub async fn handshake(
        &self,
        tcp: TcpStream,
        pin: PeerPin,
    ) -> Result<TlsConnection, TransportError> {
        tune(&tcp)?;
        let peer_addr = tcp.peer_addr()?;
        let verifier = Arc::new(PinningVerifier::new(
            pin,
            self.provider.signature_verification_algorithms,
        ));
        let connector =
            tokio_rustls::TlsConnector::from(Arc::new(self.client_config(verifier.clone())?));
        // An IP address as the server name: rustls then sends no SNI.
        let server_name = ServerName::IpAddress(peer_addr.ip().into());
        let stream =
            match time::timeout(self.timeouts.handshake, connector.connect(server_name, tcp)).await
            {
                Err(_) => return Err(TransportError::Timeout(DialStep::Handshake)),
                Ok(Err(error)) => return Err(handshake_error(error, &verifier)),
                Ok(Ok(stream)) => stream,
            };

        let (_, connection) = stream.get_ref();
        if connection.alpn_protocol() != Some(ALPN_PROTOCOL) {
            return Err(TransportError::NoApplicationProtocol);
        }
        let peer = verifier
            .presented()
            .ok_or_else(|| TransportError::Tls("the phone presented no certificate".into()))?;
        Ok(TlsConnection {
            stream,
            local: self.fingerprint,
            peer,
            peer_addr,
        })
    }

    /// Races dials to several addresses of the same phone, starting one
    /// every 250 ms, and returns the first connection that completes TLS
    /// with an accepted key. The other dials are cancelled.
    ///
    /// For QR code pairing, where every connection that completes TLS runs
    /// the proof exchange, dial the addresses one by one instead.
    ///
    /// # Errors
    ///
    /// Fails when every dial failed, with the most telling of their errors,
    /// or when `addrs` is empty.
    pub async fn dial_any(
        &self,
        addrs: &[SocketAddr],
        pin: &PeerPin,
    ) -> Result<TlsConnection, TransportError> {
        let mut queue: VecDeque<SocketAddr> = addrs.iter().copied().collect();
        let mut attempts = JoinSet::new();
        let mut stagger = time::interval(DIAL_STAGGER);
        let mut error: Option<TransportError> = None;
        loop {
            tokio::select! {
                _ = stagger.tick(), if !queue.is_empty() => {
                    if let Some(addr) = queue.pop_front() {
                        let dialer = self.clone();
                        let pin = pin.clone();
                        attempts.spawn(async move { dialer.dial(addr, pin).await });
                    }
                }
                Some(joined) = attempts.join_next(), if !attempts.is_empty() => match joined {
                    Ok(Ok(connection)) => return Ok(connection),
                    Ok(Err(failure)) => error = Some(more_telling(error, failure)),
                    Err(join_error) => {
                        let failure = TransportError::Io(io::Error::other(join_error));
                        error = Some(more_telling(error, failure));
                    }
                },
                else => {
                    return Err(error.unwrap_or_else(|| {
                        TransportError::Io(io::Error::new(
                            io::ErrorKind::InvalidInput,
                            "no address to dial",
                        ))
                    }));
                }
            }
        }
    }

    fn client_config(
        &self,
        verifier: Arc<PinningVerifier>,
    ) -> Result<ClientConfig, TransportError> {
        let mut config = ClientConfig::builder_with_provider(self.provider.clone())
            .with_protocol_versions(&[&rustls::version::TLS13])?
            .dangerous()
            .with_custom_certificate_verifier(verifier)
            .with_client_cert_resolver(self.client_cert.clone());
        config.alpn_protocols = vec![ALPN_PROTOCOL.to_vec()];
        // A resumed handshake carries no certificates, so neither key would
        // be checked again.
        config.resumption = Resumption::disabled();
        config.enable_early_data = false;
        config.enable_sni = false;
        Ok(config)
    }
}

impl fmt::Debug for TlsDialer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TlsDialer")
            .field("fingerprint", &self.fingerprint)
            .field("timeouts", &self.timeouts)
            .finish_non_exhaustive()
    }
}

impl Connector for TlsDialer {
    type Connection = TlsConnection;

    fn connect(
        &self,
        addr: SocketAddr,
        pin: PeerPin,
    ) -> impl Future<Output = Result<TlsConnection, TransportError>> + Send {
        self.dial(addr, pin)
    }
}

/// Maps a failed handshake to an error, naming the key when the pin
/// refused it.
///
/// A certificate alert here is never [`TransportError::RejectedByPeer`]:
/// nothing the handshake received so far is authenticated, so anyone who
/// answers at the address can send one, in plaintext. The phone's real
/// refusal arrives after the handshake.
fn handshake_error(error: io::Error, verifier: &PinningVerifier) -> TransportError {
    let pin_refused = error
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<rustls::Error>())
        .is_some_and(|tls| {
            matches!(
                tls,
                rustls::Error::InvalidCertificate(CertificateError::ApplicationVerificationFailure)
            )
        });
    match verifier.presented() {
        Some(fingerprint) if pin_refused => TransportError::Untrusted(fingerprint),
        _ => match TransportError::from(error) {
            TransportError::RejectedByPeer(alert) => {
                TransportError::Tls(format!("received a {alert} alert during the handshake"))
            }
            other => other,
        },
    }
}

/// Keeps the error that says more about why a phone could not be reached:
/// key and protocol problems over timeouts and socket errors.
fn more_telling(kept: Option<TransportError>, new: TransportError) -> TransportError {
    let weight = |error: &TransportError| match error {
        TransportError::Io(_) => 0,
        TransportError::Timeout(_) => 1,
        _ => 2,
    };
    match kept {
        Some(kept) if weight(&kept) >= weight(&new) => kept,
        _ => new,
    }
}

/// A TLS connection to a phone, with the fingerprints of both sides.
pub struct TlsConnection {
    stream: TlsStream<TcpStream>,
    local: CertificateFingerprint,
    peer: CertificateFingerprint,
    peer_addr: SocketAddr,
}

impl Connection for TlsConnection {
    fn peer_fingerprint(&self) -> CertificateFingerprint {
        self.peer
    }

    fn local_fingerprint(&self) -> CertificateFingerprint {
        self.local
    }

    fn peer_addr(&self) -> SocketAddr {
        self.peer_addr
    }

    fn export_keying_material(
        &self,
        label: &str,
        context: &[u8],
        output: &mut [u8],
    ) -> Result<(), TransportError> {
        let (_, connection) = self.stream.get_ref();
        connection
            .export_keying_material(output, label.as_bytes(), Some(context))
            .map(|_| ())
            .map_err(|error| TransportError::Exporter(error.to_string()))
    }
}

impl fmt::Debug for TlsConnection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TlsConnection")
            .field("peer", &self.peer)
            .field("peer_addr", &self.peer_addr)
            .finish_non_exhaustive()
    }
}

impl AsyncRead for TlsConnection {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_read(cx, buf)
    }
}

impl AsyncWrite for TlsConnection {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().stream).poll_write(cx, buf)
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().stream).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.stream.is_write_vectored()
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn connections_to_the_phone_give_up_on_unacknowledged_writes() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let tcp = TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        tune(&tcp).unwrap();
        assert!(tcp.nodelay().unwrap());
        assert_eq!(
            socket2::SockRef::from(&tcp).tcp_user_timeout().unwrap(),
            Some(USER_TIMEOUT)
        );
    }
}
