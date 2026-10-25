//! Phone calls, controlled from the desktop.
//!
//! The phone reports call state; the desktop places, answers and ends
//! calls. Call audio travels over Bluetooth, set up through the
//! `CallAudio` trait in `link2linux-platform`, not over this session.
//!
//! Stub: the types and the [`Calls`] trait are in place; the wire messages
//! come with this feature's schema.

use crate::device::FeatureError;

/// Which side started a call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CallDirection {
    /// Someone called the phone.
    Incoming,
    /// The phone called someone.
    Outgoing,
}

/// Where a call is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum CallState {
    /// Incoming and not answered yet.
    Ringing,
    /// Outgoing and not answered yet.
    Dialing,
    /// Both sides are talking.
    Active,
    /// On hold.
    Held,
    /// Over, answered or not.
    Ended,
}

/// A call on the phone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call {
    /// The phone's identifier for this call.
    pub id: String,
    /// The other party's number, when known.
    pub number: Option<String>,
    /// The other party's contact name, when known.
    pub contact_name: Option<String>,
    /// Who started it.
    pub direction: CallDirection,
    /// Where it is now.
    pub state: CallState,
}

/// What the phone reports about calls: every state change, as the full
/// current [`Call`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CallEvent {
    /// A call started or changed state.
    Updated(Call),
}

/// Requests the desktop makes about calls.
pub trait Calls: Send + Sync {
    /// Calls `number` from the phone.
    fn dial(&self, number: &str) -> impl Future<Output = Result<(), FeatureError>> + Send;

    /// Answers a ringing call.
    fn answer(&self, call_id: &str) -> impl Future<Output = Result<(), FeatureError>> + Send;

    /// Ends a call, or declines it while it rings.
    fn hang_up(&self, call_id: &str) -> impl Future<Output = Result<(), FeatureError>> + Send;
}
