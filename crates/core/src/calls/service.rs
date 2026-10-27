//! The calls feature in one phone's session.

use link2linux_protocol::Payload;
use link2linux_protocol::v1::{BluetoothPairingRequest, ErrorCode, ErrorReply};
use tokio::io::{AsyncRead, AsyncWrite};

use super::pairing::PhoneAnswer;
use crate::Feature;
use crate::session::{Availability, Session, SessionError, SessionEvent};

/// What [`CallsService::handle_event`] made of an event.
#[derive(Debug)]
#[allow(
    clippy::large_enum_variant,
    reason = "most events are not the feature's; boxing them would only add an allocation"
)]
pub enum Handled {
    /// Not an event of the calls feature: the caller handles it.
    NotMine(SessionEvent),
    /// The calls feature took it.
    Taken,
    /// The phone answered the pairing request.
    Answered(PhoneAnswer),
}

/// The calls feature with one phone, in its session. Calls themselves
/// travel over Bluetooth, so the session only carries the desktop's
/// availability and the request to pair:
///
/// 1. [`CallsService::session_started`] once the session is up, and
///    [`CallsService::availability_changed`] whenever the desktop's
///    availability changes;
/// 2. every [`SessionEvent`] through [`CallsService::handle_event`];
/// 3. [`CallsService::ask_to_pair`] while the user pairs over Bluetooth;
/// 4. [`CallsService::session_ended`] when the session is gone.
#[derive(Debug, Default)]
pub struct CallsService {
    /// The pairing request that waits for its answer.
    in_flight: Option<u64>,
}

impl CallsService {
    /// The service for one phone.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Reports the desktop's `availability` for calls. Call it once the
    /// session is up.
    ///
    /// # Errors
    ///
    /// Fails when the session fails.
    pub async fn session_started<S>(
        &mut self,
        session: &mut Session<S>,
        availability: Availability,
    ) -> Result<(), SessionError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        self.in_flight = None;
        self.availability_changed(session, availability).await
    }

    /// Tells the phone the desktop's new `availability` for calls.
    ///
    /// # Errors
    ///
    /// Fails when the session fails.
    pub async fn availability_changed<S>(
        &mut self,
        session: &mut Session<S>,
        availability: Availability,
    ) -> Result<(), SessionError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        session.set_availability(Feature::Calls, availability).await
    }

    /// The session is over. Returns what became of a pairing request that
    /// waited.
    pub fn session_ended(&mut self) -> Option<PhoneAnswer> {
        self.in_flight.take().map(|_| PhoneAnswer::NoAnswer)
    }

    /// Whether a pairing request waits for the phone's answer.
    #[must_use]
    pub fn is_asking(&self) -> bool {
        self.in_flight.is_some()
    }

    /// Asks the phone to pair with the desktop's adapter that `request`
    /// names, if calls are ready on both devices and no request waits.
    /// Returns whether the request went out.
    ///
    /// # Errors
    ///
    /// Fails when the session fails.
    pub async fn ask_to_pair<S>(
        &mut self,
        session: &mut Session<S>,
        request: BluetoothPairingRequest,
    ) -> Result<bool, SessionError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        if self.in_flight.is_some() || !session.is_ready(Feature::Calls) {
            return Ok(false);
        }
        let seq = session
            .start_request(Payload::BluetoothPairingRequest(request))
            .await?;
        tracing::debug!(seq, "asked the phone to pair over Bluetooth");
        self.in_flight = Some(seq);
        Ok(true)
    }

    /// Takes the events of the calls feature: the answer to the pairing
    /// request, and a pairing request from the phone, which only the
    /// desktop sends and is refused. A `FeatureStatus` is returned, for the
    /// caller to look whether a request can go out now.
    ///
    /// # Errors
    ///
    /// Fails when the session fails.
    pub async fn handle_event<S>(
        &mut self,
        session: &mut Session<S>,
        event: SessionEvent,
    ) -> Result<Handled, SessionError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        match event {
            SessionEvent::Reply(reply) if self.in_flight == Some(reply.request) => {
                self.in_flight = None;
                let answer = match reply.into_result() {
                    Ok(Payload::Ack(_)) => PhoneAnswer::Asked,
                    Ok(_) => PhoneAnswer::NoAnswer,
                    Err(error) => PhoneAnswer::Refused(error.code()),
                };
                tracing::debug!(?answer, "the phone answered the Bluetooth pairing request");
                Ok(Handled::Answered(answer))
            }
            SessionEvent::RequestTimedOut { request } if self.in_flight == Some(request) => {
                self.in_flight = None;
                Ok(Handled::Answered(PhoneAnswer::NoAnswer))
            }
            SessionEvent::Request {
                seq,
                payload: Payload::BluetoothPairingRequest(_),
            } => {
                session
                    .reply_error(
                        seq,
                        ErrorReply {
                            code: ErrorCode::Unsupported.into(),
                            detail: "only the desktop asks to pair over Bluetooth".into(),
                            ..ErrorReply::default()
                        },
                    )
                    .await?;
                Ok(Handled::Taken)
            }
            other => Ok(Handled::NotMine(other)),
        }
    }
}
