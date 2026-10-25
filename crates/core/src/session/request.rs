//! Requests this side sends, and their answers.

use link2linux_protocol::Payload;
use link2linux_protocol::v1::ErrorReply;
use tokio::time::Instant;

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
}
