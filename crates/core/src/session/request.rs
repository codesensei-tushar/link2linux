//! Requests this side sends, and their answers.

use std::time::Duration;

use link2linux_protocol::Payload;
use link2linux_protocol::v1::ErrorReply;
use tokio::time::Instant;

use super::SessionError;
use super::rules::Expects;

/// An answer to one of this side's requests.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Reply {
    /// The sequence number of the request it answers.
    pub request: u64,
    /// The peer's sequence number for the answer.
    pub seq: u64,
    /// `Ack`, `ErrorReply`, or the reply message the request names.
    pub payload: Payload,
    /// Whether this is the request's last answer. Only requests answered by
    /// several messages, such as `NotificationSyncRequest`, get answers
    /// that are not.
    pub last: bool,
}

impl Reply {
    /// The answer's payload, or the peer's `ErrorReply` as an error.
    ///
    /// # Errors
    ///
    /// Returns the `ErrorReply` when the peer refused the request.
    pub fn into_result(self) -> Result<Payload, ErrorReply> {
        match self.payload {
            Payload::ErrorReply(error) => Err(error),
            payload => Ok(payload),
        }
    }
}

/// Why a request got no answer it could use.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum RequestError {
    /// The peer answered with an `ErrorReply`. Its code says whether
    /// retrying can help; its detail is for logs only.
    #[error("the peer refused the request: {} ({})", .0.code().as_str_name(), .0.detail)]
    Refused(ErrorReply),
    /// No answer arrived within [`SessionConfig::request_timeout`]. An
    /// answer that arrives later is ignored.
    ///
    /// [`SessionConfig::request_timeout`]: super::SessionConfig::request_timeout
    #[error("no answer within {0:?}")]
    TimedOut(Duration),
    /// The session ended before the answer arrived.
    #[error("the session ended before the answer arrived")]
    Ended,
    /// No request with this sequence number waits for an answer: it was
    /// never sent, or its last answer has already arrived.
    #[error("no request {0} waits for an answer")]
    UnknownRequest(u64),
    /// Sending the request failed, or the session failed while waiting. The
    /// session is over in the second case.
    #[error(transparent)]
    Session(SessionError),
}

impl From<SessionError> for RequestError {
    fn from(err: SessionError) -> Self {
        match err {
            SessionError::Ended => Self::Ended,
            err => Self::Session(err),
        }
    }
}

/// A request of this side that waits for an answer.
#[derive(Debug, Clone, Copy)]
pub(super) struct Pending {
    /// What may answer it, besides `ErrorReply`.
    pub(super) expects: Expects,
    /// When it times out, unless an answer arrives first.
    pub(super) deadline: Instant,
}

#[cfg(test)]
mod tests {
    use link2linux_protocol::v1::{Ack, ErrorCode};

    use super::*;

    #[test]
    fn errors_become_results() {
        let reply = |payload| Reply {
            request: 2,
            seq: 3,
            payload,
            last: true,
        };
        let ack = Payload::Ack(Ack {});
        assert_eq!(reply(ack.clone()).into_result(), Ok(ack));
        let error = ErrorReply {
            code: ErrorCode::NotFound.into(),
            ..ErrorReply::default()
        };
        let refused = reply(Payload::ErrorReply(error.clone())).into_result();
        assert_eq!(refused, Err(error));
    }

    #[test]
    fn an_ended_session_is_reported_as_such() {
        let err = RequestError::from(SessionError::Ended);
        assert!(matches!(err, RequestError::Ended), "{err:?}");
        let err = RequestError::from(SessionError::UnexpectedHello);
        assert!(matches!(err, RequestError::Session(_)), "{err:?}");
    }
}
