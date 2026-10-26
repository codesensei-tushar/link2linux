//! The checks a frame passes before its body is decoded.
//!
//! Decoding allocates for every entry of a repeated field, and an empty
//! entry takes two bytes on the wire but up to hundreds in memory, so a
//! frame well within the size limit could need hundreds of MiB once
//! decoded. [`check`] reads the frame's wire format without allocating and
//! refuses it when a repeated field holds more entries than
//! `docs/protocol.md` allows, or when the frame is too large for a frame
//! without an image. It reads into the payloads that have repeated fields
//! and skips everything else by its length.
//!
//! A decoder merges a message field that appears more than once, so the
//! entries of every occurrence count together.
//!
//! The limits inside a notification are looser than the content limits of
//! `docs/protocol.md`, "Notifications", which the desktop applies after
//! decoding by dropping extra messages, actions and reply choices. These
//! only bound what decoding can allocate.

use crate::codec::{FrameError, peek_varint};

/// `Envelope` payloads, by field number.
const HELLO: u32 = 10;
const FEATURE_STATUS: u32 = 14;
const IMAGE_DATA: u32 = 18;
const NOTIFICATION_SYNC_BATCH: u32 = 101;
const NOTIFICATION_POSTED: u32 = 102;

/// A repeated field and the most entries a frame may hold.
#[derive(Debug, Clone, Copy)]
struct Limit {
    field: &'static str,
    max: usize,
}

impl Limit {
    /// Counts one more entry into `count`.
    fn add(self, count: &mut usize) -> Result<(), FrameError> {
        *count += 1;
        if *count > self.max {
            return Err(FrameError::TooManyEntries {
                field: self.field,
                max: self.max,
            });
        }
        Ok(())
    }
}

const FEATURES: Limit = Limit {
    field: "Hello.features",
    max: 64,
};
const FEATURE_STATES: Limit = Limit {
    field: "FeatureStatus.features",
    max: 64,
};
/// Of a sync batch and `NotificationPosted` together.
const NOTIFICATIONS: Limit = Limit {
    field: "PhoneNotification",
    max: 100,
};
const REMOVED_KEYS: Limit = Limit {
    field: "NotificationSyncBatch.removed_keys",
    max: 1000,
};
/// Per notification.
const ACTIONS: Limit = Limit {
    field: "PhoneNotification.actions",
    max: 32,
};
/// Per notification.
const MESSAGES: Limit = Limit {
    field: "PhoneNotification.messages",
    max: 32,
};
/// Per action.
const REPLY_CHOICES: Limit = Limit {
    field: "NotificationAction.reply_choices",
    max: 16,
};

/// Entries counted over a whole frame.
#[derive(Debug, Default)]
struct Totals {
    features: usize,
    feature_states: usize,
    notifications: usize,
    removed_keys: usize,
}

/// Checks the frame body `body` (one serialized `Envelope`) before it is
/// decoded: the entries of its repeated fields, and that it is at most
/// `max_message_len` bytes long, not counting an `ImageData` payload.
pub(crate) fn check(body: &[u8], max_message_len: usize) -> Result<(), FrameError> {
    let mut totals = Totals::default();
    let mut image_len = 0;
    for_each_field(body, |payload, contents| match payload {
        HELLO => count(contents, 5, FEATURES, &mut totals.features),
        FEATURE_STATUS => count(contents, 1, FEATURE_STATES, &mut totals.feature_states),
        IMAGE_DATA => {
            image_len += contents.len();
            Ok(())
        }
        NOTIFICATION_SYNC_BATCH => for_each_field(contents, |field, value| match field {
            2 => notification(value, &mut totals.notifications),
            3 => REMOVED_KEYS.add(&mut totals.removed_keys),
            _ => Ok(()),
        }),
        NOTIFICATION_POSTED => for_each_field(contents, |field, value| match field {
            1 => notification(value, &mut totals.notifications),
            _ => Ok(()),
        }),
        _ => Ok(()),
    })?;
    let len = body.len() - image_len;
    if len > max_message_len {
        return Err(FrameError::TooLarge {
            len: len as u64,
            max: max_message_len,
        });
    }
    Ok(())
}

/// Checks one `PhoneNotification` and counts it into `total`.
fn notification(buf: &[u8], total: &mut usize) -> Result<(), FrameError> {
    NOTIFICATIONS.add(total)?;
    let (mut actions, mut messages) = (0, 0);
    for_each_field(buf, |field, value| match field {
        14 => {
            ACTIONS.add(&mut actions)?;
            count(value, 5, REPLY_CHOICES, &mut 0)
        }
        15 => MESSAGES.add(&mut messages),
        _ => Ok(()),
    })
}

/// Counts the entries of the repeated field `number` of the message in
/// `buf` into `total`.
fn count(buf: &[u8], number: u32, limit: Limit, total: &mut usize) -> Result<(), FrameError> {
    for_each_field(buf, |field, _| {
        if field == number {
            limit.add(total)
        } else {
            Ok(())
        }
    })
}

/// Calls `visit` with the field number and contents of each
/// length-delimited field of the message in `buf`, in wire order, and skips
/// the other fields.
fn for_each_field(
    mut buf: &[u8],
    mut visit: impl FnMut(u32, &[u8]) -> Result<(), FrameError>,
) -> Result<(), FrameError> {
    while !buf.is_empty() {
        let key = varint(&mut buf)?;
        let number = u32::try_from(key >> 3).map_err(|_| FrameError::Malformed)?;
        let skip = match key & 0b111 {
            0 => {
                varint(&mut buf)?;
                0
            }
            1 => 8,
            2 => {
                let len = usize::try_from(varint(&mut buf)?).map_err(|_| FrameError::Malformed)?;
                let contents = buf.get(..len).ok_or(FrameError::Malformed)?;
                visit(number, contents)?;
                len
            }
            5 => 4,
            // Groups (3 and 4) are not part of proto3; 6 and 7 are not
            // wire types.
            _ => return Err(FrameError::Malformed),
        };
        buf = buf.get(skip..).ok_or(FrameError::Malformed)?;
    }
    Ok(())
}

/// Reads the varint at the start of `buf` and moves past it.
fn varint(buf: &mut &[u8]) -> Result<u64, FrameError> {
    match peek_varint(buf) {
        Ok(Some((value, len))) => {
            *buf = &buf[len..];
            Ok(value)
        }
        _ => Err(FrameError::Malformed),
    }
}

#[cfg(test)]
mod tests {
    use prost::Message;

    use super::*;
    use crate::v1::{
        FeatureState, FeatureStatus, ImageData, NotificationAction, NotificationPosted,
        NotificationSyncBatch, PhoneNotification,
    };
    use crate::{Envelope, Hello, Payload};

    fn frame(payload: Payload) -> Vec<u8> {
        Envelope {
            seq: 1,
            payload: Some(payload),
            ..Envelope::default()
        }
        .encode_to_vec()
    }

    fn batch(notifications: usize, removed_keys: usize) -> Payload {
        Payload::NotificationSyncBatch(NotificationSyncBatch {
            notifications: vec![PhoneNotification::default(); notifications],
            removed_keys: vec![String::new(); removed_keys],
            ..NotificationSyncBatch::default()
        })
    }

    fn posted(notification: PhoneNotification) -> Payload {
        Payload::NotificationPosted(NotificationPosted {
            notification: Some(notification),
            alert: true,
        })
    }

    fn checked(body: &[u8]) -> Result<(), FrameError> {
        check(body, usize::MAX)
    }

    fn too_many(result: Result<(), FrameError>) -> &'static str {
        match result {
            Err(FrameError::TooManyEntries { field, .. }) => field,
            other => panic!("expected too many entries, got {other:?}"),
        }
    }

    #[test]
    fn accepts_frames_at_every_limit() {
        let hello = Hello {
            features: vec!["notifications".into(); 64],
            ..Hello::default()
        };
        checked(&frame(Payload::Hello(hello))).unwrap();
        let status = FeatureStatus {
            features: vec![FeatureState::default(); 64],
        };
        checked(&frame(Payload::FeatureStatus(status))).unwrap();
        checked(&frame(batch(100, 1000))).unwrap();
        let full = PhoneNotification {
            actions: vec![
                NotificationAction {
                    reply_choices: vec!["ok".into(); 16],
                    ..NotificationAction::default()
                };
                32
            ],
            messages: vec![Default::default(); 32],
            ..PhoneNotification::default()
        };
        checked(&frame(posted(full.clone()))).unwrap();
        let mut batch = batch(0, 0);
        if let Payload::NotificationSyncBatch(batch) = &mut batch {
            batch.notifications = vec![full; 100];
        }
        checked(&frame(batch)).unwrap();
    }

    #[test]
    fn refuses_one_entry_too_many() {
        let hello = Hello {
            features: vec![String::new(); 65],
            ..Hello::default()
        };
        assert_eq!(
            too_many(checked(&frame(Payload::Hello(hello)))),
            "Hello.features"
        );
        let status = FeatureStatus {
            features: vec![FeatureState::default(); 65],
        };
        assert_eq!(
            too_many(checked(&frame(Payload::FeatureStatus(status)))),
            "FeatureStatus.features"
        );
        assert_eq!(
            too_many(checked(&frame(batch(101, 0)))),
            "PhoneNotification"
        );
        assert_eq!(
            too_many(checked(&frame(batch(0, 1001)))),
            "NotificationSyncBatch.removed_keys"
        );
        let actions = PhoneNotification {
            actions: vec![NotificationAction::default(); 33],
            ..PhoneNotification::default()
        };
        assert_eq!(
            too_many(checked(&frame(posted(actions)))),
            "PhoneNotification.actions"
        );
        let messages = PhoneNotification {
            messages: vec![Default::default(); 33],
            ..PhoneNotification::default()
        };
        assert_eq!(
            too_many(checked(&frame(posted(messages)))),
            "PhoneNotification.messages"
        );
        let choices = PhoneNotification {
            actions: vec![NotificationAction {
                reply_choices: vec![String::new(); 17],
                ..NotificationAction::default()
            }],
            ..PhoneNotification::default()
        };
        assert_eq!(
            too_many(checked(&frame(posted(choices)))),
            "NotificationAction.reply_choices"
        );
    }

    #[test]
    fn counts_every_occurrence_of_a_merged_payload() {
        // Two serialized envelopes back to back decode as one, whose batch
        // holds the notifications of both.
        let mut body = frame(batch(60, 0));
        body.extend(frame(batch(60, 0)));
        let merged = Envelope::decode(body.as_slice()).unwrap();
        assert!(matches!(
            merged.payload,
            Some(Payload::NotificationSyncBatch(batch)) if batch.notifications.len() == 120
        ));
        assert_eq!(too_many(checked(&body)), "PhoneNotification");

        // A posted notification counts against the same total.
        let mut body = frame(batch(100, 0));
        body.extend(frame(posted(PhoneNotification::default())));
        assert_eq!(too_many(checked(&body)), "PhoneNotification");
    }

    #[test]
    fn only_an_image_may_take_more_than_the_message_limit() {
        let image = frame(Payload::ImageData(ImageData {
            data: vec![7; 1000],
            ..ImageData::default()
        }));
        check(&image, 100).unwrap();
        let text = frame(posted(PhoneNotification {
            text: "x".repeat(1000),
            ..PhoneNotification::default()
        }));
        assert!(matches!(
            check(&text, 100),
            Err(FrameError::TooLarge { max: 100, .. })
        ));
        // What is not the image still counts.
        let mut both = image;
        both.extend(frame(Payload::Hello(Hello {
            device_name: "x".repeat(200),
            ..Hello::default()
        })));
        assert!(matches!(
            check(&both, 100),
            Err(FrameError::TooLarge { .. })
        ));
    }

    #[test]
    fn refuses_bodies_it_cannot_read() {
        for body in [
            &[0xff, 0xff][..],             // a key that never ends
            &[0x08][..],                   // a varint field without its value
            &[0x09, 1, 2, 3][..],          // a 64-bit field cut short
            &[0xd2, 0x06, 0x05, 0x00][..], // a batch longer than the frame
            &[0x0b, 0x0c][..],             // a group
            &[0x0e][..],                   // wire type 6
        ] {
            assert!(
                matches!(checked(body), Err(FrameError::Malformed)),
                "{body:02x?}"
            );
        }
        // Inside a payload too: a batch whose notification is cut short.
        assert!(matches!(
            checked(&[0xaa, 0x06, 0x02, 0x12, 0x05]),
            Err(FrameError::Malformed)
        ));
    }

    #[test]
    fn skips_fields_it_does_not_count() {
        let mut body = frame(Payload::Hello(Hello::default()));
        // Unknown fields of every wire type, at the envelope's top level.
        body.extend([
            0x78, 0x01, 0x79, 0, 0, 0, 0, 0, 0, 0, 0, 0x7a, 0x01, 0xaa, 0x7d, 0, 0, 0, 0,
        ]);
        checked(&body).unwrap();
    }
}
