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
//! Right after the `Hello`s, each side sends `FeatureStatus`, how ready each
//! of its features is (see [`Availability`]), then `PresenceUpdate`, and
//! sends them again whenever they change ([`Session::set_availability`],
//! [`Session::set_presence`]). The peer's come out as
//! [`SessionEvent::FeatureStatus`] and [`SessionEvent::Presence`]. A request
//! of a feature only goes out while the feature is ready on both sides.
//!
//! On a pairing connection (one that the desktop dialed to pair, which its
//! `Hello` says), the pairing exchange comes first: the session carries only
//! pairing messages, and sends `FeatureStatus` and `PresenceUpdate` once
//! [`Session::finish_pairing`] says the pairing is stored. On every other
//! connection, [`SessionStart::expected_peer_id`] names the device the
//! peer's key belongs to, and a `Hello` with another device id is a
//! protocol error.
//!
//! After that, [`Session::next_event`] drives the session. It answers the
//! peer's `Ping`s and, on the desktop, sends its own on the
//! [`PingSchedule`]: often while [`Session::set_active`] says the user can
//! see the session's activity, rarely otherwise. It fails with
//! [`SessionError::PeerSilent`] when nothing arrives from the peer for a few
//! ping intervals, or on the phone, which never pings, for
//! [`SessionConfig::silence_timeout`]. One-way messages come out as
//! [`SessionEvent::Message`] and go in through [`Session::send`].
//!
//! This side's requests go out through [`Session::send_request`], which
//! waits for the answer, or [`Session::start_request`], whose answers come
//! out as [`SessionEvent::Reply`] so that several requests can wait at
//! once. A request without an answer within
//! [`SessionConfig::request_timeout`] fails.
//!
//! The peer's requests come out as [`SessionEvent::Request`], and
//! [`Session::reply`], [`Session::ack`] and [`Session::reply_error`] answer
//! them. The session applies the rules for messages it does not expect
//! itself: it answers an unknown request, or a request of a feature that is
//! not active, with `ERROR_CODE_UNSUPPORTED`, drops one-way messages of
//! inactive features and answers nobody waits for, and treats a message in
//! the wrong place as a protocol error.
//!
//! A session ends with a `Goodbye`. The peer's comes out as
//! [`SessionEvent::Goodbye`], whose [`ReconnectPolicy`] tells the desktop
//! what to do next; [`Session::goodbye`] sends this side's. When the peer
//! breaks a protocol rule, the session sends `Goodbye` with
//! `GOODBYE_REASON_PROTOCOL_ERROR` before it reports the error.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::future::poll_fn;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use futures_util::{FutureExt, Sink, SinkExt, Stream, StreamExt};
use link2linux_protocol::v1::{
    Ack, ErrorCode, ErrorReply, Goodbye, GoodbyeReason, PairingMethod, PresenceUpdate,
};
use link2linux_protocol::{
    DEFAULT_MAX_FRAME_LEN, Envelope, EnvelopeCodec, FrameError, HANDSHAKE_MAX_FRAME_LEN, Hello,
    Payload, Ping, Pong,
};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::time::{self, Instant};
use tokio_util::codec::{Encoder, Framed};
use tracing::debug;

use crate::device::{DeviceInfo, Feature};
use crate::transport::TransportError;

mod goodbye;
mod liveness;
mod request;
mod rules;
mod status;
mod version;

pub use goodbye::{PeerGoodbye, ReconnectPolicy};
pub use liveness::PingSchedule;
pub use request::{Reply, RequestError};
pub use status::{Availability, FeatureStates};
pub use version::VersionRange;

use request::Pending;

use liveness::Liveness;
use rules::{Kind, Rules};

/// Tunables for a [`Session`]. The default suits the desktop;
/// [`SessionConfig::phone`] suits a stand-in for the phone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionConfig {
    /// How long to wait for the peer's `Hello`.
    pub handshake_timeout: Duration,
    /// When this side pings the peer, and how many silent intervals end the
    /// session. `None` on a side that only answers pings, as the phone
    /// does. Its intervals must not be zero.
    pub ping: Option<PingSchedule>,
    /// On a side that does not ping: how long the peer may stay silent
    /// before the session counts as lost.
    pub silence_timeout: Duration,
    /// How long a request waits for an answer, and for each further answer
    /// of a request answered by several messages.
    pub request_timeout: Duration,
    /// Largest frame body accepted or sent once the session has started,
    /// in bytes. Until then, during the `Hello` exchange and on a pairing
    /// connection until [`Session::finish_pairing`], frames are limited to
    /// [`HANDSHAKE_MAX_FRAME_LEN`], or this if it is smaller.
    pub max_frame_len: usize,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            handshake_timeout: Duration::from_secs(10),
            ping: Some(PingSchedule::default()),
            silence_timeout: Duration::from_secs(240),
            request_timeout: Duration::from_secs(30),
            max_frame_len: DEFAULT_MAX_FRAME_LEN,
        }
    }
}

impl SessionConfig {
    /// The phone's settings: it never pings, and counts the session as lost
    /// after 240 seconds without a frame, 4 times the longest interval the
    /// desktop may ping at.
    #[must_use]
    pub fn phone() -> Self {
        Self {
            ping: None,
            ..Self::default()
        }
    }
}

/// What this side brings to a session: who it is, and what it reports
/// right after the `Hello`s.
///
/// A [`DeviceInfo`] converts into one with every feature ready and no
/// presence information.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct SessionStart {
    /// This device, as its `Hello` announces it.
    pub device: DeviceInfo,
    /// How ready each of the device's features is, sent as
    /// `FeatureStatus`. Features without an entry are reported as ready;
    /// entries for features the device does not list are ignored.
    pub availability: FeatureStates,
    /// Sent as `PresenceUpdate`: the phone's presence key for this
    /// desktop, or the desktop's Bluetooth LE address.
    pub presence: PresenceUpdate,
    /// Set by the desktop when it dialed the phone to pair, as
    /// `Hello.pairing_method`. The session then starts with the pairing
    /// exchange; see [`Session::finish_pairing`].
    pub pairing: Option<PairingMethod>,
    /// The device id of the pairing record that the transport found by the
    /// key the peer presented. The peer's `Hello` must carry this id, or
    /// the handshake fails with [`SessionError::UnexpectedPeer`]. Never
    /// look a record up by the `Hello`'s device id instead: it is only
    /// what the peer claims.
    pub expected_peer_id: Option<String>,
}

impl SessionStart {
    /// A start for `device` with every feature ready and no presence
    /// information.
    #[must_use]
    pub fn new(device: DeviceInfo) -> Self {
        Self {
            device,
            availability: FeatureStates::new(),
            presence: PresenceUpdate::default(),
            pairing: None,
            expected_peer_id: None,
        }
    }
}

impl From<DeviceInfo> for SessionStart {
    fn from(device: DeviceInfo) -> Self {
        Self::new(device)
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
    /// A one-way message from the peer, such as a feature event.
    Message {
        /// The peer's sequence number for it.
        seq: u64,
        /// The message.
        payload: Payload,
    },
    /// A request from the peer. Answer it with [`Session::reply`],
    /// [`Session::ack`] or [`Session::reply_error`], passing `seq`. Requests
    /// of features that are not active never come out here: the session
    /// answers them with `ERROR_CODE_UNSUPPORTED` itself.
    Request {
        /// The peer's sequence number for it, which the answer refers to.
        seq: u64,
        /// The request.
        payload: Payload,
    },
    /// An answer to a request this side sent with
    /// [`Session::start_request`], or one whose [`Session::send_request`]
    /// was dropped before the answer arrived.
    Reply(Reply),
    /// A request this side sent with [`Session::start_request`] got no
    /// answer within [`SessionConfig::request_timeout`]. An answer that
    /// arrives later is ignored.
    RequestTimedOut {
        /// The request's sequence number.
        request: u64,
    },
    /// The peer's `FeatureStatus`: how ready each feature it listed in its
    /// `Hello` is. The first one follows the `Hello`; each later one
    /// replaces the previous one as a whole. See also
    /// [`Session::peer_availability`].
    FeatureStatus(FeatureStates),
    /// The peer's `PresenceUpdate`: how to recognize and reach it while it
    /// is not connected. It follows the first `FeatureStatus`, and comes
    /// again whenever its content changes.
    Presence(PresenceUpdate),
    /// The peer answered this side's latest `Ping`.
    Pong {
        /// Time from sending the `Ping` to receiving the `Pong`.
        rtt: Duration,
    },
    /// The peer ended the session with a `Goodbye`. It is the last event:
    /// [`Session::next_event`] returns `Ok(None)` from then on.
    Goodbye(PeerGoodbye),
    /// The frames waiting to be written fell to the length given to
    /// [`Session::wake_when_drained`].
    Drained,
}

impl SessionEvent {
    /// What the event is, such as the schema name of its message, for logs.
    /// Events can carry the user's content, so logs name them instead of
    /// printing them.
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Self::Message { payload, .. } | Self::Request { payload, .. } => {
                rules::of(payload).name
            }
            Self::Reply(reply) => rules::of(&reply.payload).name,
            Self::RequestTimedOut { .. } => "RequestTimedOut",
            Self::FeatureStatus(_) => "FeatureStatus",
            Self::Presence(_) => "PresenceUpdate",
            Self::Pong { .. } => "Pong",
            Self::Goodbye(_) => "Goodbye",
            Self::Drained => "Drained",
        }
    }
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
    /// The peer's `Hello` names another device than the pairing record
    /// that its key belongs to ([`SessionStart::expected_peer_id`]).
    #[error("the peer's key belongs to device {expected}, but its Hello says {received}")]
    UnexpectedPeer {
        /// The device id of the pairing record.
        expected: String,
        /// The device id in the peer's `Hello`.
        received: String,
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
    /// The peer sent a message where the protocol does not allow it, such
    /// as an answer that does not fit its request.
    #[error("protocol error by the peer: {0}")]
    Protocol(String),
    /// Nothing arrived from the peer for this long, so the connection
    /// counts as lost.
    #[error("no message from the peer for {0:?}")]
    PeerSilent(Duration),
    /// This side tried to send a message the session sends by itself or
    /// through a dedicated method, such as a `Goodbye` through
    /// [`Session::send`] instead of [`Session::goodbye`].
    #[error("{0} cannot be sent this way")]
    NotAllowed(&'static str),
    /// This side tried to send a message of a feature that is not active in
    /// this session.
    #[error("{0} is not active in this session")]
    FeatureNotActive(Feature),
    /// This side tried to send a request of a feature that is not ready on
    /// both devices.
    #[error("{0} is not ready on both devices")]
    FeatureNotReady(Feature),
    /// [`Session::reply`] named a request the peer has not sent.
    #[error("the peer sent no message {0} to answer")]
    NoSuchRequest(u64),
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
            | Self::UnexpectedHello
            | Self::UnexpectedPeer { .. }
            | Self::Protocol(_) => true,
            _ => false,
        }
    }

    /// Whether the peer refused this device's certificate
    /// ([`TransportError::RejectedByPeer`]). On the desktop the refusal
    /// arrives on the first read after the TLS handshake, which is in
    /// [`Session::handshake`], when the phone does not, or no longer, trust
    /// its key.
    #[must_use]
    pub fn is_rejected_by_peer(&self) -> bool {
        matches!(self, Self::Frame(FrameError::Io(error)) if TransportError::is_refusal(error))
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
    /// This side's availability of every feature it listed.
    availability: FeatureStates,
    presence: PresenceUpdate,
    /// The peer's latest `FeatureStatus`, once one has arrived.
    peer_availability: Option<FeatureStates>,
    /// The peer's latest `PresenceUpdate`, once one has arrived.
    peer_presence: Option<PresenceUpdate>,
    /// The pairing method, on a pairing connection.
    pairing_method: Option<PairingMethod>,
    /// Whether this side is still pairing: on a pairing connection, until
    /// `finish_pairing`.
    pairing: bool,
    /// The protocol major version both sides use.
    version: u32,
    /// [`SessionConfig::max_frame_len`], the frame limit once the session
    /// has started.
    max_frame_len: usize,
    liveness: Liveness,
    request_timeout: Duration,
    /// This side's requests that wait for an answer, by sequence number.
    pending: BTreeMap<u64, Pending>,
    /// Sequence number of the next envelope this side sends.
    next_seq: u64,
    /// Highest sequence number received from the peer.
    last_peer_seq: u64,
    next_nonce: u64,
    /// Nonce and send time of the `Ping` awaiting its `Pong`.
    pending_ping: Option<(u64, Instant)>,
    rtt: Option<Duration>,
    /// Events taken off the stream that `next_event` has not returned yet.
    events: VecDeque<SessionEvent>,
    /// An `ErrorReply` the peer sent without `reply_to`, which explains the
    /// `Goodbye` that should follow.
    peer_error: Option<ErrorReply>,
    /// Whether frames were queued since the last completed flush.
    unflushed: bool,
    /// The queued length at which to report [`SessionEvent::Drained`].
    drain_mark: Option<usize>,
    /// Whether the session is over: a `Goodbye` went either way, the peer
    /// closed the stream, or the session failed.
    ended: bool,
}

impl<S> Session<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    /// Runs the `Hello` exchange on `io`, sends this side's
    /// `FeatureStatus` and `PresenceUpdate` unless this is a pairing
    /// connection, and returns the established session.
    ///
    /// This side's `Hello` goes out first, without waiting for the peer, so
    /// both ends may call this at the same time. That relies on the stream
    /// buffering one small `Hello` frame, which every real transport does.
    ///
    /// # Errors
    ///
    /// Fails if the peer's `Hello` does not arrive within
    /// [`SessionConfig::handshake_timeout`], if the peer sends anything else
    /// first or sends an invalid `Hello`, with
    /// [`SessionError::UnexpectedPeer`] when its device id is not the
    /// expected one, and on any framing or I/O error. On a protocol error it
    /// sends `Goodbye` with `GOODBYE_REASON_PROTOCOL_ERROR` first.
    /// When the two sides share no protocol version, it sends `Goodbye` with
    /// `GOODBYE_REASON_VERSION_MISMATCH`, shuts the stream down and fails
    /// with [`SessionError::VersionMismatch`].
    ///
    /// # Panics
    ///
    /// Panics if an interval of [`SessionConfig::ping`] is zero.
    pub async fn handshake(
        io: S,
        start: impl Into<SessionStart>,
        config: SessionConfig,
    ) -> Result<Self, SessionError> {
        let SessionStart {
            device: local,
            availability,
            presence,
            pairing,
            expected_peer_id,
        } = start.into();
        // The peer may not be paired yet, so its frames stay small until the
        // session starts.
        let codec =
            EnvelopeCodec::with_max_frame_len(config.max_frame_len.min(HANDSHAKE_MAX_FRAME_LEN));
        let mut framed = Framed::new(io, codec);
        let mut hello = hello_from(&local);
        if let Some(method) = pairing {
            hello.pairing_method = method.into();
        }
        let hello = Envelope {
            seq: 1,
            payload: Some(Payload::Hello(hello)),
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
            .and_then(|first| accept_hello(first, expected_peer_id.as_deref()));
        let (peer, version, last_peer_seq, peer_pairing) = match result {
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
        // The desktop's Hello decides whether this is a pairing connection.
        let pairing_method = pairing.or(peer_pairing);
        debug!(peer.id, peer.name, ?peer.kind, version, ?pairing_method, "session established");

        let liveness = Liveness::new(config.ping, config.silence_timeout, Instant::now());
        let availability = local
            .features
            .iter()
            .map(|&feature| {
                let state = availability.get(&feature).cloned();
                (feature, state.unwrap_or(Availability::Ready))
            })
            .collect();
        let mut session = Self {
            framed,
            local,
            peer,
            availability,
            presence,
            peer_availability: None,
            peer_presence: None,
            pairing_method,
            pairing: pairing_method.is_some(),
            version,
            max_frame_len: config.max_frame_len,
            liveness,
            request_timeout: config.request_timeout,
            pending: BTreeMap::new(),
            next_seq: 2,
            last_peer_seq,
            next_nonce: 1,
            pending_ping: None,
            rtt: None,
            events: VecDeque::new(),
            peer_error: None,
            unflushed: false,
            drain_mark: None,
            ended: false,
        };
        if !session.pairing {
            session.start()?;
        }
        time::timeout(config.handshake_timeout, session.flush())
            .await
            .map_err(|_| SessionError::HandshakeTimeout(config.handshake_timeout))??;
        Ok(session)
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

    /// The pairing method, on a connection the desktop dialed to pair.
    #[must_use]
    pub fn pairing_method(&self) -> Option<PairingMethod> {
        self.pairing_method
    }

    /// Whether this side is still in the pairing exchange: on a pairing
    /// connection, until [`Session::finish_pairing`].
    #[must_use]
    pub fn is_pairing(&self) -> bool {
        self.pairing
    }

    /// Ends this side's pairing exchange once the pairing is stored, and
    /// starts the session proper: sends `FeatureStatus` and
    /// `PresenceUpdate`. Until then, only pairing messages go either way.
    /// Does nothing on a connection that is not, or no longer, pairing.
    ///
    /// # Errors
    ///
    /// Fails with [`SessionError::Ended`] once the session is over, and on
    /// framing or I/O errors.
    pub async fn finish_pairing(&mut self) -> Result<(), SessionError> {
        if self.ended {
            return Err(SessionError::Ended);
        }
        if !self.pairing {
            return Ok(());
        }
        self.pairing = false;
        self.start()?;
        self.flush().await
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

    /// This device's availability of each feature it listed in its
    /// `Hello`, as last reported to the peer.
    #[must_use]
    pub fn availability(&self) -> &FeatureStates {
        &self.availability
    }

    /// The peer's availability of each feature it listed in its `Hello`,
    /// from its latest `FeatureStatus`. `None` until the first one arrives.
    #[must_use]
    pub fn peer_availability(&self) -> Option<&FeatureStates> {
        self.peer_availability.as_ref()
    }

    /// The peer's latest `PresenceUpdate`, once one has arrived.
    #[must_use]
    pub fn peer_presence(&self) -> Option<&PresenceUpdate> {
        self.peer_presence.as_ref()
    }

    /// Whether `feature` can be used now: it is active, and ready on this
    /// device and, by its latest `FeatureStatus`, on the peer.
    #[must_use]
    pub fn is_ready(&self, feature: Feature) -> bool {
        let ready =
            |states: &FeatureStates| states.get(&feature).is_some_and(Availability::is_ready);
        self.is_feature_active(feature)
            && ready(&self.availability)
            && self.peer_availability.as_ref().is_some_and(ready)
    }

    /// Reports a new availability for one of this device's features, and
    /// sends a complete `FeatureStatus` if it changed. Features this device
    /// did not list in its `Hello` are ignored.
    ///
    /// # Errors
    ///
    /// Fails with [`SessionError::Ended`] once the session is over, and on
    /// framing or I/O errors.
    pub async fn set_availability(
        &mut self,
        feature: Feature,
        availability: Availability,
    ) -> Result<(), SessionError> {
        if self.ended {
            return Err(SessionError::Ended);
        }
        match self.availability.get_mut(&feature) {
            Some(current) if *current != availability => *current = availability,
            _ => return Ok(()),
        }
        self.enqueue(Payload::FeatureStatus(status::status_for(
            &self.local.features,
            &self.availability,
        )))?;
        self.flush().await
    }

    /// Sends a new `PresenceUpdate` if it differs from the last one.
    ///
    /// # Errors
    ///
    /// Fails with [`SessionError::Ended`] once the session is over, and on
    /// framing or I/O errors.
    pub async fn set_presence(&mut self, presence: PresenceUpdate) -> Result<(), SessionError> {
        if self.ended {
            return Err(SessionError::Ended);
        }
        if presence == self.presence {
            return Ok(());
        }
        self.presence = presence;
        self.enqueue(Payload::PresenceUpdate(self.presence.clone()))?;
        self.flush().await
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

    /// How many bytes of frames wait to be written to the peer.
    #[must_use]
    pub fn queued_len(&self) -> usize {
        self.framed.write_buffer().len()
    }

    /// Asks for one [`SessionEvent::Drained`] once at most `len` bytes of
    /// frames wait to be written, so that a side that stopped queueing
    /// bulk data at some [`Session::queued_len`] knows when to go on. A
    /// later call replaces the mark.
    pub fn wake_when_drained(&mut self, len: usize) {
        self.drain_mark = Some(len);
    }

    /// Sends a one-way message and returns its sequence number.
    ///
    /// The message is numbered and queued before the first `await`, so if
    /// the returned future is dropped early, the message still goes out
    /// with the next write.
    ///
    /// # Errors
    ///
    /// Fails with [`SessionError::UnexpectedHello`] for a `Hello`, with
    /// [`SessionError::NotAllowed`] for a `Goodbye` (see
    /// [`Session::goodbye`]), a `FeatureStatus` or `PresenceUpdate` (see
    /// [`Session::set_availability`] and [`Session::set_presence`]), a
    /// request (see [`Session::send_request`]) or an answer (see
    /// [`Session::reply`]), with
    /// [`SessionError::FeatureNotActive`] for a message of a feature that is
    /// not active, with [`SessionError::Ended`] once the session is over,
    /// and on framing or I/O errors.
    pub async fn send(&mut self, payload: Payload) -> Result<u64, SessionError> {
        let rules = rules::of(&payload);
        match payload {
            Payload::Hello(_) => return Err(SessionError::UnexpectedHello),
            Payload::Goodbye(_) | Payload::FeatureStatus(_) | Payload::PresenceUpdate(_) => {
                return Err(SessionError::NotAllowed(rules.name));
            }
            _ if matches!(rules.kind, Kind::Reply | Kind::Request(_)) => {
                return Err(SessionError::NotAllowed(rules.name));
            }
            _ => {}
        }
        self.check_can_send(rules)?;
        let seq = self.enqueue(payload)?;
        self.flush().await?;
        Ok(seq)
    }

    /// Sends a request and waits for its first answer.
    ///
    /// Events that arrive meanwhile wait, in order, for
    /// [`Session::next_event`]. For a request answered by several messages,
    /// the first answer has [`Reply::last`] unset; get the others with
    /// [`Session::next_reply`].
    ///
    /// If the returned future is dropped after the request went out, the
    /// request still waits for its answer, which then comes out of
    /// [`Session::next_event`] as [`SessionEvent::Reply`].
    ///
    /// # Errors
    ///
    /// Fails with [`RequestError::Refused`] when the peer answers with an
    /// `ErrorReply`, with [`RequestError::TimedOut`] after
    /// [`SessionConfig::request_timeout`], with [`RequestError::Ended`] when
    /// the session ends first, and as [`Session::start_request`] does.
    pub async fn send_request(&mut self, payload: Payload) -> Result<Reply, RequestError> {
        let request = self.start_request(payload).await?;
        self.next_reply(request).await
    }

    /// Sends a request without waiting for its answer, and returns its
    /// sequence number.
    ///
    /// Its answers come out of [`Session::next_event`] as
    /// [`SessionEvent::Reply`], or [`Session::next_reply`] waits for them.
    /// Without an answer within [`SessionConfig::request_timeout`], it fails
    /// with [`SessionEvent::RequestTimedOut`]. Several requests may wait at
    /// the same time. Like [`Session::send`], the request goes out even if
    /// the returned future is dropped early.
    ///
    /// # Errors
    ///
    /// Fails with [`SessionError::NotAllowed`] when `payload` is not a
    /// request, with [`SessionError::FeatureNotActive`] for a request of a
    /// feature that is not active, with [`SessionError::FeatureNotReady`]
    /// for one that is not ready on both devices, with [`SessionError::Ended`] once the
    /// session is over, and on framing or I/O errors.
    pub async fn start_request(&mut self, payload: Payload) -> Result<u64, SessionError> {
        let seq = self.queue_request(payload)?;
        self.flush().await?;
        Ok(seq)
    }

    /// Like [`Session::start_request`], but without waiting for the request
    /// to be written: it goes out while [`Session::next_event`] waits for
    /// frames, as answers do.
    ///
    /// This is for bulk data, such as file chunks. Writing a large frame can
    /// wait for the peer to read, and a peer that writes bulk data too may
    /// only read once this side reads; so a side that waited for its own
    /// writes before reading again could wait forever. Keep
    /// [`Session::queued_len`] low: the session stops reading while more
    /// than 1 MiB waits to be written.
    ///
    /// # Errors
    ///
    /// As for [`Session::start_request`], except that writing happens later.
    pub fn queue_request(&mut self, payload: Payload) -> Result<u64, SessionError> {
        let rules = rules::of(&payload);
        let Kind::Request(expects) = rules.kind else {
            return Err(SessionError::NotAllowed(rules.name));
        };
        self.check_can_send(rules)?;
        if let Some(feature) = rules.feature
            && !self.is_ready(feature)
        {
            return Err(SessionError::FeatureNotReady(feature));
        }
        let seq = self.enqueue_envelope(Envelope {
            expects_reply: true,
            payload: Some(payload),
            ..Envelope::default()
        })?;
        let deadline = Instant::now() + self.request_timeout;
        self.pending.insert(seq, Pending { expects, deadline });
        Ok(seq)
    }

    /// Waits for the next answer to this side's request `request`, sent
    /// with [`Session::start_request`] or [`Session::send_request`].
    ///
    /// Events that arrive meanwhile wait, in order, for
    /// [`Session::next_event`]. This method is cancel safe.
    ///
    /// # Errors
    ///
    /// Fails with [`RequestError::Refused`] when the answer is an
    /// `ErrorReply`, with [`RequestError::TimedOut`] when none arrives in
    /// time, with [`RequestError::Ended`] when the session ends first, with
    /// [`RequestError::UnknownRequest`] when `request` does not wait for an
    /// answer, and with [`RequestError::Session`] when the session fails.
    pub async fn next_reply(&mut self, request: u64) -> Result<Reply, RequestError> {
        loop {
            let found = self.events.iter().position(|event| match event {
                SessionEvent::Reply(reply) => reply.request == request,
                SessionEvent::RequestTimedOut { request: timed_out } => *timed_out == request,
                _ => false,
            });
            if let Some(index) = found {
                return match self.events.remove(index) {
                    Some(SessionEvent::Reply(reply)) => match reply.payload {
                        Payload::ErrorReply(error) => Err(RequestError::Refused(error)),
                        _ => Ok(reply),
                    },
                    _ => Err(RequestError::TimedOut(self.request_timeout)),
                };
            }
            if self.ended {
                return Err(RequestError::Ended);
            }
            if !self.pending.contains_key(&request) {
                return Err(RequestError::UnknownRequest(request));
            }
            if self.events.len() >= MAX_WAITING_EVENTS {
                let detail = format!(
                    "more than {MAX_WAITING_EVENTS} messages arrived while waiting for an answer"
                );
                return Err(self.fail(SessionError::Protocol(detail)).into());
            }
            self.step().await?;
        }
    }

    /// Answers the peer's request with sequence number `request` and
    /// returns the answer's own sequence number.
    ///
    /// `payload` is `Ack`, `ErrorReply`, or the reply message the request
    /// names. A request answered by several messages gets one call per
    /// message, the last one saying so (for example
    /// `NotificationSyncBatch.more` unset). An `ErrorReply.detail` longer
    /// than 1 KiB is shortened. Like [`Session::send`], the answer goes out
    /// even if the returned future is dropped early.
    ///
    /// # Errors
    ///
    /// Fails with [`SessionError::NotAllowed`] when `payload` is not an
    /// answer, with [`SessionError::NoSuchRequest`] when the peer has not
    /// sent `request` yet, with [`SessionError::Ended`] once the session is
    /// over, and on framing or I/O errors.
    pub async fn reply(&mut self, request: u64, payload: Payload) -> Result<u64, SessionError> {
        let seq = self.queue_reply(request, payload)?;
        self.flush().await?;
        Ok(seq)
    }

    /// Like [`Session::reply`], but without waiting for the answer to be
    /// written: it goes out while [`Session::next_event`] waits for frames.
    /// See [`Session::queue_request`] for why.
    ///
    /// # Errors
    ///
    /// As for [`Session::reply`], except that writing happens later.
    pub fn queue_reply(&mut self, request: u64, mut payload: Payload) -> Result<u64, SessionError> {
        let rules = rules::of(&payload);
        if rules.kind != Kind::Reply {
            return Err(SessionError::NotAllowed(rules.name));
        }
        if request == 0 || request > self.last_peer_seq {
            return Err(SessionError::NoSuchRequest(request));
        }
        if self.ended {
            return Err(SessionError::Ended);
        }
        if let Payload::ErrorReply(error) = &mut payload {
            truncate_detail(&mut error.detail);
        }
        self.enqueue_envelope(Envelope {
            reply_to: request,
            payload: Some(payload),
            ..Envelope::default()
        })
    }

    /// Answers the peer's request `request` with `Ack`. See
    /// [`Session::reply`].
    ///
    /// # Errors
    ///
    /// As for [`Session::reply`].
    pub async fn ack(&mut self, request: u64) -> Result<u64, SessionError> {
        self.reply(request, Payload::Ack(Ack {})).await
    }

    /// Answers the peer's request `request` with an `ErrorReply`. See
    /// [`Session::reply`].
    ///
    /// # Errors
    ///
    /// As for [`Session::reply`].
    pub async fn reply_error(
        &mut self,
        request: u64,
        error: ErrorReply,
    ) -> Result<u64, SessionError> {
        self.reply(request, Payload::ErrorReply(error)).await
    }

    /// Checks that a message with `rules` may go out now.
    fn check_can_send(&self, rules: Rules) -> Result<(), SessionError> {
        if self.ended {
            return Err(SessionError::Ended);
        }
        // Pairing messages only while pairing, and feature traffic only
        // after it.
        let allowed = match rules.kind {
            Kind::Pairing => self.pairing,
            Kind::Request(_) | Kind::Event => !self.pairing,
            Kind::Control | Kind::Reply => true,
        };
        if !allowed {
            return Err(SessionError::NotAllowed(rules.name));
        }
        match rules.feature {
            Some(feature) if !self.is_feature_active(feature) => {
                Err(SessionError::FeatureNotActive(feature))
            }
            _ => Ok(()),
        }
    }

    /// Says whether the session carries activity the user can see, such
    /// as a focused window or a running transfer or call. A pinging side
    /// then pings on the active interval of its [`PingSchedule`], starting
    /// with a `Ping` right away, and otherwise on the idle one. Sessions
    /// start idle.
    ///
    /// # Errors
    ///
    /// Fails with [`SessionError::Ended`] once the session is over, and on
    /// framing or I/O errors.
    pub async fn set_active(&mut self, active: bool) -> Result<(), SessionError> {
        if self.ended {
            return Err(SessionError::Ended);
        }
        if self.liveness.set_active(active, Instant::now()) {
            self.queue_ping()?;
            self.flush().await?;
        }
        Ok(())
    }

    /// Whether the session carries activity the user can see, as last set
    /// with [`Session::set_active`].
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.liveness.is_active()
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
    /// Fails with [`SessionError::PeerSilent`] when the peer goes silent,
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
        let silence_deadline = self.liveness.deadline();
        let request_deadline = self.pending.values().map(|pending| pending.deadline).min();
        let drain_mark = self.drain_mark;
        tokio::select! {
            io = poll_fn(|cx| poll_io(&mut self.framed, &mut self.unflushed, drain_mark, cx)) => match io {
                Io::Drained => {
                    self.drain_mark = None;
                    self.events.push_back(SessionEvent::Drained);
                    Ok(())
                }
                Io::Frame(envelope) => {
                    self.liveness.received(Instant::now());
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
            () = self.liveness.next_ping() => self.queue_ping(),
            () = time::sleep_until(silence_deadline) => {
                Err(self.fail(SessionError::PeerSilent(self.liveness.limit())))
            }
            () = sleep_until(request_deadline) => {
                self.expire_requests();
                Ok(())
            }
        }
    }

    /// Fails every request whose time for an answer is up.
    fn expire_requests(&mut self) {
        let now = Instant::now();
        let events = &mut self.events;
        self.pending.retain(|&request, pending| {
            let expired = pending.deadline <= now;
            if expired {
                debug!(request, "a request got no answer in time");
                events.push_back(SessionEvent::RequestTimedOut { request });
            }
            !expired
        });
    }

    /// Handles one incoming envelope, queueing what the caller should see.
    fn handle(&mut self, envelope: Envelope) -> Result<(), SessionError> {
        check_seq(self.last_peer_seq, envelope.seq)?;
        let Envelope {
            seq,
            reply_to,
            expects_reply,
            payload,
        } = envelope;
        self.last_peer_seq = seq;
        if reply_to != 0 && expects_reply {
            return Err(SessionError::Protocol(format!(
                "message {seq} is an answer that expects a reply"
            )));
        }
        let Some(payload) = payload else {
            // A message type added by a newer version.
            if expects_reply {
                return self.queue_error_reply(seq, ErrorCode::Unsupported, "unknown request");
            }
            debug!(seq, "ignoring a message this build does not know");
            return Ok(());
        };
        let rules = rules::of(&payload);
        // The peer is pairing until its FeatureStatus arrives, which it
        // sends once it has stored the pairing.
        let peer_pairing = self.pairing_method.is_some() && self.peer_availability.is_none();
        match rules.kind {
            Kind::Pairing if !peer_pairing => {
                return Err(SessionError::Protocol(format!(
                    "{} outside the pairing exchange",
                    rules.name
                )));
            }
            Kind::Request(_) | Kind::Event if peer_pairing => {
                return Err(SessionError::Protocol(format!(
                    "{} during the pairing exchange",
                    rules.name
                )));
            }
            _ => {}
        }
        match (rules.kind, expects_reply) {
            _ if reply_to != 0 => self.on_answer(reply_to, seq, payload, rules),
            (Kind::Reply, true) => Err(SessionError::Protocol(format!(
                "{} {seq} expects a reply",
                rules.name
            ))),
            (Kind::Request(_), true) => self.on_request(seq, payload, rules),
            (_, true) => {
                // Not a request, so the peer waits for an answer it cannot
                // get. Say so, then take the message as it is.
                let detail = format!("{} is not a request", rules.name);
                self.queue_error_reply(seq, ErrorCode::Unsupported, &detail)?;
                self.on_one_way(seq, payload, rules)
            }
            (_, false) => self.on_one_way(seq, payload, rules),
        }
    }

    /// Handles an envelope with `reply_to` set.
    fn on_answer(
        &mut self,
        request: u64,
        seq: u64,
        payload: Payload,
        rules: Rules,
    ) -> Result<(), SessionError> {
        if rules.kind != Kind::Reply {
            return Err(SessionError::Protocol(format!(
                "{} {seq} answers message {request}, but it is not an answer",
                rules.name
            )));
        }
        let Some(pending) = self.pending.get_mut(&request) else {
            // Most likely the request timed out.
            debug!(request, seq, "ignoring an answer nobody waits for");
            return Ok(());
        };
        if !pending.expects.allows(&payload) {
            return Err(SessionError::Protocol(format!(
                "{} {seq} cannot answer request {request}",
                rules.name
            )));
        }
        let last = rules::is_last_answer(&payload);
        if last {
            self.pending.remove(&request);
        } else {
            pending.deadline = Instant::now() + self.request_timeout;
        }
        self.events.push_back(SessionEvent::Reply(Reply {
            request,
            seq,
            payload,
            last,
        }));
        Ok(())
    }

    /// Handles a request from the peer.
    fn on_request(&mut self, seq: u64, payload: Payload, rules: Rules) -> Result<(), SessionError> {
        if let Some(feature) = rules.feature
            && !self.is_feature_active(feature)
        {
            let detail = format!("{feature} is not active in this session");
            return self.queue_error_reply(seq, ErrorCode::Unsupported, &detail);
        }
        // A feature this side reported as turned off or waiting for a
        // permission is refused with the matching code.
        let refusal = match rules.feature.and_then(|f| self.availability.get(&f)) {
            Some(Availability::Disabled) => Some((ErrorCode::Disabled, String::new())),
            Some(Availability::PermissionRequired(permission)) => {
                Some((ErrorCode::PermissionRequired, permission.clone()))
            }
            _ => None,
        };
        if let Some((code, permission)) = refusal {
            debug!(
                seq,
                ?code,
                "refusing a request of a feature that is not ready"
            );
            let error = ErrorReply {
                code: code.into(),
                detail: format!("{} is not ready on this device", rules.name),
                permission,
                ..ErrorReply::default()
            };
            self.enqueue_envelope(Envelope {
                reply_to: seq,
                payload: Some(Payload::ErrorReply(error)),
                ..Envelope::default()
            })?;
            return Ok(());
        }
        self.events
            .push_back(SessionEvent::Request { seq, payload });
        Ok(())
    }

    /// Handles a message that wants no answer.
    fn on_one_way(&mut self, seq: u64, payload: Payload, rules: Rules) -> Result<(), SessionError> {
        match payload {
            Payload::Hello(_) => return Err(SessionError::UnexpectedHello),
            Payload::Ping(Ping { nonce }) => {
                self.enqueue(Payload::Pong(Pong { nonce }))?;
            }
            Payload::Pong(Pong { nonce }) => match self.pending_ping {
                Some((pending, sent_at)) if pending == nonce => {
                    self.pending_ping = None;
                    let rtt = sent_at.elapsed();
                    self.rtt = Some(rtt);
                    self.events.push_back(SessionEvent::Pong { rtt });
                }
                _ => debug!(nonce, "ignoring a Pong for a superseded Ping"),
            },
            Payload::Goodbye(goodbye) => {
                let mut goodbye = PeerGoodbye::from(goodbye);
                goodbye.error = self.peer_error.take();
                debug!(reason = ?goodbye.reason, goodbye.detail, "the peer ended the session");
                self.ended = true;
                self.events.push_back(SessionEvent::Goodbye(goodbye));
            }
            Payload::FeatureStatus(status) => {
                let states = status::states_from(status, &self.peer.features);
                self.peer_availability = Some(states.clone());
                self.events.push_back(SessionEvent::FeatureStatus(states));
            }
            Payload::PresenceUpdate(presence) => {
                self.peer_presence = Some(presence.clone());
                self.events.push_back(SessionEvent::Presence(presence));
            }
            // An ErrorReply that answers nothing explains a coming Goodbye.
            Payload::ErrorReply(error) => {
                debug!(code = ?error.code(), error.detail, "the peer reported an error");
                self.peer_error = Some(error);
            }
            _ if rules.kind == Kind::Reply => {
                debug!(seq, rules.name, "ignoring an answer to nothing");
            }
            _ if rules
                .feature
                .is_some_and(|feature| !self.is_feature_active(feature)) =>
            {
                debug!(seq, rules.name, "ignoring a message of an inactive feature");
            }
            // Everything else is for the layers above the session.
            payload => self
                .events
                .push_back(SessionEvent::Message { seq, payload }),
        }
        Ok(())
    }

    /// Whether `feature` is active: both devices listed it in their Hello.
    fn is_feature_active(&self, feature: Feature) -> bool {
        self.local.features.contains(&feature) && self.peer.features.contains(&feature)
    }

    /// Queues an `ErrorReply` to the peer's request `request`.
    fn queue_error_reply(
        &mut self,
        request: u64,
        code: ErrorCode,
        detail: &str,
    ) -> Result<(), SessionError> {
        debug!(request, ?code, detail, "refusing a request");
        let error = ErrorReply {
            code: code.into(),
            detail: detail.to_owned(),
            ..ErrorReply::default()
        };
        self.enqueue_envelope(Envelope {
            reply_to: request,
            payload: Some(Payload::ErrorReply(error)),
            ..Envelope::default()
        })?;
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
        self.enqueue_envelope(Envelope {
            payload: Some(payload),
            ..Envelope::default()
        })
    }

    /// Like [`Session::enqueue`], for an envelope with `reply_to` or
    /// `expects_reply` set. Its `seq` is overwritten.
    fn enqueue_envelope(&mut self, mut envelope: Envelope) -> Result<u64, SessionError> {
        let seq = self.next_seq;
        envelope.seq = seq;
        let mut codec = *self.framed.codec();
        codec.encode(&envelope, self.framed.write_buffer_mut())?;
        self.next_seq += 1;
        self.unflushed = true;
        Ok(seq)
    }

    /// Starts the session proper: lifts the small frame limit of the
    /// handshake, now that the peer is paired, and queues this side's
    /// `FeatureStatus` and `PresenceUpdate`.
    fn start(&mut self) -> Result<(), SessionError> {
        *self.framed.codec_mut() = EnvelopeCodec::with_max_frame_len(self.max_frame_len);
        let status = status::status_for(&self.local.features, &self.availability);
        self.enqueue(Payload::FeatureStatus(status))?;
        self.enqueue(Payload::PresenceUpdate(self.presence.clone()))?;
        Ok(())
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

/// How much this side queues for the peer before it stops reading from it,
/// in bytes. The answers the session sends by itself, such as a `Pong` or
/// an `ErrorReply` to an unknown request, then wait for the peer to read,
/// instead of piling up behind a peer that sends but never reads.
const MAX_QUEUED_LEN: usize = 1024 * 1024;

/// How many events [`Session::next_reply`] keeps for
/// [`Session::next_event`] while it waits for an answer.
const MAX_WAITING_EVENTS: usize = 1024;

/// Sleeps until `deadline`, or forever without one.
async fn sleep_until(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => time::sleep_until(deadline).await,
        None => std::future::pending().await,
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
    /// At most the drain mark's bytes wait to be written.
    Drained,
}

/// Writes queued frames while waiting for the next incoming one, so a slow
/// write never stops the session from reading, unless more than
/// [`MAX_QUEUED_LEN`] bytes wait to be written. Reports when at most
/// `drain_mark` bytes wait.
fn poll_io<S>(
    framed: &mut Framed<S, EnvelopeCodec>,
    unflushed: &mut bool,
    drain_mark: Option<usize>,
    cx: &mut Context<'_>,
) -> Poll<Io>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    if let Poll::Ready(Err(err)) = poll_flush(framed, unflushed, cx) {
        return Poll::Ready(Io::WriteFailed(err));
    }
    if drain_mark.is_some_and(|mark| framed.write_buffer().len() <= mark) {
        return Poll::Ready(Io::Drained);
    }
    // The flush is pending, so it wakes this task once the peer reads.
    if framed.write_buffer().len() > MAX_QUEUED_LEN {
        return Poll::Pending;
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
/// version both sides use, the envelope's sequence number and the pairing
/// method its `Hello` names.
fn accept_hello(
    first: Envelope,
    expected_peer_id: Option<&str>,
) -> Result<(DeviceInfo, u32, u64, Option<PairingMethod>), SessionError> {
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
    if let Some(expected) = expected_peer_id
        && hello.device_id != expected
    {
        return Err(SessionError::UnexpectedPeer {
            expected: expected.to_owned(),
            received: hello.device_id,
        });
    }
    let pairing = match hello.pairing_method() {
        PairingMethod::Unspecified => None,
        method => Some(method),
    };
    Ok((peer_from(hello)?, version, first.seq, pairing))
}

/// Validates the peer's `Hello`, apart from its versions, and turns it into
/// a [`DeviceInfo`].
fn peer_from(hello: Hello) -> Result<DeviceInfo, SessionError> {
    if !is_device_id(&hello.device_id) {
        return Err(SessionError::InvalidHello(
            "device_id is not 1 to 64 ASCII letters, digits or hyphens",
        ));
    }
    // The frame's limits already allow at most 64 of them.
    if hello
        .features
        .iter()
        .any(|name| name.len() > MAX_FEATURE_LEN)
    {
        return Err(SessionError::InvalidHello(
            "a feature token is longer than 32 bytes",
        ));
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
        name: clean_name(&hello.device_name),
        kind,
        features,
    })
}

/// The longest device id, in bytes.
const MAX_DEVICE_ID_LEN: usize = 64;
/// The longest device name, in bytes.
const MAX_DEVICE_NAME_LEN: usize = 64;
/// The longest capability token, in bytes.
const MAX_FEATURE_LEN: usize = 32;

/// Whether `id` can be a device id: 1 to 64 ASCII letters, digits and
/// hyphens. That takes in a UUID, and the `desktop-` ids of the desktop app
/// up to 0.1.1.
fn is_device_id(id: &str) -> bool {
    (1..=MAX_DEVICE_ID_LEN).contains(&id.len())
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

/// `name` without control and bidirectional formatting characters, cut to
/// [`MAX_DEVICE_NAME_LEN`] bytes at a character boundary.
fn clean_name(name: &str) -> String {
    let mut clean = String::new();
    for c in name.chars().filter(|&c| !is_hidden(c)) {
        if clean.len() + c.len_utf8() > MAX_DEVICE_NAME_LEN {
            break;
        }
        clean.push(c);
    }
    clean
}

/// Whether `c` is a control character or a formatting character that
/// changes the direction of text.
fn is_hidden(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'
        )
}

/// The longest `ErrorReply.detail` the protocol allows, in bytes.
const MAX_DETAIL_LEN: usize = 1024;

/// Shortens `detail` to [`MAX_DETAIL_LEN`] bytes, at a character boundary.
fn truncate_detail(detail: &mut String) {
    if detail.len() > MAX_DETAIL_LEN {
        let mut end = MAX_DETAIL_LEN;
        while !detail.is_char_boundary(end) {
            end -= 1;
        }
        detail.truncate(end);
    }
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
