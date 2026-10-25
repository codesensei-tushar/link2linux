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

impl Expects {
    /// Whether `payload` may answer a request that expects `self`.
    pub(crate) fn allows(self, payload: &Payload) -> bool {
        matches!(
            (self, payload),
            (_, Payload::ErrorReply(_))
                | (Self::Ack, Payload::Ack(_))
                | (Self::ImageData, Payload::ImageData(_))
                | (
                    Self::NotificationSyncBatch,
                    Payload::NotificationSyncBatch(_)
                )
        )
    }
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

/// Whether `answer` is the last answer to its request. Only a reply message
/// that says so, such as `NotificationSyncBatch.more`, announces more.
pub(crate) fn is_last_answer(answer: &Payload) -> bool {
    !matches!(answer, Payload::NotificationSyncBatch(batch) if batch.more)
}

#[cfg(test)]
mod tests {
    use link2linux_protocol::v1::{
        Ack, ErrorReply, ImageData, ImageRequest, NotificationDismissRequest, NotificationSyncBatch,
    };

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

    #[test]
    fn checks_which_answers_fit() {
        let ack = Payload::Ack(Ack {});
        let error = Payload::ErrorReply(ErrorReply::default());
        let data = Payload::ImageData(ImageData::default());
        assert!(Expects::Ack.allows(&ack));
        assert!(Expects::Ack.allows(&error));
        assert!(!Expects::Ack.allows(&data));
        assert!(Expects::ImageData.allows(&data));
        assert!(!Expects::ImageData.allows(&ack));
        assert!(Expects::NotificationSyncBatch.allows(&error));
    }

    #[test]
    fn only_batches_with_more_are_not_last() {
        let more = NotificationSyncBatch {
            more: true,
            ..NotificationSyncBatch::default()
        };
        assert!(!is_last_answer(&Payload::NotificationSyncBatch(more)));
        let last = NotificationSyncBatch::default();
        assert!(is_last_answer(&Payload::NotificationSyncBatch(last)));
        assert!(is_last_answer(&Payload::Ack(Ack {})));
    }
}
