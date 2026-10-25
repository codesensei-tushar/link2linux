//! A session between two paired devices, on top of an established stream.
//!
//! The stream is normally the TLS connection set up by the transport layer,
//! but any `AsyncRead + AsyncWrite` works, which is how the tests run
//! sessions over in-memory pipes.
//!
//! A session starts with the `Hello` exchange: both sides send `Hello` as
//! their first frame, then read the peer's. The session uses the highest
//! protocol version both sides speak (see [`VersionRange`]). When they share
//! none, each side sends `Goodbye` with `GOODBYE_REASON_VERSION_MISMATCH`
//! and gives up. A peer that sends anything else first is refused.
//!
//! After that, [`Session::next_event`] drives the session. It answers the
//! peer's `Ping`s, sends a `Ping` of its own every
//! [`SessionConfig::ping_interval`], and fails with
//! [`SessionError::IdleTimeout`] when nothing arrives from the peer for
//! [`SessionConfig::idle_timeout`]. Feature messages come out as
//! [`SessionEvent::Message`] and go in through [`Session::send`].
//!
//! A session ends with a `Goodbye`. The peer's comes out as
//! [`SessionEvent::Goodbye`], whose [`ReconnectPolicy`] tells the desktop
//! what to do next; [`Session::goodbye`] sends this side's. When the peer
//! breaks a protocol rule, the session sends `Goodbye` with
//! `GOODBYE_REASON_PROTOCOL_ERROR` before it reports the error.

use std::collections::{BTreeSet, VecDeque};
use std::future::poll_fn;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use futures_util::{FutureExt, Sink, SinkExt, Stream, StreamExt};
use link2linux_protocol::v1::{Goodbye, GoodbyeReason};
use link2linux_protocol::{
    DEFAULT_MAX_FRAME_LEN, Envelope, EnvelopeCodec, FrameError, Hello, Payload, Ping, Pong,
};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::time::{self, Instant, Interval, MissedTickBehavior};
use tokio_util::codec::{Encoder, Framed};
use tracing::debug;

use crate::device::{DeviceInfo, Feature};

mod goodbye;
mod version;

pub use goodbye::{PeerGoodbye, ReconnectPolicy};
pub use version::VersionRange;

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
    /// The peer ended the session with a `Goodbye`. It is the last event:
    /// [`Session::next_event`] returns `Ok(None)` from then on.
    Goodbye(PeerGoodbye),
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
    /// The two devices share no protocol version. If
    /// [`remote.is_older_than(local)`](VersionRange::is_older_than), the
    /// peer's app needs an update, otherwise this one.
    #[error("the peer speaks protocol version {remote}, this device speaks {local}")]
    VersionMismatch {
        /// The versions this device speaks.
        local: VersionRange,
        /// The versions the peer's `Hello` announced.
        remote: VersionRange,
    },
    /// The peer's `Hello` is missing required information.
    #[error("invalid Hello from the peer: {0}")]
    InvalidHello(&'static str),
    /// The peer's sequence number was not exactly one above its previous
    /// one: a frame was lost, repeated or reordered.
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
    /// This side tried to send a message the session sends by itself or
    /// through a dedicated method, such as a `Goodbye` through
    /// [`Session::send`] instead of [`Session::goodbye`].
    #[error("{0} cannot be sent this way")]
    NotAllowed(&'static str),
    /// The session has already ended.
    #[error("the session has ended")]
    Ended,
}

impl SessionError {
    /// Whether the peer broke a protocol rule. The session then sends
    /// `Goodbye` with `GOODBYE_REASON_PROTOCOL_ERROR` before it reports the
    /// error.
    #[must_use]
    pub fn is_protocol_error(&self) -> bool {
        match self {
            Self::Frame(err) => !matches!(err, FrameError::Io(_) | FrameError::Encode(_)),
            Self::ExpectedHello
            | Self::InvalidHello(_)
            | Self::OutOfOrder { .. }
            | Self::UnexpectedHello => true,
            _ => false,
        }
    }

    /// What the desktop does after a session failed with this error.
    #[must_use]
    pub fn reconnect_policy(&self) -> ReconnectPolicy {
        match self {
            Self::VersionMismatch { .. } => ReconnectPolicy::WaitForUpdate,
            _ => ReconnectPolicy::after_loss(),
        }
    }
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
    /// The protocol major version both sides use.
    version: u32,
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
    /// Events taken off the stream that `next_event` has not returned yet.
    events: VecDeque<SessionEvent>,
    /// Whether frames were queued since the last completed flush.
    unflushed: bool,
    /// Whether the session is over: a `Goodbye` went either way, the peer
    /// closed the stream, or the session failed.
    ended: bool,
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
    /// first or sends an invalid `Hello`, and on any framing or I/O error.
    /// When the two sides share no protocol version, it sends `Goodbye` with
    /// `GOODBYE_REASON_VERSION_MISMATCH`, shuts the stream down and fails
    /// with [`SessionError::VersionMismatch`].
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
        let result = time::timeout(config.handshake_timeout, exchange)
            .await
            .map_err(|_| SessionError::HandshakeTimeout(config.handshake_timeout))
            .flatten()
            .and_then(accept_hello);
        let (peer, version, last_peer_seq) = match result {
            Ok(accepted) => accepted,
            Err(err) => {
                let (reason, detail) = match &err {
                    SessionError::VersionMismatch { local, .. } => (
                        GoodbyeReason::VersionMismatch,
                        format!("the sender speaks protocol version {local}"),
                    ),
                    _ if err.is_protocol_error() => (GoodbyeReason::ProtocolError, err.to_string()),
                    _ => return Err(err),
                };
                let goodbye = Envelope {
                    seq: 2,
                    payload: Some(Payload::Goodbye(Goodbye {
                        reason: reason.into(),
                        detail,
                    })),
                    ..Envelope::default()
                };
                let farewell = async {
                    framed.send(&goodbye).await?;
                    SinkExt::<&Envelope>::close(&mut framed).await
                };
                // Best effort: the peer may already have hung up.
                let _ = time::timeout(config.handshake_timeout, farewell).await;
                return Err(err);
            }
        };
        debug!(peer.id, peer.name, ?peer.kind, version, "session established");

        let now = Instant::now();
        let mut ping_timer = time::interval_at(now + config.ping_interval, config.ping_interval);
        ping_timer.set_missed_tick_behavior(MissedTickBehavior::Delay);
        Ok(Self {
            framed,
            local,
            peer,
            version,
            idle_timeout: config.idle_timeout,
            next_seq: 2,
            last_peer_seq,
            last_received: now,
            ping_timer,
            next_nonce: 1,
            pending_ping: None,
            rtt: None,
            events: VecDeque::new(),
            unflushed: false,
            ended: false,
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

    /// The protocol major version this session uses: the highest one both
    /// devices speak.
    #[must_use]
    pub fn protocol_version(&self) -> u32 {
        self.version
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

    /// Whether the session is over: a `Goodbye` went either way, the peer
    /// closed the stream, or the session failed.
    #[must_use]
    pub fn has_ended(&self) -> bool {
        self.ended
    }

    /// Sends a feature message and returns its sequence number.
    ///
    /// The message is numbered and queued before the first `await`, so if
    /// the returned future is dropped early, the message still goes out
    /// with the next write.
    ///
    /// # Errors
    ///
    /// Fails with [`SessionError::UnexpectedHello`] for a `Hello`, with
    /// [`SessionError::NotAllowed`] for a `Goodbye` (see
    /// [`Session::goodbye`]), with [`SessionError::Ended`] once the session
    /// is over, and on framing or I/O errors.
    pub async fn send(&mut self, payload: Payload) -> Result<u64, SessionError> {
        match payload {
            Payload::Hello(_) => return Err(SessionError::UnexpectedHello),
            Payload::Goodbye(_) => return Err(SessionError::NotAllowed("Goodbye")),
            _ => {}
        }
        if self.ended {
            return Err(SessionError::Ended);
        }
        let seq = self.enqueue(payload)?;
        self.flush().await?;
        Ok(seq)
    }

    /// Sends a `Ping` now. Its `Pong` comes out of [`Session::next_event`]
    /// as [`SessionEvent::Pong`]. A newer `Ping` supersedes an unanswered
    /// one.
    ///
    /// # Errors
    ///
    /// Fails with [`SessionError::Ended`] once the session is over, and on
    /// framing or I/O errors.
    pub async fn ping(&mut self) -> Result<(), SessionError> {
        if self.ended {
            return Err(SessionError::Ended);
        }
        self.queue_ping()?;
        self.flush().await
    }

    /// Waits for the next event, handling liveness on the way.
    ///
    /// Returns `Ok(None)` once the session is over: after the peer's
    /// [`SessionEvent::Goodbye`], or when the peer closed the stream without
    /// one, which counts as a lost connection
    /// ([`ReconnectPolicy::after_loss`]).
    ///
    /// This method is cancel safe: dropping the returned future before it
    /// completes loses no incoming message, and frames queued for sending
    /// stay queued.
    ///
    /// # Errors
    ///
    /// Fails with [`SessionError::IdleTimeout`] when the peer goes silent,
    /// on protocol violations ([`SessionError::OutOfOrder`],
    /// [`SessionError::UnexpectedHello`], undecodable frames), and on I/O
    /// errors. On a protocol violation the session first queues a `Goodbye`
    /// with `GOODBYE_REASON_PROTOCOL_ERROR` and writes it if it can do so
    /// without waiting; [`Session::close`] flushes it. The session is over
    /// after an error.
    pub async fn next_event(&mut self) -> Result<Option<SessionEvent>, SessionError> {
        loop {
            if let Some(event) = self.events.pop_front() {
                return Ok(Some(event));
            }
            if self.ended {
                return Ok(None);
            }
            self.step().await?;
        }
    }

    /// Ends the session on purpose: sends `Goodbye` with `reason`, flushes
    /// and shuts the stream down. `detail` is English text for the peer's
    /// logs.
    ///
    /// # Errors
    ///
    /// Fails if the final writes or the shutdown fail.
    pub async fn goodbye(
        mut self,
        reason: GoodbyeReason,
        detail: &str,
    ) -> Result<(), SessionError> {
        if !self.ended {
            self.ended = true;
            self.enqueue(Payload::Goodbye(Goodbye {
                reason: reason.into(),
                detail: detail.to_owned(),
            }))?;
        }
        self.close().await
    }

    /// Flushes queued frames and shuts the stream down, without a
    /// `Goodbye`. Use it once the session is over; to end a running session
    /// on purpose, use [`Session::goodbye`].
    ///
    /// # Errors
    ///
    /// Fails if the final flush or the shutdown fails.
    pub async fn close(mut self) -> Result<(), SessionError> {
        SinkExt::<&Envelope>::close(&mut self.framed).await?;
        Ok(())
    }

    /// Waits for one thing to happen on the session (a frame arrives, the
    /// ping timer fires, the peer goes silent) and handles it. Queued frames
    /// are written in the meantime.
    ///
    /// Cancel safe: everything after an `await` point is synchronous.
    async fn step(&mut self) -> Result<(), SessionError> {
        let idle_deadline = self.last_received + self.idle_timeout;
        tokio::select! {
            io = poll_fn(|cx| poll_io(&mut self.framed, &mut self.unflushed, cx)) => match io {
                Io::Frame(envelope) => {
                    self.last_received = Instant::now();
                    match self.handle(envelope) {
                        Ok(()) => Ok(()),
                        Err(err) => Err(self.fail(err)),
                    }
                }
                Io::Closed => {
                    debug!("the peer closed the stream");
                    self.ended = true;
                    Ok(())
                }
                Io::ReadFailed(err) | Io::WriteFailed(err) => Err(self.fail(err.into())),
            },
            _ = self.ping_timer.tick() => self.queue_ping(),
            () = time::sleep_until(idle_deadline) => {
                Err(self.fail(SessionError::IdleTimeout(self.idle_timeout)))
            }
        }
    }

    /// Handles one incoming envelope, queueing what the caller should see.
    fn handle(&mut self, envelope: Envelope) -> Result<(), SessionError> {
        check_seq(self.last_peer_seq, envelope.seq)?;
        self.last_peer_seq = envelope.seq;
        match envelope.payload {
            // A message type added by a newer version: skip it.
            None => {
                debug!(
                    seq = envelope.seq,
                    "ignoring a message this build does not know"
                );
            }
            Some(Payload::Hello(_)) => return Err(SessionError::UnexpectedHello),
            Some(Payload::Ping(Ping { nonce })) => {
                self.enqueue(Payload::Pong(Pong { nonce }))?;
            }
            Some(Payload::Pong(Pong { nonce })) => match self.pending_ping {
                Some((pending, sent_at)) if pending == nonce => {
                    self.pending_ping = None;
                    let rtt = sent_at.elapsed();
                    self.rtt = Some(rtt);
                    self.events.push_back(SessionEvent::Pong { rtt });
                }
                _ => debug!(nonce, "ignoring a Pong for a superseded Ping"),
            },
            Some(Payload::Goodbye(goodbye)) => {
                let goodbye = PeerGoodbye::from(goodbye);
                debug!(reason = ?goodbye.reason, goodbye.detail, "the peer ended the session");
                self.ended = true;
                self.events.push_back(SessionEvent::Goodbye(goodbye));
            }
            // Everything else is for the layers above the session.
            Some(payload) => self.events.push_back(SessionEvent::Message {
                seq: envelope.seq,
                payload,
            }),
        }
        Ok(())
    }

    /// Ends the session after `err`. For a protocol violation by the peer,
    /// queues a `Goodbye` saying so and writes it if that needs no waiting.
    fn fail(&mut self, err: SessionError) -> SessionError {
        if !self.ended && err.is_protocol_error() {
            let goodbye = Payload::Goodbye(Goodbye {
                reason: GoodbyeReason::ProtocolError.into(),
                detail: err.to_string(),
            });
            if self.enqueue(goodbye).is_ok() {
                let _ = poll_fn(|cx| poll_flush(&mut self.framed, &mut self.unflushed, cx))
                    .now_or_never();
            }
        }
        self.ended = true;
        err
    }

    /// Numbers `payload` and queues it for writing, returning its sequence
    /// number. Nothing is numbered if the envelope cannot be encoded.
    fn enqueue(&mut self, payload: Payload) -> Result<u64, SessionError> {
        let seq = self.next_seq;
        let envelope = Envelope {
            seq,
            payload: Some(payload),
            ..Envelope::default()
        };
        let mut codec = *self.framed.codec();
        codec.encode(&envelope, self.framed.write_buffer_mut())?;
        self.next_seq += 1;
        self.unflushed = true;
        Ok(seq)
    }

    /// Queues a `Ping` with a new nonce.
    fn queue_ping(&mut self) -> Result<(), SessionError> {
        let nonce = self.next_nonce;
        self.next_nonce = self.next_nonce.wrapping_add(1);
        self.pending_ping = Some((nonce, Instant::now()));
        self.enqueue(Payload::Ping(Ping { nonce }))?;
        Ok(())
    }

    /// Writes every queued frame and flushes the stream.
    async fn flush(&mut self) -> Result<(), SessionError> {
        poll_fn(|cx| poll_flush(&mut self.framed, &mut self.unflushed, cx)).await?;
        Ok(())
    }
}

/// What [`poll_io`] saw.
#[allow(
    clippy::large_enum_variant,
    reason = "lives for one match; boxing the envelope would add an allocation per frame"
)]
enum Io {
    /// A frame arrived.
    Frame(Envelope),
    /// The peer closed the stream.
    Closed,
    /// Reading failed; the stream is out of sync or broken.
    ReadFailed(FrameError),
    /// Writing queued frames failed.
    WriteFailed(FrameError),
}

/// Writes queued frames while waiting for the next incoming one, so a slow
/// write never stops the session from reading.
fn poll_io<S>(
    framed: &mut Framed<S, EnvelopeCodec>,
    unflushed: &mut bool,
    cx: &mut Context<'_>,
) -> Poll<Io>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    if let Poll::Ready(Err(err)) = poll_flush(framed, unflushed, cx) {
        return Poll::Ready(Io::WriteFailed(err));
    }
    Pin::new(framed).poll_next(cx).map(|frame| match frame {
        Some(Ok(envelope)) => Io::Frame(envelope),
        Some(Err(err)) => Io::ReadFailed(err),
        None => Io::Closed,
    })
}

/// Writes queued frames and flushes the stream, if anything was queued.
fn poll_flush<S>(
    framed: &mut Framed<S, EnvelopeCodec>,
    unflushed: &mut bool,
    cx: &mut Context<'_>,
) -> Poll<Result<(), FrameError>>
where
    S: AsyncWrite + Unpin,
{
    if !*unflushed {
        return Poll::Ready(Ok(()));
    }
    let flushed = Sink::<&Envelope>::poll_flush(Pin::new(framed), cx);
    if let Poll::Ready(Ok(())) = flushed {
        *unflushed = false;
    }
    flushed
}

/// Builds this device's `Hello`.
fn hello_from(local: &DeviceInfo) -> Hello {
    let mut hello = Hello {
        device_id: local.id.clone(),
        device_name: local.name.clone(),
        kind: local.kind.into(),
        features: local
            .features
            .iter()
            .map(|feature| feature.name().to_owned())
            .collect(),
        ..Hello::default()
    };
    VersionRange::SUPPORTED.write_to(&mut hello);
    hello
}

/// Checks the peer's first envelope. Returns who the peer is, the protocol
/// version both sides use, and the envelope's sequence number.
fn accept_hello(first: Envelope) -> Result<(DeviceInfo, u32, u64), SessionError> {
    check_seq(0, first.seq)?;
    let Some(Payload::Hello(hello)) = first.payload else {
        return Err(SessionError::ExpectedHello);
    };
    let remote = VersionRange::of_hello(&hello).ok_or(SessionError::InvalidHello(
        "no valid protocol version range",
    ))?;
    let local = VersionRange::SUPPORTED;
    let version = local
        .negotiate(remote)
        .ok_or(SessionError::VersionMismatch { local, remote })?;
    Ok((peer_from(hello)?, version, first.seq))
}

/// Validates the peer's `Hello`, apart from its versions, and turns it into
/// a [`DeviceInfo`].
fn peer_from(hello: Hello) -> Result<DeviceInfo, SessionError> {
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

/// Checks that a sequence number from the peer is exactly one above the
/// last one, which catches lost and repeated frames.
fn check_seq(previous: u64, received: u64) -> Result<(), SessionError> {
    if previous.checked_add(1) == Some(received) {
        Ok(())
    } else {
        Err(SessionError::OutOfOrder { previous, received })
    }
}
