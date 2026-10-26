//! One dial attempt: racing the candidate addresses of a plan.

use std::time::Duration;

use link2linux_core::transport::{Connector, PeerPin, TransportError};
use tokio::task::JoinSet;
use tokio::time;

use crate::candidates::Plan;

/// Why an attempt got no connection.
#[derive(Debug, thiserror::Error)]
pub enum DialError {
    /// The plan was empty: nothing is known about where the phone is.
    #[error("no address to dial")]
    NoCandidates,
    /// Every candidate failed. Holds the error that says the most: a phone
    /// refusing this computer's certificate over a timeout.
    #[error(transparent)]
    Failed(TransportError),
}

impl DialError {
    /// The phone answered and refused this desktop's certificate: it
    /// removed the pairing.
    #[must_use]
    pub fn is_rejected_by_peer(&self) -> bool {
        matches!(self, Self::Failed(TransportError::RejectedByPeer(_)))
    }
}

/// How much an error tells about the phone, for picking the one to report.
fn weight(error: &TransportError) -> u8 {
    match error {
        TransportError::RejectedByPeer(_) => 3,
        TransportError::Untrusted(_) | TransportError::UnsupportedPeerKey => 2,
        TransportError::Timeout(_) | TransportError::Io(_) => 0,
        _ => 1,
    }
}

/// Connects to the candidates of `plan`, starting each at its time, and
/// returns the first connection whose TLS handshake accepted `pin`. The
/// others are dropped: attempts still running are cancelled and connections
/// that completed late are closed.
///
/// # Errors
///
/// Fails when the plan is empty or every candidate failed.
pub async fn race<C>(
    connector: &C,
    plan: Plan,
    pin: PeerPin,
) -> Result<(C::Connection, std::net::SocketAddr), DialError>
where
    C: Clone + Connector + 'static,
{
    if plan.is_empty() {
        return Err(DialError::NoCandidates);
    }
    let mut attempts = JoinSet::new();
    for (address, delay) in plan {
        let connector = connector.clone();
        let pin = pin.clone();
        attempts.spawn(async move {
            if delay > Duration::ZERO {
                time::sleep(delay).await;
            }
            connector
                .connect(address, pin)
                .await
                .map(|connection| (connection, address))
        });
    }
    let mut worst: Option<TransportError> = None;
    while let Some(joined) = attempts.join_next().await {
        match joined {
            Ok(Ok(winner)) => return Ok(winner),
            Ok(Err(error)) => {
                tracing::debug!(%error, "a dial candidate failed");
                if worst.as_ref().is_none_or(|w| weight(&error) > weight(w)) {
                    worst = Some(error);
                }
            }
            Err(join_error) => tracing::debug!(%join_error, "a dial task ended early"),
        }
    }
    Err(worst.map_or(DialError::NoCandidates, DialError::Failed))
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::net::SocketAddr;
    use std::sync::{Arc, Mutex};

    use link2linux_core::identity::CertificateFingerprint;
    use link2linux_core::transport::{Connection, DialStep};
    use tokio::io::{AsyncRead, AsyncWrite, DuplexStream, ReadBuf};

    use super::*;

    struct Fake(DuplexStream, SocketAddr);

    impl AsyncRead for Fake {
        fn poll_read(
            mut self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> std::task::Poll<io::Result<()>> {
            std::pin::Pin::new(&mut self.0).poll_read(cx, buf)
        }
    }

    impl AsyncWrite for Fake {
        fn poll_write(
            mut self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
            buf: &[u8],
        ) -> std::task::Poll<io::Result<usize>> {
            std::pin::Pin::new(&mut self.0).poll_write(cx, buf)
        }
        fn poll_flush(
            mut self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<io::Result<()>> {
            std::pin::Pin::new(&mut self.0).poll_flush(cx)
        }
        fn poll_shutdown(
            mut self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<io::Result<()>> {
            std::pin::Pin::new(&mut self.0).poll_shutdown(cx)
        }
    }

    impl Connection for Fake {
        fn peer_fingerprint(&self) -> CertificateFingerprint {
            CertificateFingerprint([0; 32])
        }
        fn local_fingerprint(&self) -> CertificateFingerprint {
            CertificateFingerprint([1; 32])
        }
        fn peer_addr(&self) -> SocketAddr {
            self.1
        }
        fn export_keying_material(
            &self,
            _: &str,
            _: &[u8],
            _: &mut [u8],
        ) -> Result<(), TransportError> {
            Ok(())
        }
    }

    /// Connects to the addresses listed as answering, after their delays;
    /// fails the others with `refusal`.
    #[derive(Clone)]
    struct Scripted {
        answering: Vec<SocketAddr>,
        refusal: fn() -> TransportError,
        started: Arc<Mutex<Vec<SocketAddr>>>,
    }

    impl Connector for Scripted {
        type Connection = Fake;

        async fn connect(&self, addr: SocketAddr, _: PeerPin) -> Result<Fake, TransportError> {
            self.started.lock().unwrap().push(addr);
            if self.answering.contains(&addr) {
                Ok(Fake(tokio::io::duplex(8).0, addr))
            } else {
                Err((self.refusal)())
            }
        }
    }

    fn scripted(answering: &[SocketAddr], refusal: fn() -> TransportError) -> Scripted {
        Scripted {
            answering: answering.to_vec(),
            refusal,
            started: Arc::default(),
        }
    }

    fn a(port: u16) -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], port))
    }

    fn timeout() -> TransportError {
        TransportError::Timeout(DialStep::Connect)
    }

    fn pin() -> PeerPin {
        PeerPin::Pairing
    }

    #[tokio::test(start_paused = true)]
    async fn the_first_connection_to_complete_wins_and_later_ones_never_start() {
        let connector = scripted(&[a(1)], timeout);
        let plan = vec![(a(1), Duration::ZERO), (a(2), Duration::from_millis(250))];
        let (connection, address) = race(&connector, plan, pin()).await.unwrap();
        assert_eq!((address, connection.peer_addr()), (a(1), a(1)));
        assert_eq!(*connector.started.lock().unwrap(), [a(1)]);
    }

    #[tokio::test(start_paused = true)]
    async fn a_later_candidate_wins_when_the_first_fails() {
        let connector = scripted(&[a(2)], timeout);
        let plan = vec![(a(1), Duration::ZERO), (a(2), Duration::from_millis(250))];
        let (_, address) = race(&connector, plan, pin()).await.unwrap();
        assert_eq!(address, a(2));
    }

    #[tokio::test]
    async fn an_empty_plan_is_reported_not_dialed() {
        let connector = scripted(&[], timeout);
        let error = race(&connector, Vec::new(), pin()).await.err().unwrap();
        assert!(matches!(error, DialError::NoCandidates));
    }

    #[tokio::test(start_paused = true)]
    async fn a_refusal_outranks_timeouts_in_the_report() {
        #[derive(Clone)]
        struct Mixed;
        impl Connector for Mixed {
            type Connection = Fake;
            async fn connect(&self, addr: SocketAddr, _: PeerPin) -> Result<Fake, TransportError> {
                Err(if addr.port() == 2 {
                    TransportError::RejectedByPeer("BadCertificate".into())
                } else {
                    timeout()
                })
            }
        }
        let plan = vec![
            (a(1), Duration::ZERO),
            (a(2), Duration::ZERO),
            (a(3), Duration::ZERO),
        ];
        let error = race(&Mixed, plan, pin()).await.err().unwrap();
        assert!(error.is_rejected_by_peer());
    }
}
