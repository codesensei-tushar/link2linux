//! Which message may go where, after the message index in
//! `docs/protocol.md`.
//!
//! The matches here are exhaustive on purpose: a message added to the
//! schema does not compile until it is classified.

use link2linux_protocol::Payload;

use crate::device::Feature;

/// The role a message plays in a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    /// Session control, handled by the session itself or reported as
    /// session state.
    Control,
    /// Part of the pairing exchange, allowed only on a pairing connection
    /// before the session proper starts.
    Pairing,
    /// A request, answered by [`Expects`] or by `ErrorReply`.
    Request(Expects),
    /// An answer to a request. It carries the request's `seq` in
    /// `reply_to`.
    Reply,
    /// A one-way feature event.
    Event,
}

/// The answer a request gets when it succeeds. Every request may also get
/// an `ErrorReply`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Expects {
    /// `Ack`.
    Ack,
    /// `ImageData`.
    ImageData,
    /// One or more `NotificationSyncBatch`.
    NotificationSyncBatch,
}

/// What the session needs to know about one message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Rules {
    /// The message's name in the schema, for errors and logs.
    pub(crate) name: &'static str,
    pub(crate) kind: Kind,
    /// The feature it belongs to. Messages without one are allowed in
    /// every session.
    pub(crate) feature: Option<Feature>,
}

/// Looks up the rules for `payload`.
pub(crate) fn of(payload: &Payload) -> Rules {
    use Kind::{Control, Event, Pairing, Reply, Request};
    use Payload as P;

    const NOTIFICATIONS: Option<Feature> = Some(Feature::Notifications);
    let (name, kind, feature) = match payload {
        P::Hello(_) => ("Hello", Control, None),
        P::Ping(_) => ("Ping", Control, None),
        P::Pong(_) => ("Pong", Control, None),
        P::Goodbye(_) => ("Goodbye", Control, None),
        P::FeatureStatus(_) => ("FeatureStatus", Control, None),
        P::Ack(_) => ("Ack", Reply, None),
        P::ErrorReply(_) => ("ErrorReply", Reply, None),
        P::ImageRequest(_) => ("ImageRequest", Request(Expects::ImageData), None),
        P::ImageData(_) => ("ImageData", Reply, None),
        P::PairingProof(_) => ("PairingProof", Pairing, None),
        P::SasCommit(_) => ("SasCommit", Pairing, None),
        P::SasNonce(_) => ("SasNonce", Pairing, None),
        P::PairingResult(_) => ("PairingResult", Pairing, None),
        P::PresenceUpdate(_) => ("PresenceUpdate", Control, None),
        P::KeyRotation(_) => ("KeyRotation", Request(Expects::Ack), None),
        P::NotificationSyncRequest(_) => (
            "NotificationSyncRequest",
            Request(Expects::NotificationSyncBatch),
            NOTIFICATIONS,
        ),
        P::NotificationSyncBatch(_) => ("NotificationSyncBatch", Reply, NOTIFICATIONS),
        P::NotificationPosted(_) => ("NotificationPosted", Event, NOTIFICATIONS),
        P::NotificationRemoved(_) => ("NotificationRemoved", Event, NOTIFICATIONS),
        P::NotificationDismissRequest(_) => (
            "NotificationDismissRequest",
            Request(Expects::Ack),
            NOTIFICATIONS,
        ),
        P::NotificationActionRequest(_) => (
            "NotificationActionRequest",
            Request(Expects::Ack),
            NOTIFICATIONS,
        ),
    };
    Rules {
        name,
        kind,
        feature,
    }
}

#[cfg(test)]
mod tests {
    use link2linux_protocol::v1::{ImageRequest, NotificationDismissRequest};

    use super::*;

    #[test]
    fn requests_name_their_answers() {
        let image = of(&Payload::ImageRequest(ImageRequest::default()));
        assert_eq!(image.kind, Kind::Request(Expects::ImageData));
        assert_eq!(image.feature, None);

        let dismiss = of(&Payload::NotificationDismissRequest(
            NotificationDismissRequest::default(),
        ));
        assert_eq!(dismiss.name, "NotificationDismissRequest");
        assert_eq!(dismiss.kind, Kind::Request(Expects::Ack));
        assert_eq!(dismiss.feature, Some(Feature::Notifications));
    }
}
