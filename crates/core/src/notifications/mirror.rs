//! The desktop's copy of the phone's notifications, kept in step through
//! delta sync (`docs/protocol.md`, "Delta sync" and "Notifications").
//!
//! The desktop asks for the changes since its [`SyncCursor`] with
//! [`Mirror::sync_request`], applies the answering batches with
//! [`Mirror::apply_batch`], and from then on the phone's live events with
//! [`Mirror::apply_posted`] and [`Mirror::apply_removed`]. Each call returns
//! the [`Change`]s to show. Only the last batch's cursor is kept, so a sync
//! cut short by a dropped connection starts again from the old cursor.
//!
//! What the phone sends is held to the protocol's limits ("Limits" and
//! "Keys") before it is kept, so a phone that ignores them cannot grow the
//! desktop's memory or push huge strings on to the desktop's notification
//! service.

use std::collections::BTreeMap;

use link2linux_protocol::v1::{
    NotificationPosted, NotificationRemovalReason, NotificationRemoved, NotificationSyncBatch,
    NotificationSyncRequest, PhoneNotification,
};

use crate::sync::{Advance, SyncCursor, SyncRun};

/// A change to the mirrored notifications.
#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    /// A notification appeared, or replaced the one with the same key.
    Posted {
        /// The notification as the phone shares it.
        notification: Box<PhoneNotification>,
        /// Whether to alert the user. Never set for notifications that
        /// arrive in a sync, which the user may have seen already.
        alert: bool,
    },
    /// A notification is gone from the phone.
    Removed {
        /// Its key.
        key: String,
        /// Why, when the phone said; `UNSPECIFIED` after a sync.
        reason: NotificationRemovalReason,
    },
}

/// A message the mirror cannot apply: the phone broke the sync rules.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MirrorError {
    /// A sync batch arrived with no sync request waiting for it.
    #[error("a notification sync batch arrived without a request")]
    UnexpectedBatch,
    /// A live event arrived before the sync it follows had finished.
    #[error("a live notification event arrived before the sync finished")]
    EventBeforeSync,
    /// A posted notification without the notification, or without a key.
    #[error("a notification without a key")]
    MissingKey,
    /// A key or group longer than the phone may send.
    #[error("a notification key or group over {MAX_KEY_BYTES} bytes")]
    KeyTooLong,
}

/// The longest `key` or `group`; the phone hashes longer ones.
pub const MAX_KEY_BYTES: usize = 256;
/// The most notifications kept for one phone. Android lets each app show
/// 50 at a time, so a real phone stays far below this. Past it, new keys
/// are dropped; ones already kept still update. A snapshot counts only
/// what it lists, since all else goes at its end.
pub const MAX_ITEMS: usize = 1000;

const NAME_BYTES: usize = 256;
const SHORT_TEXT_BYTES: usize = 1024;
const LONG_TEXT_BYTES: usize = 8 * 1024;
const MAX_MESSAGES: usize = 10;
const MAX_ACTIONS: usize = 8;
const MAX_CHOICES: usize = 5;
const ACTION_ID_BYTES: usize = 256;

/// The phone's notifications as the desktop knows them.
#[derive(Debug, Clone, Default)]
pub struct Mirror {
    items: BTreeMap<String, PhoneNotification>,
    cursor: SyncCursor,
    /// The sync in progress, if any.
    run: Option<SyncRun<String>>,
    /// Set once a sync has finished in this session.
    live: bool,
}

impl Mirror {
    /// A mirror that resumes from `cursor`, stored with `items` when the
    /// last session ended. Pass the default cursor and no items to start
    /// over.
    #[must_use]
    pub fn new(cursor: SyncCursor, items: impl IntoIterator<Item = PhoneNotification>) -> Self {
        Self {
            items: items.into_iter().map(|n| (n.key.clone(), n)).collect(),
            cursor,
            run: None,
            live: false,
        }
    }

    /// The cursor to store along with [`Self::items`].
    #[must_use]
    pub fn cursor(&self) -> SyncCursor {
        self.cursor
    }

    /// The mirrored notifications, by key.
    pub fn items(&self) -> impl Iterator<Item = &PhoneNotification> {
        self.items.values()
    }

    /// The notification with `key`, if mirrored.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&PhoneNotification> {
        self.items.get(key)
    }

    /// Starts a sync: the request to send. Live events are refused until
    /// its last batch arrives.
    pub fn sync_request(&mut self) -> NotificationSyncRequest {
        self.run = Some(SyncRun::new());
        self.live = false;
        NotificationSyncRequest {
            since: self.cursor.to_wire(),
        }
    }

    /// Forgets the sync in progress, for example when the session ends.
    /// The items and the cursor stay.
    pub fn session_ended(&mut self) {
        self.run = None;
        self.live = false;
    }

    /// Applies one batch of the answer to [`Self::sync_request`].
    pub fn apply_batch(
        &mut self,
        batch: NotificationSyncBatch,
    ) -> Result<Vec<Change>, MirrorError> {
        let run = self.run.as_mut().ok_or(MirrorError::UnexpectedBatch)?;
        let notifications = batch
            .notifications
            .into_iter()
            .map(within_limits)
            .collect::<Result<Vec<_>, _>>()?;
        // The keys go into the run as they are kept.
        run.batch(batch.reset, std::iter::empty());
        let mut changes = Vec::new();
        for key in batch.removed_keys {
            if self.items.remove(&key).is_some() {
                changes.push(removed(key, NotificationRemovalReason::Unspecified));
            }
        }
        for notification in notifications {
            if !self.keep(&notification) {
                continue;
            }
            changes.push(Change::Posted {
                notification: Box::new(notification),
                alert: false,
            });
        }
        if batch.more {
            return Ok(changes);
        }
        let run = self.run.take().expect("checked above");
        if let Some(keep) = run.finish() {
            let gone: Vec<String> = self
                .items
                .keys()
                .filter(|key| !keep.contains(*key))
                .cloned()
                .collect();
            for key in gone {
                self.items.remove(&key);
                changes.push(removed(key, NotificationRemovalReason::Unspecified));
            }
        }
        self.cursor = SyncCursor::from_wire(batch.cursor);
        self.live = true;
        Ok(changes)
    }

    /// Applies a live `NotificationPosted`.
    pub fn apply_posted(
        &mut self,
        posted: NotificationPosted,
    ) -> Result<Option<Change>, MirrorError> {
        if !self.live {
            return Err(MirrorError::EventBeforeSync);
        }
        let notification = within_limits(posted.notification.ok_or(MirrorError::MissingKey)?)?;
        if self.cursor.advance(notification.revision) == Advance::Stale {
            return Ok(None);
        }
        if !self.keep(&notification) {
            return Ok(None);
        }
        Ok(Some(Change::Posted {
            notification: Box::new(notification),
            alert: posted.alert,
        }))
    }

    /// Applies a live `NotificationRemoved`.
    pub fn apply_removed(
        &mut self,
        event: NotificationRemoved,
    ) -> Result<Option<Change>, MirrorError> {
        if !self.live {
            return Err(MirrorError::EventBeforeSync);
        }
        if self.cursor.advance(event.revision) == Advance::Stale {
            return Ok(None);
        }
        let reason = event.reason();
        Ok(self
            .items
            .remove(&event.key)
            .map(|_| removed(event.key, reason)))
    }
}

impl Mirror {
    /// Keeps `notification`, unless it is new and the mirror is full. In a
    /// snapshot, the mirror holds only what the snapshot listed: one it
    /// did not keep goes at its end, even if the mirror held it before.
    fn keep(&mut self, notification: &PhoneNotification) -> bool {
        let key = &notification.key;
        let fits = match self.run.as_ref().and_then(SyncRun::listed) {
            Some(listed) => listed.contains(key) || listed.len() < MAX_ITEMS,
            None => self.items.contains_key(key) || self.items.len() < MAX_ITEMS,
        };
        if !fits {
            return false;
        }
        if let Some(run) = &mut self.run {
            run.batch(false, [key.clone()]);
        }
        self.items.insert(key.clone(), notification.clone());
        true
    }
}

/// `notification` cut to the protocol's limits: text at a character
/// boundary, extra messages, actions and reply choices dropped. Keys are
/// never cut, since they identify things, so an over-long one is an error.
fn within_limits(mut n: PhoneNotification) -> Result<PhoneNotification, MirrorError> {
    if n.key.is_empty() {
        return Err(MirrorError::MissingKey);
    }
    if n.key.len() > MAX_KEY_BYTES || n.group.len() > MAX_KEY_BYTES {
        return Err(MirrorError::KeyTooLong);
    }
    truncate(&mut n.app_id, NAME_BYTES);
    truncate(&mut n.app_name, NAME_BYTES);
    truncate(&mut n.title, SHORT_TEXT_BYTES);
    truncate(&mut n.sub_text, SHORT_TEXT_BYTES);
    truncate(&mut n.conversation_title, SHORT_TEXT_BYTES);
    truncate(&mut n.text, LONG_TEXT_BYTES);
    // The latest messages are the last ones.
    let extra = n.messages.len().saturating_sub(MAX_MESSAGES);
    n.messages.drain(..extra);
    for message in &mut n.messages {
        truncate(&mut message.sender, NAME_BYTES);
        truncate(&mut message.text, LONG_TEXT_BYTES);
    }
    // An action id is never cut either: one that is too long cannot be
    // asked for, so the action goes.
    n.actions
        .retain(|action| !action.id.is_empty() && action.id.len() <= ACTION_ID_BYTES);
    n.actions.truncate(MAX_ACTIONS);
    for action in &mut n.actions {
        truncate(&mut action.label, SHORT_TEXT_BYTES);
        action.reply_choices.truncate(MAX_CHOICES);
        for choice in &mut action.reply_choices {
            truncate(choice, NAME_BYTES);
        }
    }
    Ok(n)
}

/// Cuts `text` to at most `max` bytes, at a character boundary.
fn truncate(text: &mut String, max: usize) {
    if text.len() > max {
        let mut end = max;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
}

fn removed(key: String, reason: NotificationRemovalReason) -> Change {
    Change::Removed { key, reason }
}

#[cfg(test)]
mod tests {
    use super::*;
    use link2linux_protocol::v1;

    fn note(key: &str, revision: u64) -> PhoneNotification {
        PhoneNotification {
            key: key.into(),
            revision,
            title: format!("title {key}"),
            ..PhoneNotification::default()
        }
    }

    fn cursor(epoch: u64, revision: u64) -> Option<v1::SyncCursor> {
        Some(v1::SyncCursor { epoch, revision })
    }

    fn keys(mirror: &Mirror) -> Vec<&str> {
        mirror.items().map(|n| n.key.as_str()).collect()
    }

    fn batch(
        reset: bool,
        items: Vec<PhoneNotification>,
        removed: &[&str],
        more: bool,
        at: Option<v1::SyncCursor>,
    ) -> NotificationSyncBatch {
        NotificationSyncBatch {
            reset,
            notifications: items,
            removed_keys: removed.iter().map(|k| (*k).into()).collect(),
            cursor: at,
            more,
        }
    }

    #[test]
    fn a_snapshot_keeps_only_what_it_lists_and_stores_the_last_cursor() {
        let mut mirror = Mirror::new(SyncCursor::new(7, 3), [note("old", 1), note("a", 2)]);
        assert_eq!(mirror.sync_request().since, cursor(7, 3));

        let first = mirror
            .apply_batch(batch(true, vec![note("a", 4)], &[], true, cursor(9, 4)))
            .unwrap();
        assert_eq!(first.len(), 1);
        // Not the last batch: the cursor stays until the sync is complete.
        assert_eq!(mirror.cursor(), SyncCursor::new(7, 3));

        let last = mirror
            .apply_batch(batch(false, vec![note("b", 5)], &[], false, cursor(9, 5)))
            .unwrap();
        assert!(last.contains(&removed(
            "old".into(),
            NotificationRemovalReason::Unspecified
        )));
        assert_eq!(keys(&mirror), ["a", "b"]);
        assert_eq!(mirror.cursor(), SyncCursor::new(9, 5));
        assert!(matches!(&last[0], Change::Posted { alert: false, .. }));
    }

    #[test]
    fn a_delta_applies_changes_and_removals() {
        let mut mirror = Mirror::new(SyncCursor::new(9, 5), [note("a", 4), note("b", 5)]);
        mirror.sync_request();
        let changes = mirror
            .apply_batch(batch(
                false,
                vec![note("c", 7)],
                &["a", "never-had"],
                false,
                cursor(9, 7),
            ))
            .unwrap();
        assert_eq!(changes.len(), 2, "removing an unknown key changes nothing");
        assert_eq!(keys(&mirror), ["b", "c"]);
        assert_eq!(mirror.cursor(), SyncCursor::new(9, 7));
    }

    #[test]
    fn a_fresh_mirror_asks_for_everything() {
        let mut mirror = Mirror::default();
        assert_eq!(mirror.sync_request().since, None);
    }

    #[test]
    fn live_events_follow_the_cursor_and_skip_what_is_stale() {
        let mut mirror = Mirror::default();
        mirror.sync_request();
        mirror
            .apply_batch(batch(true, vec![note("a", 3)], &[], false, cursor(1, 3)))
            .unwrap();

        let posted = NotificationPosted {
            notification: Some(note("b", 4)),
            alert: true,
        };
        assert!(matches!(
            mirror.apply_posted(posted.clone()).unwrap(),
            Some(Change::Posted { alert: true, .. })
        ));
        assert_eq!(
            mirror.apply_posted(posted).unwrap(),
            None,
            "revision 4 again is stale"
        );

        let gone = NotificationRemoved {
            key: "a".into(),
            revision: 5,
            reason: NotificationRemovalReason::DismissedFromDesktop.into(),
        };
        assert_eq!(
            mirror.apply_removed(gone).unwrap(),
            Some(removed(
                "a".into(),
                NotificationRemovalReason::DismissedFromDesktop
            ))
        );
        assert_eq!(keys(&mirror), ["b"]);
        assert_eq!(mirror.cursor(), SyncCursor::new(1, 5));
    }

    #[test]
    fn the_phone_must_follow_the_order_of_sync() {
        let mut mirror = Mirror::default();
        assert_eq!(
            mirror.apply_batch(batch(true, vec![], &[], false, cursor(1, 0))),
            Err(MirrorError::UnexpectedBatch)
        );
        mirror.sync_request();
        let early = NotificationPosted {
            notification: Some(note("a", 1)),
            alert: true,
        };
        assert_eq!(
            mirror.apply_posted(early.clone()),
            Err(MirrorError::EventBeforeSync)
        );
        assert_eq!(
            mirror.apply_batch(batch(true, vec![note("", 1)], &[], false, cursor(1, 1))),
            Err(MirrorError::MissingKey)
        );
        mirror
            .apply_batch(batch(true, vec![], &[], false, cursor(1, 1)))
            .unwrap();
        assert_eq!(
            mirror.apply_posted(NotificationPosted {
                notification: None,
                alert: false
            }),
            Err(MirrorError::MissingKey)
        );
        // A dropped session needs a new sync before live events count.
        mirror.session_ended();
        assert_eq!(
            mirror.apply_posted(early),
            Err(MirrorError::EventBeforeSync)
        );
    }

    #[test]
    fn what_the_phone_sends_is_held_to_the_protocol_limits() {
        let mut mirror = Mirror::default();
        mirror.sync_request();
        mirror
            .apply_batch(batch(true, vec![], &[], false, cursor(1, 0)))
            .unwrap();
        let action = |id: String| v1::NotificationAction {
            id,
            label: "é".repeat(4000),
            reply_choices: vec!["yes".into(); 20],
            ..v1::NotificationAction::default()
        };
        let mut big = note("big", 1);
        big.title = "é".repeat(1_000_000);
        big.app_name = "x".repeat(3 * 1024 * 1024);
        big.text = "t".repeat(100_000);
        big.messages = (0..30)
            .map(|i| v1::ConversationMessage {
                text: format!("message {i}"),
                ..v1::ConversationMessage::default()
            })
            .collect();
        big.actions = (0..20).map(|i| action(format!("a:{i}"))).collect();
        big.actions.insert(0, action("x".repeat(300)));
        let Some(Change::Posted { notification, .. }) = mirror
            .apply_posted(NotificationPosted {
                notification: Some(big),
                alert: true,
            })
            .unwrap()
        else {
            panic!("posted");
        };
        // 1024 bytes of two-byte characters, cut at a boundary.
        assert_eq!(notification.title.len(), 1024);
        assert_eq!(notification.app_name.len(), 256);
        assert_eq!(notification.text.len(), 8 * 1024);
        assert_eq!(notification.messages.len(), 10);
        assert_eq!(
            notification.messages[0].text, "message 20",
            "the latest stay"
        );
        assert_eq!(notification.actions.len(), 8);
        assert_eq!(
            notification.actions[0].id, "a:0",
            "an over-long id is dropped"
        );
        assert_eq!(notification.actions[0].label.len(), 1024);
        assert_eq!(notification.actions[0].reply_choices.len(), 5);
        assert_eq!(mirror.get("big"), Some(&*notification));
    }

    #[test]
    fn over_long_keys_are_refused_and_the_mirror_stays_bounded() {
        let mut mirror = Mirror::default();
        mirror.sync_request();
        let mut long_group = note("a", 1);
        long_group.group = "g".repeat(257);
        assert_eq!(
            mirror.apply_batch(batch(true, vec![long_group], &[], false, cursor(1, 1))),
            Err(MirrorError::KeyTooLong)
        );
        let many = (0..MAX_ITEMS as u64 + 5)
            .map(|i| note(&format!("k{i}"), i + 1))
            .collect();
        mirror
            .apply_batch(batch(true, many, &[], false, cursor(1, 2000)))
            .unwrap();
        assert_eq!(mirror.items().count(), MAX_ITEMS);
        let posted = |key: &str, revision| NotificationPosted {
            notification: Some(note(key, revision)),
            alert: true,
        };
        assert_eq!(mirror.apply_posted(posted("new", 2001)).unwrap(), None);
        assert!(
            mirror.apply_posted(posted("k0", 2002)).unwrap().is_some(),
            "a kept notification still updates"
        );
        assert_eq!(mirror.items().count(), MAX_ITEMS);
        assert_eq!(
            mirror.apply_posted(posted(&"k".repeat(257), 2003)),
            Err(MirrorError::KeyTooLong)
        );
    }

    #[test]
    fn a_sync_cut_short_keeps_the_old_cursor() {
        let mut mirror = Mirror::new(SyncCursor::new(2, 8), [note("a", 8)]);
        mirror.sync_request();
        mirror
            .apply_batch(batch(true, vec![note("a", 9)], &[], true, cursor(3, 9)))
            .unwrap();
        mirror.session_ended();
        assert_eq!(mirror.cursor(), SyncCursor::new(2, 8));
        assert_eq!(mirror.sync_request().since, cursor(2, 8));
    }

    #[test]
    fn a_snapshot_while_full_replaces_what_the_mirror_held() {
        let mut mirror = Mirror::default();
        mirror.sync_request();
        let old = (0..MAX_ITEMS as u64)
            .map(|i| note(&format!("old{i}"), i + 1))
            .collect();
        mirror
            .apply_batch(batch(true, old, &[], false, cursor(1, 2000)))
            .unwrap();
        assert_eq!(mirror.items().count(), MAX_ITEMS);

        // The phone's listener restarted: a snapshot of three new
        // notifications, none of the old ones.
        mirror.sync_request();
        let fresh = vec![note("new1", 1), note("new2", 2), note("new3", 3)];
        mirror
            .apply_batch(batch(true, fresh, &[], false, cursor(2, 3)))
            .unwrap();
        assert_eq!(keys(&mirror), ["new1", "new2", "new3"]);

        // A snapshot that lists more than fit keeps the first that do, in
        // two batches, and nothing past them.
        mirror.sync_request();
        let first = (0..MAX_ITEMS as u64 - 1)
            .map(|i| note(&format!("more{i:04}"), i + 1))
            .collect();
        let second = vec![note("new1", 2000), note("last", 2001), note("over", 2002)];
        mirror
            .apply_batch(batch(true, first, &[], true, None))
            .unwrap();
        mirror
            .apply_batch(batch(false, second, &[], false, cursor(3, 2002)))
            .unwrap();
        assert_eq!(mirror.items().count(), MAX_ITEMS);
        assert_eq!(mirror.get("new1").unwrap().revision, 2000);
        assert!(mirror.get("last").is_none() && mirror.get("over").is_none());
        assert!(mirror.get("new2").is_none());
    }
}
