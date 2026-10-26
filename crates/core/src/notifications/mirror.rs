//! The desktop's copy of the phone's notifications, kept in step through
//! delta sync (`docs/protocol.md`, "Delta sync" and "Notifications").
//!
//! The desktop asks for the changes since its [`SyncCursor`] with
//! [`Mirror::sync_request`], applies the answering batches with
//! [`Mirror::apply_batch`], and from then on the phone's live events with
//! [`Mirror::apply_posted`] and [`Mirror::apply_removed`]. Each call returns
//! the [`Change`]s to show. Only the last batch's cursor is kept, so a sync
//! cut short by a dropped connection starts again from the old cursor.

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
}

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
        if batch.notifications.iter().any(|n| n.key.is_empty()) {
            return Err(MirrorError::MissingKey);
        }
        run.batch(
            batch.reset,
            batch.notifications.iter().map(|n| n.key.clone()),
        );
        let mut changes = Vec::new();
        for key in batch.removed_keys {
            if self.items.remove(&key).is_some() {
                changes.push(removed(key, NotificationRemovalReason::Unspecified));
            }
        }
        for notification in batch.notifications {
            self.items
                .insert(notification.key.clone(), notification.clone());
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
        let notification = posted.notification.ok_or(MirrorError::MissingKey)?;
        if notification.key.is_empty() {
            return Err(MirrorError::MissingKey);
        }
        if self.cursor.advance(notification.revision) == Advance::Stale {
            return Ok(None);
        }
        self.items
            .insert(notification.key.clone(), notification.clone());
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
}
