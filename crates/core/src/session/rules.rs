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
    /// `FileAccept`.
    FileAccept,
    /// `PhoneStatusReply`.
    PhoneStatusReply,
    /// `PhotoListReply`.
    PhotoListReply,
    /// `PhotoThumbnail`.
    PhotoThumbnail,
    /// `PhotoData`.
    PhotoData,
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
                | (Self::FileAccept, Payload::FileAccept(_))
                | (Self::PhoneStatusReply, Payload::PhoneStatusReply(_))
                | (Self::PhotoListReply, Payload::PhotoListReply(_))
                | (Self::PhotoThumbnail, Payload::PhotoThumbnail(_))
                | (Self::PhotoData, Payload::PhotoData(_))
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
    const FILES: Option<Feature> = Some(Feature::Files);
    const CLIPBOARD: Option<Feature> = Some(Feature::Clipboard);
    const CALLS: Option<Feature> = Some(Feature::Calls);
    const PHOTOS: Option<Feature> = Some(Feature::Photos);
    const SCREEN: Option<Feature> = Some(Feature::Screen);
    const STATUS: Option<Feature> = Some(Feature::Status);
    const RING: Option<Feature> = Some(Feature::Ring);
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
        P::BluetoothPairingRequest(_) => ("BluetoothPairingRequest", Request(Expects::Ack), CALLS),
        P::PhotoListRequest(_) => ("PhotoListRequest", Request(Expects::PhotoListReply), PHOTOS),
        P::PhotoListReply(_) => ("PhotoListReply", Reply, PHOTOS),
        P::PhotoListChanged(_) => ("PhotoListChanged", Event, PHOTOS),
        P::PhotoThumbnailRequest(_) => (
            "PhotoThumbnailRequest",
            Request(Expects::PhotoThumbnail),
            PHOTOS,
        ),
        P::PhotoThumbnail(_) => ("PhotoThumbnail", Reply, PHOTOS),
        P::PhotoDataRequest(_) => ("PhotoDataRequest", Request(Expects::PhotoData), PHOTOS),
        P::PhotoData(_) => ("PhotoData", Reply, PHOTOS),
        P::FileOffer(_) => ("FileOffer", Request(Expects::FileAccept), FILES),
        P::FileAccept(_) => ("FileAccept", Reply, FILES),
        P::FileChunk(_) => ("FileChunk", Request(Expects::Ack), FILES),
        P::FileCancel(_) => ("FileCancel", Request(Expects::Ack), FILES),
        P::ClipboardPush(_) => ("ClipboardPush", Request(Expects::Ack), CLIPBOARD),
        P::ScreenStart(_) => ("ScreenStart", Request(Expects::Ack), SCREEN),
        P::ScreenStop(_) => ("ScreenStop", Request(Expects::Ack), SCREEN),
        P::ScreenFormat(_) => ("ScreenFormat", Event, SCREEN),
        P::ScreenFrame(_) => ("ScreenFrame", Event, SCREEN),
        P::ScreenKeyFrameRequest(_) => ("ScreenKeyFrameRequest", Event, SCREEN),
        P::ScreenEnded(_) => ("ScreenEnded", Event, SCREEN),
        P::ScreenInput(_) => ("ScreenInput", Request(Expects::Ack), SCREEN),
        P::PhoneStatusRequest(_) => (
            "PhoneStatusRequest",
            Request(Expects::PhoneStatusReply),
            STATUS,
        ),
        P::PhoneStatusReply(_) => ("PhoneStatusReply", Reply, STATUS),
        P::PhoneStatusChanged(_) => ("PhoneStatusChanged", Event, STATUS),
        P::RingRequest(_) => ("RingRequest", Request(Expects::Ack), RING),
        P::RingStop(_) => ("RingStop", Request(Expects::Ack), RING),
        P::RingEnded(_) => ("RingEnded", Event, RING),
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
        Ack, BluetoothPairingRequest, ClipboardPush, ErrorReply, FileAccept, ImageData,
        ImageRequest, NotificationDismissRequest, NotificationSyncBatch, PhoneStatusChanged,
        PhoneStatusReply, PhoneStatusRequest, PhotoData, PhotoDataRequest, PhotoListChanged,
        PhotoListReply, PhotoListRequest, PhotoThumbnail, PhotoThumbnailRequest, RingEnded,
        RingStop, ScreenFrame, ScreenInput, ScreenKeyFrameRequest, ScreenStart,
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

        let push = of(&Payload::ClipboardPush(ClipboardPush::default()));
        assert_eq!(push.kind, Kind::Request(Expects::Ack));
        assert_eq!(push.feature, Some(Feature::Clipboard));

        let pairing = of(&Payload::BluetoothPairingRequest(
            BluetoothPairingRequest::default(),
        ));
        assert_eq!(pairing.kind, Kind::Request(Expects::Ack));
        assert_eq!(pairing.feature, Some(Feature::Calls));

        let status = of(&Payload::PhoneStatusRequest(PhoneStatusRequest {}));
        assert_eq!(status.kind, Kind::Request(Expects::PhoneStatusReply));
        assert_eq!(status.feature, Some(Feature::Status));

        let stop = of(&Payload::RingStop(RingStop {}));
        assert_eq!(stop.kind, Kind::Request(Expects::Ack));
        assert_eq!(stop.feature, Some(Feature::Ring));
    }

    #[test]
    fn photo_requests_name_their_answers() {
        let list = of(&Payload::PhotoListRequest(PhotoListRequest {}));
        assert_eq!(list.kind, Kind::Request(Expects::PhotoListReply));
        assert_eq!(list.feature, Some(Feature::Photos));
        assert!(
            Expects::PhotoListReply.allows(&Payload::PhotoListReply(PhotoListReply::default()))
        );

        let thumbnail = of(&Payload::PhotoThumbnailRequest(
            PhotoThumbnailRequest::default(),
        ));
        assert_eq!(thumbnail.kind, Kind::Request(Expects::PhotoThumbnail));
        assert!(
            Expects::PhotoThumbnail.allows(&Payload::PhotoThumbnail(PhotoThumbnail::default()))
        );
        assert!(!Expects::PhotoThumbnail.allows(&Payload::PhotoData(PhotoData::default())));

        let data = of(&Payload::PhotoDataRequest(PhotoDataRequest::default()));
        assert_eq!(data.kind, Kind::Request(Expects::PhotoData));
        assert!(Expects::PhotoData.allows(&Payload::PhotoData(PhotoData::default())));

        let changed = of(&Payload::PhotoListChanged(PhotoListChanged::default()));
        assert_eq!(changed.kind, Kind::Event);
        assert_eq!(changed.feature, Some(Feature::Photos));
    }

    #[test]
    fn screen_video_is_events_and_its_control_requests() {
        let start = of(&Payload::ScreenStart(ScreenStart::default()));
        assert_eq!(start.kind, Kind::Request(Expects::Ack));
        assert_eq!(start.feature, Some(Feature::Screen));
        let input = of(&Payload::ScreenInput(ScreenInput::default()));
        assert_eq!(input.kind, Kind::Request(Expects::Ack));
        for event in [
            Payload::ScreenFrame(ScreenFrame::default()),
            Payload::ScreenKeyFrameRequest(ScreenKeyFrameRequest {}),
        ] {
            let rules = of(&event);
            assert_eq!(rules.kind, Kind::Event);
            assert_eq!(rules.feature, Some(Feature::Screen));
        }
    }

    #[test]
    fn status_and_ring_updates_are_events() {
        let changed = of(&Payload::PhoneStatusChanged(PhoneStatusChanged::default()));
        assert_eq!(changed.kind, Kind::Event);
        assert_eq!(changed.feature, Some(Feature::Status));
        let ended = of(&Payload::RingEnded(RingEnded::default()));
        assert_eq!(ended.kind, Kind::Event);
        assert_eq!(ended.feature, Some(Feature::Ring));
    }

    #[test]
    fn events_are_named_without_their_content() {
        use crate::session::SessionEvent;

        let push = SessionEvent::Message {
            seq: 4,
            payload: Payload::ClipboardPush(ClipboardPush {
                text: "hunter2".into(),
                sensitive: true,
            }),
        };
        assert_eq!(push.name(), "ClipboardPush");
        let timed_out = SessionEvent::RequestTimedOut { request: 4 };
        assert_eq!(timed_out.name(), "RequestTimedOut");
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
        let accept = Payload::FileAccept(FileAccept::default());
        assert!(Expects::FileAccept.allows(&accept));
        assert!(Expects::FileAccept.allows(&error));
        assert!(!Expects::FileAccept.allows(&ack));
        assert!(!Expects::Ack.allows(&accept));
        let status = Payload::PhoneStatusReply(PhoneStatusReply::default());
        assert!(Expects::PhoneStatusReply.allows(&status));
        assert!(Expects::PhoneStatusReply.allows(&error));
        assert!(!Expects::PhoneStatusReply.allows(&ack));
        assert!(!Expects::Ack.allows(&status));
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
