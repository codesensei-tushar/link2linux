//! One phone's part of the clipboard feature, driven by its session.

use std::sync::Arc;

use link2linux_platform::{Clipboard, ClipboardAccess, ClipboardText};
use link2linux_protocol::Payload;
use link2linux_protocol::v1::{ClipboardPush, ErrorCode, ErrorReply};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::time::Instant;

use super::hub::ClipboardHub;
use super::{Clip, ClipboardEvent, Digest, MAX_TEXT_BYTES, Origin, PhoneClipboard, PushFailure};
use crate::Feature;
use crate::session::{Availability, Session, SessionError, SessionEvent};

/// The clipboard feature with one phone, between its session, the desktop
/// clipboard and the [`ClipboardHub`]. It does not own the session, so it
/// can share one with other features:
///
/// 1. [`ClipboardService::session_started`] once the session is up;
/// 2. every [`SessionEvent`] through [`ClipboardService::handle_event`],
///    which keeps the clipboard events and hands back the rest;
/// 3. each copy the hub returns through [`ClipboardService::share`];
/// 4. [`ClipboardService::sharing_changed`] when the user turns sharing on
///    or off;
/// 5. [`ClipboardService::session_ended`] when the session is gone.
///
/// The service stays with its phone across sessions, so it remembers what
/// the phone already has.
pub struct ClipboardService<C> {
    device_id: String,
    hub: ClipboardHub,
    clipboard: C,
    /// The push that waits for its answer: its sequence number and text.
    in_flight: Option<(u64, Arc<Clip>)>,
    /// The newest copy made while a push waits, sent once it is answered.
    queued: Option<(Arc<Clip>, Origin)>,
    /// The text the phone holds as far as the desktop knows: the text it
    /// pushed here or was pushed from here, whichever came last, and
    /// nothing once a push to it failed. It does not go to the phone again
    /// by itself, nor as a recent copy when a session starts.
    holds: Option<Digest>,
    /// The phone's state as last reported.
    state: Option<PhoneClipboard>,
}

impl<C: Clipboard> ClipboardService<C> {
    /// The service for phone `device_id`, which writes the phone's pushes
    /// to `clipboard`.
    pub fn new(device_id: impl Into<String>, hub: ClipboardHub, clipboard: C) -> Self {
        Self {
            device_id: device_id.into(),
            hub,
            clipboard,
            in_flight: None,
            queued: None,
            holds: None,
            state: None,
        }
    }

    /// Reports the phone's state, and pushes a copy the user made less
    /// than [`RECENT_COPY`](super::RECENT_COPY) ago that the phone does
    /// not have yet. Call it once the session is up.
    ///
    /// # Errors
    ///
    /// Fails when the session fails.
    pub async fn session_started<S>(&mut self, session: &mut Session<S>) -> Result<(), SessionError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        self.report(session);
        self.push_recent(session).await
    }

    /// Forgets the push that waits and the copy queued behind it: a push
    /// that fails is not sent again, because a late paste of old text
    /// would surprise the user more than a lost one.
    pub fn session_ended(&mut self) {
        if let Some((_, clip)) = self.in_flight.take() {
            tracing::debug!(bytes = clip.len(), "a clipboard push got no answer");
            self.failed(&clip, PushFailure::Failed);
        }
        self.queued = None;
        self.set_state(PhoneClipboard::Disconnected);
    }

    /// Takes the events of the clipboard feature and returns every other
    /// event, for the caller to handle. A `FeatureStatus` is used and
    /// returned too.
    ///
    /// # Errors
    ///
    /// Fails when the session fails.
    pub async fn handle_event<S>(
        &mut self,
        session: &mut Session<S>,
        event: SessionEvent,
    ) -> Result<Option<SessionEvent>, SessionError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        match event {
            SessionEvent::Request {
                seq,
                payload: Payload::ClipboardPush(push),
            } => {
                self.receive(session, seq, push).await?;
                Ok(None)
            }
            SessionEvent::Reply(reply) if self.is_in_flight(reply.request) => {
                self.answered(reply.into_result().err());
                self.send_queued(session).await?;
                Ok(None)
            }
            SessionEvent::RequestTimedOut { request } if self.is_in_flight(request) => {
                self.answered(Some(ErrorReply {
                    code: ErrorCode::Internal.into(),
                    detail: "no answer in time".into(),
                    ..ErrorReply::default()
                }));
                self.send_queued(session).await?;
                Ok(None)
            }
            SessionEvent::FeatureStatus(status) => {
                let was_ready = self.state == Some(PhoneClipboard::Ready);
                self.report(session);
                if !session.is_ready(Feature::Clipboard) {
                    self.queued = None;
                } else if !was_ready {
                    self.push_recent(session).await?;
                }
                Ok(Some(SessionEvent::FeatureStatus(status)))
            }
            other => Ok(Some(other)),
        }
    }

    /// Pushes `clip` to the phone, if clipboard sharing is ready on both
    /// devices. An [`Origin::Automatic`] copy does not go to a phone that
    /// pushed the same text here or already got it. While a push waits for
    /// its answer, the copy waits instead, replacing any older one.
    ///
    /// # Errors
    ///
    /// Fails when the session fails.
    pub async fn share<S>(
        &mut self,
        session: &mut Session<S>,
        clip: Arc<Clip>,
        origin: Origin,
    ) -> Result<(), SessionError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        if !session.is_ready(Feature::Clipboard) {
            return Ok(());
        }
        if origin == Origin::Automatic && self.has(&clip) {
            tracing::debug!("not pushing text the phone already has");
            // The newest copy is the phone's already: an older one that
            // waits would only replace it.
            self.queued = None;
            return Ok(());
        }
        if self.in_flight.is_some() {
            self.queued = Some((clip, origin));
            return Ok(());
        }
        self.push(session, clip).await
    }

    /// The user turned clipboard sharing on or off on this desktop: tells
    /// the phone with a new `FeatureStatus`.
    ///
    /// # Errors
    ///
    /// Fails when the session fails.
    pub async fn sharing_changed<S>(&mut self, session: &mut Session<S>) -> Result<(), SessionError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let availability = if self.hub.settings().sharing {
            Availability::Ready
        } else {
            self.queued = None;
            Availability::Disabled
        };
        session
            .set_availability(Feature::Clipboard, availability)
            .await
    }

    fn is_in_flight(&self, request: u64) -> bool {
        self.in_flight
            .as_ref()
            .is_some_and(|(seq, _)| *seq == request)
    }

    /// Whether the phone holds this text, as far as the desktop knows.
    fn has(&self, clip: &Clip) -> bool {
        self.holds == Some(clip.digest())
    }

    async fn push<S>(
        &mut self,
        session: &mut Session<S>,
        clip: Arc<Clip>,
    ) -> Result<(), SessionError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let push = ClipboardPush {
            text: clip.text().text.clone(),
            sensitive: clip.is_sensitive(),
        };
        let seq = session.start_request(Payload::ClipboardPush(push)).await?;
        tracing::debug!(seq, bytes = clip.len(), "pushed the clipboard");
        self.holds = Some(clip.digest());
        self.in_flight = Some((seq, clip));
        Ok(())
    }

    /// Sends the copy that waited for the answer, checked again: the phone
    /// may have turned sharing off, or got the same text meanwhile.
    async fn send_queued<S>(&mut self, session: &mut Session<S>) -> Result<(), SessionError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        match self.queued.take() {
            Some((clip, origin)) => self.share(session, clip, origin).await,
            None => Ok(()),
        }
    }

    /// Pushes the latest copy if it is recent and the phone lacks it.
    async fn push_recent<S>(&mut self, session: &mut Session<S>) -> Result<(), SessionError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let Some(clip) = self.hub.recent(Instant::now()) else {
            return Ok(());
        };
        if self.has(&clip) {
            return Ok(());
        }
        self.share(session, clip, Origin::Explicit).await
    }

    /// Notes the answer to the push in flight.
    fn answered(&mut self, refused: Option<ErrorReply>) {
        let Some((seq, clip)) = self.in_flight.take() else {
            return;
        };
        let Some(refused) = refused else {
            self.hub.emit(ClipboardEvent::Sent {
                device_id: self.device_id.clone(),
                bytes: clip.len(),
                sensitive: clip.is_sensitive(),
            });
            return;
        };
        tracing::info!(
            seq,
            code = refused.code,
            bytes = clip.len(),
            "the phone did not take the clipboard"
        );
        let reason = match refused.code() {
            ErrorCode::Disabled => PushFailure::TurnedOff,
            ErrorCode::Unavailable | ErrorCode::Busy => PushFailure::Unavailable,
            _ => PushFailure::Failed,
        };
        self.failed(&clip, reason);
    }

    /// A push of `clip` did not reach the phone: what it holds is unknown,
    /// unless it pushed something here since.
    fn failed(&mut self, clip: &Clip, reason: PushFailure) {
        if self.has(clip) {
            self.holds = None;
        }
        self.hub.emit(ClipboardEvent::Failed {
            device_id: self.device_id.clone(),
            reason,
        });
    }

    /// Takes the phone's push: puts the text on the clipboard, or holds it
    /// for the window, and answers.
    async fn receive<S>(
        &mut self,
        session: &mut Session<S>,
        seq: u64,
        push: ClipboardPush,
    ) -> Result<(), SessionError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        if !self.hub.settings().sharing {
            return refuse(
                session,
                seq,
                ErrorCode::Disabled,
                "clipboard sharing is off",
            )
            .await;
        }
        if push.text.is_empty() || push.text.len() > MAX_TEXT_BYTES {
            tracing::info!(bytes = push.text.len(), "refusing a clipboard push");
            let detail = format!("the text must be 1 to {MAX_TEXT_BYTES} bytes");
            return refuse(session, seq, ErrorCode::InvalidArgument, &detail).await;
        }
        let clip = Clip::new(ClipboardText {
            text: push.text,
            sensitive: push.sensitive,
        });
        let (bytes, sensitive) = (clip.len(), clip.is_sensitive());
        // The phone's text is newer than a copy that waited for it.
        self.queued = None;
        let held = match self.clipboard.access() {
            ClipboardAccess::Background => false,
            ClipboardAccess::FocusOnly => !self.hub.is_focused(),
        };
        let held = held || !self.write(&clip).await;
        if held && self.clipboard.access() == ClipboardAccess::Background {
            return refuse(
                session,
                seq,
                ErrorCode::Unavailable,
                "the clipboard cannot be set",
            )
            .await;
        }
        self.holds = Some(clip.digest());
        if held {
            self.hub.hold(clip, self.device_id.clone());
        } else {
            self.hub.discard_held();
        }
        session.ack(seq).await?;
        tracing::debug!(seq, bytes, held, "took the phone's clipboard");
        self.hub.emit(ClipboardEvent::Received {
            device_id: self.device_id.clone(),
            bytes,
            sensitive,
            held,
        });
        Ok(())
    }

    /// Puts `clip` on the clipboard. Returns whether it worked.
    async fn write(&self, clip: &Clip) -> bool {
        let marked = self.hub.mark(clip.digest());
        let written = self.clipboard.write(clip.text()).await;
        if let Err(error) = &written {
            tracing::warn!(%error, "could not set the clipboard");
        }
        self.hub.written(marked, written.is_ok());
        written.is_ok()
    }

    /// Reports the phone's state if it changed.
    fn report<S>(&mut self, session: &Session<S>)
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        if let Some(state) = phone_state(session) {
            self.set_state(state);
        }
    }

    fn set_state(&mut self, state: PhoneClipboard) {
        if self.state != Some(state) {
            self.state = Some(state);
            self.hub.set_phone(&self.device_id, state);
            self.hub.emit(ClipboardEvent::Phone {
                device_id: self.device_id.clone(),
                state,
            });
        }
    }
}

/// The phone's state in `session`, or `None` until its first
/// `FeatureStatus` says.
fn phone_state<S>(session: &Session<S>) -> Option<PhoneClipboard>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    if !session.shared_features().contains(&Feature::Clipboard) {
        return Some(PhoneClipboard::Unsupported);
    }
    let availability = session.peer_availability()?.get(&Feature::Clipboard)?;
    Some(match availability {
        Availability::Ready => PhoneClipboard::Ready,
        Availability::Disabled => PhoneClipboard::TurnedOff,
        _ => PhoneClipboard::Unavailable,
    })
}

async fn refuse<S>(
    session: &mut Session<S>,
    seq: u64,
    code: ErrorCode,
    detail: &str,
) -> Result<(), SessionError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let error = ErrorReply {
        code: code.into(),
        detail: detail.to_owned(),
        ..ErrorReply::default()
    };
    session.reply_error(seq, error).await.map(drop)
}
