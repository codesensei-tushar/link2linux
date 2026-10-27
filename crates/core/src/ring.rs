//! Making the phone ring, so the user finds it (`docs/protocol.md`,
//! "Ring").
//!
//! The desktop only asks: [`RingService::ring`] sends `RingRequest`, and
//! [`RingService::stop`] sends `RingStop`. The phone rings for a minute and
//! says when it stops with `RingEnded`. The desktop shows it ringing from
//! the phone's `Ack` until then, until the session ends, or until the minute
//! is over, whichever comes first. A phone that loses the session keeps
//! ringing: that is when the user looks for it.

use link2linux_protocol::Payload;
use link2linux_protocol::v1::{ErrorCode, ErrorReply, RingEndReason, RingRequest, RingStop};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::time::{Duration, Instant};

use crate::Feature;
use crate::session::{Availability, Session, SessionError, SessionEvent};

/// How long the phone rings for one request.
pub const RING_FOR: Duration = Duration::from_secs(60);

/// Where ringing stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RingPhase {
    /// The phone does not ring, as far as the desktop knows.
    Idle,
    /// The desktop asked the phone to ring and waits for its answer.
    Starting,
    /// The phone rings.
    Ringing,
    /// The desktop asked the phone to stop and waits for its answer.
    Stopping,
}

/// Why the phone cannot ring, or did not do what the desktop asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RingFailure {
    /// The user turned ringing off on the phone.
    TurnedOff,
    /// The phone cannot ring right now, for example during a call.
    Unavailable,
    /// The phone's app cannot ring: it is older than this desktop's.
    Unsupported,
    /// No session with the phone is up.
    NotConnected,
    /// The phone refused, or gave no answer in time.
    Failed,
}

/// Why the ringing ended, as the phone said.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RingEnd {
    /// The user stopped it on the phone.
    StoppedOnPhone,
    /// A desktop stopped it: this one or another one.
    StoppedFromDesktop,
    /// The phone rang for its full minute.
    TimedOut,
    /// The phone could not keep ringing, for example because a call
    /// started.
    Interrupted,
}

impl RingEnd {
    fn from_wire(reason: RingEndReason) -> Option<Self> {
        match reason {
            RingEndReason::Unspecified => None,
            RingEndReason::StoppedOnPhone => Some(Self::StoppedOnPhone),
            RingEndReason::StoppedFromDesktop => Some(Self::StoppedFromDesktop),
            RingEndReason::TimedOut => Some(Self::TimedOut),
            RingEndReason::Interrupted => Some(Self::Interrupted),
        }
    }
}

/// What to show of the ringing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RingState {
    /// Why the phone cannot ring now, or `None` when it can.
    pub blocked: Option<RingFailure>,
    /// Where ringing stands.
    pub phase: RingPhase,
    /// Why the last ring or stop the user asked for did not work. The next
    /// one clears it.
    pub failure: Option<RingFailure>,
    /// Why the phone stopped ringing by itself or on its user's word. The
    /// next ring clears it.
    pub ended: Option<RingEnd>,
}

/// The ring feature with one phone, driven by its session. It does not own
/// the session, so it can share one with other features:
///
/// 1. [`RingService::session_started`] once the session is up;
/// 2. every [`SessionEvent`] through [`RingService::handle_event`], which
///    keeps the ring events and hands back the rest;
/// 3. [`RingService::ring`] and [`RingService::stop`] when the user asks,
///    or [`RingService::not_connected`] when the user asks while no
///    session is up;
/// 4. [`RingService::tick`] at [`RingService::next_deadline`];
/// 5. [`RingService::session_ended`] when the session is gone.
///
/// [`RingService::state`] says what to show after each of them.
#[derive(Debug)]
pub struct RingService {
    blocked: Option<RingFailure>,
    failure: Option<RingFailure>,
    ended: Option<RingEnd>,
    /// The `RingRequest` that waits for its answer.
    ringing_asked: Option<u64>,
    /// The `RingStop` that waits for its answer.
    stop_asked: Option<u64>,
    /// While the phone rings: when the desktop stops showing it, if the
    /// phone has not said so before.
    until: Option<Instant>,
}

impl Default for RingService {
    fn default() -> Self {
        Self::new()
    }
}

impl RingService {
    /// A service with no session yet.
    #[must_use]
    pub fn new() -> Self {
        Self {
            blocked: Some(RingFailure::NotConnected),
            failure: None,
            ended: None,
            ringing_asked: None,
            stop_asked: None,
            until: None,
        }
    }

    /// What to show now.
    #[must_use]
    pub fn state(&self) -> RingState {
        let phase = if self.stop_asked.is_some() {
            RingPhase::Stopping
        } else if self.until.is_some() {
            RingPhase::Ringing
        } else if self.ringing_asked.is_some() {
            RingPhase::Starting
        } else {
            RingPhase::Idle
        };
        RingState {
            blocked: self.blocked,
            phase,
            failure: self.failure,
            ended: self.ended,
        }
    }

    /// Notes whether the phone can ring. Call it once the session is up.
    pub fn session_started<S>(&mut self, session: &Session<S>)
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        self.blocked = blocked(session);
        self.failure = None;
    }

    /// Forgets what waits for an answer, and stops showing the phone as
    /// ringing: without a session, the desktop cannot know.
    pub fn session_ended(&mut self) {
        self.blocked = Some(RingFailure::NotConnected);
        self.ringing_asked = None;
        self.stop_asked = None;
        self.until = None;
        self.ended = None;
    }

    /// Asks the phone to ring, or to ring for another full minute if it
    /// already does. A phone that cannot ring is not asked: the state says
    /// why.
    ///
    /// # Errors
    ///
    /// Fails when the session fails.
    pub async fn ring<S>(&mut self, session: &mut Session<S>) -> Result<(), SessionError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        self.failure = None;
        self.ended = None;
        self.blocked = blocked(session);
        if let Some(reason) = self.blocked {
            self.failure = Some(reason);
            return Ok(());
        }
        if self.ringing_asked.is_some() {
            return Ok(());
        }
        let seq = session
            .start_request(Payload::RingRequest(RingRequest {}))
            .await?;
        tracing::debug!(seq, "asked the phone to ring");
        self.ringing_asked = Some(seq);
        Ok(())
    }

    /// Asks the phone to stop ringing. Nothing happens unless the desktop
    /// shows it ringing or about to.
    ///
    /// # Errors
    ///
    /// Fails when the session fails.
    pub async fn stop<S>(&mut self, session: &mut Session<S>) -> Result<(), SessionError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        self.failure = None;
        if matches!(self.state().phase, RingPhase::Idle | RingPhase::Stopping) {
            return Ok(());
        }
        self.blocked = blocked(session);
        if let Some(reason) = self.blocked {
            self.failure = Some(reason);
            return Ok(());
        }
        let seq = session
            .start_request(Payload::RingStop(RingStop {}))
            .await?;
        tracing::debug!(seq, "asked the phone to stop ringing");
        self.stop_asked = Some(seq);
        Ok(())
    }

    /// The user asked for a ring while no session is up.
    pub fn not_connected(&mut self) {
        self.blocked = Some(RingFailure::NotConnected);
        self.failure = Some(RingFailure::NotConnected);
        self.ended = None;
    }

    /// Takes the events of the ring feature and returns every other event,
    /// for the caller to handle. A `FeatureStatus` is used and returned too.
    pub fn handle_event<S>(
        &mut self,
        session: &Session<S>,
        event: SessionEvent,
    ) -> Option<SessionEvent>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        match event {
            SessionEvent::Reply(reply) if self.ringing_asked == Some(reply.request) => {
                self.ringing_asked = None;
                match reply.into_result() {
                    // A ring sent before a stop that waits ends with it.
                    Ok(_) if self.stop_asked.is_none() => {
                        self.until = Some(Instant::now() + RING_FOR);
                    }
                    Ok(_) => {}
                    Err(error) => self.refused(&error, "ring"),
                }
                None
            }
            SessionEvent::Reply(reply) if self.stop_asked == Some(reply.request) => {
                self.stop_asked = None;
                match reply.into_result() {
                    Ok(_) => self.until = None,
                    // The phone may still ring.
                    Err(error) => self.refused(&error, "stop ringing"),
                }
                None
            }
            SessionEvent::RequestTimedOut { request } if self.ringing_asked == Some(request) => {
                self.ringing_asked = None;
                tracing::info!("the phone did not answer the ring in time");
                self.failure = Some(RingFailure::Failed);
                None
            }
            SessionEvent::RequestTimedOut { request } if self.stop_asked == Some(request) => {
                self.stop_asked = None;
                tracing::info!("the phone did not answer the stop in time");
                self.failure = Some(RingFailure::Failed);
                None
            }
            SessionEvent::Message {
                payload: Payload::RingEnded(ended),
                ..
            } => {
                // The end of the ringing that a stop asked for needs no
                // word, and a ring that waits starts it again.
                if self.until.take().is_some() && self.stop_asked.is_none() {
                    self.ended = RingEnd::from_wire(ended.reason());
                }
                tracing::debug!(reason = ended.reason, "the phone stopped ringing");
                None
            }
            SessionEvent::FeatureStatus(states) => {
                self.blocked = blocked(session);
                Some(SessionEvent::FeatureStatus(states))
            }
            other => Some(other),
        }
    }

    /// When the desktop stops showing the phone as ringing if it has not
    /// heard otherwise.
    #[must_use]
    pub fn next_deadline(&self) -> Option<Instant> {
        self.until
    }

    /// The time [`RingService::next_deadline`] named may have come: a minute
    /// after the phone started ringing, it no longer counts as ringing.
    pub fn tick(&mut self, now: Instant) {
        if self.until.is_some_and(|until| until <= now) {
            tracing::debug!("the phone rang for its minute");
            self.until = None;
        }
    }

    fn refused(&mut self, error: &ErrorReply, what: &str) {
        tracing::info!(code = error.code, "the phone refused to {what}");
        self.failure = Some(match error.code() {
            ErrorCode::Disabled => RingFailure::TurnedOff,
            ErrorCode::Unavailable | ErrorCode::Busy | ErrorCode::PermissionRequired => {
                RingFailure::Unavailable
            }
            ErrorCode::Unsupported => RingFailure::Unsupported,
            _ => RingFailure::Failed,
        });
    }
}

/// Why the phone cannot ring in `session`, or `None` when it can.
fn blocked<S>(session: &Session<S>) -> Option<RingFailure>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    if session.is_ready(Feature::Ring) {
        return None;
    }
    if !session.shared_features().contains(&Feature::Ring) {
        return Some(RingFailure::Unsupported);
    }
    let phone = session
        .peer_availability()
        .and_then(|states| states.get(&Feature::Ring));
    Some(match phone {
        Some(Availability::Disabled) => RingFailure::TurnedOff,
        _ => RingFailure::Unavailable,
    })
}
