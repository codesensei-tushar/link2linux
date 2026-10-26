//! How a session ends, and what the desktop does next.

use link2linux_protocol::v1::{self, GoodbyeReason};

/// The peer's `Goodbye`: it ended the session on purpose, and says why.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct PeerGoodbye {
    /// Why the peer ended the session. Reasons added by newer versions
    /// read as [`GoodbyeReason::Unspecified`].
    pub reason: GoodbyeReason,
    /// English text for logs. Never shown to the user as it is.
    pub detail: String,
}

impl PeerGoodbye {
    /// What the desktop does next, decided by the [`reason`](Self::reason).
    #[must_use]
    pub fn reconnect_policy(&self) -> ReconnectPolicy {
        ReconnectPolicy::after_goodbye(self.reason)
    }
}

impl From<v1::Goodbye> for PeerGoodbye {
    fn from(goodbye: v1::Goodbye) -> Self {
        Self {
            reason: goodbye.reason(),
            detail: goodbye.detail,
        }
    }
}

/// What the desktop does after a session ends.
///
/// A `Goodbye` decides it, as `docs/protocol.md` ("Ending a session")
/// lays out. A session that ends without one counts as lost, which is
/// [`ReconnectPolicy::Backoff`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ReconnectPolicy {
    /// Keep dialing on the usual backoff schedule. For a lost connection,
    /// and after `SHUTDOWN`, `PROTOCOL_ERROR` or no reason at all.
    Backoff,
    /// Stop dialing until the user reconnects on the device that sent the
    /// `Goodbye`. After `USER_DISCONNECT`.
    WaitForUser,
    /// Stop dialing until one of the two apps is updated. After
    /// `VERSION_MISMATCH`.
    WaitForUpdate,
    /// Delete the pairing record and stop dialing for good. After
    /// `UNPAIRED`.
    Unpair,
    /// Do nothing: a newer session between the same two devices already
    /// replaced this one. After `REPLACED`.
    Replaced,
}

impl ReconnectPolicy {
    /// The policy for a session that ended with a `Goodbye` for `reason`.
    #[must_use]
    pub const fn after_goodbye(reason: GoodbyeReason) -> Self {
        match reason {
            GoodbyeReason::Unspecified | GoodbyeReason::Shutdown | GoodbyeReason::ProtocolError => {
                Self::Backoff
            }
            GoodbyeReason::UserDisconnect => Self::WaitForUser,
            GoodbyeReason::Unpaired => Self::Unpair,
            GoodbyeReason::Replaced => Self::Replaced,
            GoodbyeReason::VersionMismatch => Self::WaitForUpdate,
        }
    }

    /// The policy for a session that ended without a `Goodbye`: the stream
    /// closed, failed or went silent.
    #[must_use]
    pub const fn after_loss() -> Self {
        Self::Backoff
    }

    /// Whether the desktop keeps dialing the phone on its own.
    #[must_use]
    pub const fn keeps_dialing(self) -> bool {
        matches!(self, Self::Backoff)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_every_reason() {
        let cases = [
            (GoodbyeReason::Unspecified, ReconnectPolicy::Backoff),
            (GoodbyeReason::Shutdown, ReconnectPolicy::Backoff),
            (GoodbyeReason::UserDisconnect, ReconnectPolicy::WaitForUser),
            (GoodbyeReason::Unpaired, ReconnectPolicy::Unpair),
            (GoodbyeReason::Replaced, ReconnectPolicy::Replaced),
            (GoodbyeReason::ProtocolError, ReconnectPolicy::Backoff),
            (
                GoodbyeReason::VersionMismatch,
                ReconnectPolicy::WaitForUpdate,
            ),
        ];
        for (reason, policy) in cases {
            assert_eq!(ReconnectPolicy::after_goodbye(reason), policy, "{reason:?}");
        }
        assert!(ReconnectPolicy::after_loss().keeps_dialing());
        assert!(!ReconnectPolicy::Replaced.keeps_dialing());
    }

    #[test]
    fn unknown_reasons_count_as_unspecified() {
        let goodbye = PeerGoodbye::from(v1::Goodbye {
            reason: 99,
            detail: "from the future".into(),
        });
        assert_eq!(goodbye.reason, GoodbyeReason::Unspecified);
        assert_eq!(goodbye.reconnect_policy(), ReconnectPolicy::Backoff);
    }
}
