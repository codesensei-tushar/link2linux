//! A session between two paired devices, on top of an established stream.
//!
//! The stream is normally the TLS connection set up by the transport layer,
//! but any `AsyncRead + AsyncWrite` works, which is how the tests run
//! sessions over in-memory pipes.
//!
//! A session starts with the `Hello` exchange: both sides send `Hello` as
//! their first frame, then read the peer's. Each side refuses a peer that
//! speaks another protocol major version or sends anything else first.
//!
//! After that, [`Session::next_event`] drives the session. It answers the
//! peer's `Ping`s, sends a `Ping` of its own every
//! [`SessionConfig::ping_interval`], and fails with
//! [`SessionError::IdleTimeout`] when nothing arrives from the peer for
//! [`SessionConfig::idle_timeout`]. Feature messages come out as
//! [`SessionEvent::Message`] and go in through [`Session::send`].

use std::collections::BTreeSet;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use link2linux_protocol::{
    DEFAULT_MAX_FRAME_LEN, Envelope, EnvelopeCodec, FrameError, Hello, PROTOCOL_VERSION, Payload,
    Ping, Pong,
};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::time::{self, Instant, Interval, MissedTickBehavior};
use tokio_util::codec::Framed;
use tracing::debug;

use crate::device::{DeviceInfo, Feature};

/// Tunables for a [`Session`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionConfig {
    /// How long to wait for the peer's `Hello`.
    pub handshake_timeout: Duration,
    /// How often to ping the peer. Must not be zero.
    pub ping_interval: Duration,
    /// How long the peer may stay silent before the session is considered
    /// dead. Keep it a few times [`ping_interval`](Self::ping_interval), so
    /// one late `Pong` does not end the session.
    pub idle_timeout: Duration,
    /// Largest frame body accepted or sent, in bytes.
    pub max_frame_len: usize,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            handshake_timeout: Duration::from_secs(10),
            ping_interval: Duration::from_secs(15),
            idle_timeout: Duration::from_secs(45),
            max_frame_len: DEFAULT_MAX_FRAME_LEN,
        }
    }
}

/// Something that happened on a session.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
#[allow(
    clippy::large_enum_variant,
    reason = "messages are the common case; boxing them would only add an allocation"
)]
pub enum SessionEvent {
    /// A feature message from the peer.
    Message {
        /// The peer's sequence number for it.
        seq: u64,
        /// The message.
        payload: Payload,
    },
    /// The peer answered this side's latest `Ping`.
    Pong {
        /// Time from sending the `Ping` to receiving the `Pong`.
        rtt: Duration,
    },
}

/// Why a session failed or ended abnormally.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SessionError {
    /// Reading or writing a frame failed, including I/O errors.
    #[error(transparent)]
    Frame(#[from] FrameError),
    /// The peer's `Hello` did not arrive in time.
    #[error("no Hello from the peer within {0:?}")]
    HandshakeTimeout(Duration),
    /// The peer closed the stream before sending its `Hello`.
    #[error("the peer closed the connection during the handshake")]
    ClosedDuringHandshake,
    /// The peer's first frame was not a `Hello`.
    #[error("the peer's first message was not a Hello")]
    ExpectedHello,
    /// The peer speaks another protocol major version.
    #[error("the peer speaks protocol version {remote}, this device speaks {local}")]
    VersionMismatch {
        /// This device's [`PROTOCOL_VERSION`].
        local: u32,
        /// The version in the peer's `Hello`.
        remote: u32,
    },
    /// The peer's `Hello` is missing required information.
    #[error("invalid Hello from the peer: {0}")]
    InvalidHello(&'static str),
    /// The peer's sequence numbers did not increase.
    #[error("the peer sent sequence number {received} after {previous}")]
    OutOfOrder {
        /// The last sequence number accepted from the peer (0 before any).
        previous: u64,
        /// The offending sequence number.
        received: u64,
    },
    /// A `Hello` outside the handshake, from the peer or passed to
    /// [`Session::send`].
    #[error("Hello is only allowed as the first message")]
    UnexpectedHello,
    /// Nothing arrived from the peer for this long.
    #[error("no message from the peer for {0:?}")]
    IdleTimeout(Duration),
}

/// An established session with a peer device.
///
/// Created by [`Session::handshake`], then driven by calling
/// [`Session::next_event`] in a loop.
#[derive(Debug)]
pub struct Session<S> {
    framed: Framed<S, EnvelopeCodec>,
    local: DeviceInfo,
    peer: DeviceInfo,
    idle_timeout: Duration,
    /// Sequence number of the next envelope this side sends.
    next_seq: u64,
    /// Highest sequence number received from the peer.
    last_peer_seq: u64,
    /// When the last frame from the peer arrived.
    last_received: Instant,
    ping_timer: Interval,
    next_nonce: u64,
    /// Nonce and send time of the `Ping` awaiting its `Pong`.
    pending_ping: Option<(u64, Instant)>,
    rtt: Option<Duration>,
}

impl<S> Session<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    /// Runs the `Hello` exchange on `io` and returns the established
    /// session.
    ///
    /// This side's `Hello` goes out first, without waiting for the peer, so
    /// both ends may call this at the same time. That relies on the stream
    /// buffering one small `Hello` frame, which every real transport does.
    ///
    /// # Errors
    ///
    /// Fails if the peer's `Hello` does not arrive within
    /// [`SessionConfig::handshake_timeout`], if the peer sends anything else
    /// first, speaks another protocol version, or sends an invalid `Hello`,
    /// and on any framing or I/O error.
    ///
    /// # Panics
    ///
    /// Panics if [`SessionConfig::ping_interval`] is zero.
    pub async fn handshake(
        io: S,
        local: DeviceInfo,
        config: SessionConfig,
    ) -> Result<Self, SessionError> {
        let codec = EnvelopeCodec::with_max_frame_len(config.max_frame_len);
        let mut framed = Framed::new(io, codec);
        let hello = Envelope {
            seq: 1,
            payload: Some(Payload::Hello(hello_from(&local))),
            ..Envelope::default()
        };

        let exchange = async {
            framed.send(&hello).await?;
            match framed.next().await {
                Some(frame) => Ok(frame?),
                None => Err(SessionError::ClosedDuringHandshake),
            }
        };
        let first = time::timeout(config.handshake_timeout, exchange)
            .await
            .map_err(|_| SessionError::HandshakeTimeout(config.handshake_timeout))??;

        check_seq(0, first.seq)?;
        let Some(Payload::Hello(hello)) = first.payload else {
            return Err(SessionError::ExpectedHello);
        };
        let peer = peer_from(hello)?;
        debug!(peer.id, peer.name, ?peer.kind, "session established");

        let now = Instant::now();
        let mut ping_timer = time::interval_at(now + config.ping_interval, config.ping_interval);
        ping_timer.set_missed_tick_behavior(MissedTickBehavior::Delay);
        Ok(Self {
            framed,
            local,
            peer,
            idle_timeout: config.idle_timeout,
            next_seq: 2,
            last_peer_seq: first.seq,
            last_received: now,
            ping_timer,
            next_nonce: 1,
            pending_ping: None,
            rtt: None,
        })
    }

    /// This device, as announced to the peer.
    #[must_use]
    pub fn local(&self) -> &DeviceInfo {
        &self.local
    }

    /// The peer device, as it announced itself.
    #[must_use]
    pub fn peer(&self) -> &DeviceInfo {
        &self.peer
    }

    /// Features both devices announced, and so can use in this session.
    #[must_use]
    pub fn shared_features(&self) -> BTreeSet<Feature> {
        self.local
            .features
            .intersection(&self.peer.features)
            .copied()
            .collect()
    }

    /// The latest measured round-trip time, once a `Ping` has been answered.
    #[must_use]
    pub fn rtt(&self) -> Option<Duration> {
        self.rtt
    }

    /// Sends a feature message and returns its sequence number.
    ///
    /// # Errors
    ///
    /// Fails with [`SessionError::UnexpectedHello`] for a `Hello`, and on
    /// framing or I/O errors.
    pub async fn send(&mut self, payload: Payload) -> Result<u64, SessionError> {
        if matches!(payload, Payload::Hello(_)) {
            return Err(SessionError::UnexpectedHello);
        }
        self.send_payload(payload).await
    }

    /// Sends a `Ping` now. Its `Pong` comes out of [`Session::next_event`]
    /// as [`SessionEvent::Pong`]. A newer `Ping` supersedes an unanswered
    /// one.
    ///
    /// # Errors
    ///
    /// Fails on framing or I/O errors.
    pub async fn ping(&mut self) -> Result<(), SessionError> {
        let nonce = self.next_nonce;
        self.next_nonce = self.next_nonce.wrapping_add(1);
        self.pending_ping = Some((nonce, Instant::now()));
        self.send_payload(Payload::Ping(Ping { nonce })).await?;
        Ok(())
    }

    /// Waits for the next event, handling liveness on the way.
    ///
    /// Returns `Ok(None)` once the peer has closed the stream. Dropping the
    /// returned future before it completes loses no incoming message.
    ///
    /// # Errors
    ///
    /// Fails with [`SessionError::IdleTimeout`] when the peer goes silent,
    /// on protocol violations ([`SessionError::OutOfOrder`],
    /// [`SessionError::UnexpectedHello`]), and on framing or I/O errors.
    /// The session is unusable after an error.
    pub async fn next_event(&mut self) -> Result<Option<SessionEvent>, SessionError> {
        loop {
            let idle_deadline = self.last_received + self.idle_timeout;
            tokio::select! {
                frame = self.framed.next() => {
                    let Some(frame) = frame else {
                        return Ok(None);
                    };
                    self.last_received = Instant::now();
                    if let Some(event) = self.handle(frame?).await? {
                        return Ok(Some(event));
                    }
                }
                _ = self.ping_timer.tick() => self.ping().await?,
                () = time::sleep_until(idle_deadline) => {
                    return Err(SessionError::IdleTimeout(self.idle_timeout));
                }
            }
        }
    }

    /// Flushes pending frames and shuts the stream down.
    ///
    /// # Errors
    ///
    /// Fails if the final flush or the shutdown fails.
    pub async fn close(mut self) -> Result<(), SessionError> {
        self.framed.close().await?;
        Ok(())
    }

    /// Handles one incoming envelope, returning what the caller should see.
    async fn handle(&mut self, envelope: Envelope) -> Result<Option<SessionEvent>, SessionError> {
        check_seq(self.last_peer_seq, envelope.seq)?;
        self.last_peer_seq = envelope.seq;
        match envelope.payload {
            // A message type added by a newer version: skip it.
            None => {
                debug!(
                    seq = envelope.seq,
                    "ignoring a message this build does not know"
                );
                Ok(None)
            }
            Some(Payload::Hello(_)) => Err(SessionError::UnexpectedHello),
            Some(Payload::Ping(Ping { nonce })) => {
                self.send_payload(Payload::Pong(Pong { nonce })).await?;
                Ok(None)
            }
            Some(Payload::Pong(Pong { nonce })) => match self.pending_ping {
                Some((pending, sent_at)) if pending == nonce => {
                    self.pending_ping = None;
                    let rtt = sent_at.elapsed();
                    self.rtt = Some(rtt);
                    Ok(Some(SessionEvent::Pong { rtt }))
                }
                _ => {
                    debug!(nonce, "ignoring a Pong for a superseded Ping");
                    Ok(None)
                }
            },
            // Everything else is for the layers above the session.
            Some(payload) => Ok(Some(SessionEvent::Message {
                seq: envelope.seq,
                payload,
            })),
        }
    }

    /// Sends `payload` with the next sequence number.
    async fn send_payload(&mut self, payload: Payload) -> Result<u64, SessionError> {
        let seq = self.next_seq;
        self.next_seq += 1;
        let envelope = Envelope {
            seq,
            payload: Some(payload),
            ..Envelope::default()
        };
        self.framed.send(&envelope).await?;
        Ok(seq)
    }
}

/// Builds this device's `Hello`.
fn hello_from(local: &DeviceInfo) -> Hello {
    Hello {
        protocol_version: PROTOCOL_VERSION,
        device_id: local.id.clone(),
        device_name: local.name.clone(),
        kind: local.kind.into(),
        features: local
            .features
            .iter()
            .map(|feature| feature.name().to_owned())
            .collect(),
        ..Hello::default()
    }
}

/// Validates the peer's `Hello` and turns it into a [`DeviceInfo`].
fn peer_from(hello: Hello) -> Result<DeviceInfo, SessionError> {
    if hello.protocol_version != PROTOCOL_VERSION {
        return Err(SessionError::VersionMismatch {
            local: PROTOCOL_VERSION,
            remote: hello.protocol_version,
        });
    }
    if hello.device_id.is_empty() {
        return Err(SessionError::InvalidHello("empty device_id"));
    }
    // Device kinds and features added by newer versions read as unknown.
    let kind = hello.kind();
    let mut features = BTreeSet::new();
    for name in &hello.features {
        match Feature::from_name(name) {
            Some(feature) => {
                features.insert(feature);
            }
            None => debug!(feature = %name, "ignoring a feature this build does not know"),
        }
    }
    Ok(DeviceInfo {
        id: hello.device_id,
        name: hello.device_name,
        kind,
        features,
    })
}

/// Checks that a sequence number from the peer is above the last one.
fn check_seq(previous: u64, received: u64) -> Result<(), SessionError> {
    if received > previous {
        Ok(())
    } else {
        Err(SessionError::OutOfOrder { previous, received })
    }
}
