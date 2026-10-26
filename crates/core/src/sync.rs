//! Delta sync: following a collection that lives on the other device.
//!
//! The device that owns a collection, such as the phone's notifications,
//! numbers each change with the next revision of the collection, within an
//! epoch that names one run of its change log. The other device follows it
//! with a [`SyncCursor`]: the owner's epoch and the highest revision it has
//! applied. `docs/protocol.md` ("Delta sync") gives the rules.
//!
//! - The follower sends its cursor in the collection's sync request. The
//!   owner picks a [`SyncAnswer`] with [`SyncCursor::answer`]: a delta when
//!   it can, otherwise a snapshot.
//! - The follower takes the batches through a [`SyncRun`], which tells it,
//!   after a snapshot, which items to keep. It stores only the last batch's
//!   cursor, in the same transaction as the items.
//! - Each live event that follows carries its revision, which moves the
//!   cursor on through [`SyncCursor::advance`].

use std::cmp::Ordering;
use std::collections::BTreeSet;

use link2linux_protocol::v1;

/// A follower's position in a collection's change log.
///
/// Cursors compare only within one epoch: a cursor from another epoch is
/// neither older nor newer, because the owner started its log again.
///
/// ```
/// use link2linux_core::sync::SyncCursor;
///
/// let cursor = SyncCursor::new(7, 41);
/// assert!(cursor < SyncCursor::new(7, 42));
/// assert_eq!(cursor.partial_cmp(&SyncCursor::new(8, 1)), None);
/// assert!(SyncCursor::NONE.is_none());
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct SyncCursor {
    /// The run of the change log, a random non-zero number the owner picks.
    /// 0 means no cursor.
    pub epoch: u64,
    /// The highest revision the follower has applied in that run.
    pub revision: u64,
}

impl SyncCursor {
    /// No cursor: the follower has nothing yet, and gets a snapshot.
    pub const NONE: Self = Self {
        epoch: 0,
        revision: 0,
    };

    /// The cursor at `revision` in `epoch`.
    #[must_use]
    pub const fn new(epoch: u64, revision: u64) -> Self {
        Self { epoch, revision }
    }

    /// Whether this is [`SyncCursor::NONE`], or any cursor without an
    /// epoch.
    #[must_use]
    pub const fn is_none(self) -> bool {
        self.epoch == 0
    }

    /// Whether both cursors are in the same run of the change log.
    #[must_use]
    pub const fn same_epoch(self, other: Self) -> bool {
        !self.is_none() && self.epoch == other.epoch
    }

    /// The cursor for a sync request's `since` field, which stays unset
    /// without a cursor.
    #[must_use]
    pub fn to_wire(self) -> Option<v1::SyncCursor> {
        (!self.is_none()).then(|| self.into())
    }

    /// Reads a cursor field, unset meaning [`SyncCursor::NONE`].
    #[must_use]
    pub fn from_wire(cursor: Option<v1::SyncCursor>) -> Self {
        cursor.map_or(Self::NONE, Self::from)
    }

    /// Follower side: takes the revision of a live event in this cursor's
    /// epoch. A revision above the cursor moves it there and is
    /// [`Advance::Applied`]; one at or below it was applied already and is
    /// [`Advance::Stale`].
    pub fn advance(&mut self, revision: u64) -> Advance {
        if revision > self.revision {
            self.revision = revision;
            Advance::Applied
        } else {
            Advance::Stale
        }
    }

    /// Owner side: how to answer a follower that sends `since`, when the
    /// owner's log is at `self` and it may have dropped the tombstones of
    /// removals up to revision `pruned_through` (0 when it kept them all).
    ///
    /// A delta needs the same epoch, a follower that is not ahead of the
    /// owner, and every tombstone after the follower's revision.
    #[must_use]
    pub fn answer(self, since: Self, pruned_through: u64) -> SyncAnswer {
        if since.same_epoch(self)
            && since.revision <= self.revision
            && since.revision >= pruned_through
        {
            SyncAnswer::Delta {
                after: since.revision,
            }
        } else {
            SyncAnswer::Snapshot
        }
    }
}

impl PartialOrd for SyncCursor {
    /// Orders cursors of the same epoch by revision. Cursors of different
    /// epochs are not comparable.
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        (self.epoch == other.epoch).then(|| self.revision.cmp(&other.revision))
    }
}

impl From<v1::SyncCursor> for SyncCursor {
    fn from(cursor: v1::SyncCursor) -> Self {
        Self::new(cursor.epoch, cursor.revision)
    }
}

impl From<SyncCursor> for v1::SyncCursor {
    fn from(cursor: SyncCursor) -> Self {
        Self {
            epoch: cursor.epoch,
            revision: cursor.revision,
        }
    }
}

/// What a live event's revision did to a follower's cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Advance {
    /// A change after the cursor: apply it. The cursor moved.
    Applied,
    /// A change the follower already has: skip it.
    Stale,
}

/// How the owner answers a sync request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncAnswer {
    /// The items changed after revision `after`, and the keys of items
    /// removed after it.
    Delta {
        /// The follower's revision.
        after: u64,
    },
    /// Every item, with `reset` set on the first batch.
    Snapshot,
}

/// A follower's bookkeeping for the batches that answer one sync request.
///
/// After a snapshot, the follower keeps only the items the snapshot listed;
/// [`SyncRun::finish`] returns their keys. After a delta, it returns `None`:
/// the batches already said what changed and what was removed.
///
/// ```
/// use link2linux_core::sync::SyncRun;
///
/// let mut run = SyncRun::new();
/// run.batch(true, ["a", "b"]);
/// run.batch(false, ["c"]);
/// let keep = run.finish().unwrap();
/// assert!(keep.contains("a") && keep.contains("c") && !keep.contains("z"));
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncRun<K> {
    /// The keys of a snapshot so far, once a batch had `reset` set.
    snapshot: Option<BTreeSet<K>>,
}

impl<K: Ord> SyncRun<K> {
    /// A run before its first batch.
    #[must_use]
    pub fn new() -> Self {
        Self { snapshot: None }
    }

    /// Takes one batch: its `reset` flag and the keys of the items it
    /// carries. A batch with `reset` starts a snapshot, dropping what
    /// earlier batches of the run listed.
    pub fn batch(&mut self, reset: bool, keys: impl IntoIterator<Item = K>) {
        if reset {
            self.snapshot = Some(BTreeSet::new());
        }
        if let Some(snapshot) = &mut self.snapshot {
            snapshot.extend(keys);
        }
    }

    /// Whether the run turned out to be a snapshot.
    #[must_use]
    pub fn is_snapshot(&self) -> bool {
        self.snapshot.is_some()
    }

    /// Ends the run after its last batch. For a snapshot, returns the keys
    /// of the items to keep; every other mirrored item is gone.
    #[must_use]
    pub fn finish(self) -> Option<BTreeSet<K>> {
        self.snapshot
    }
}

impl<K: Ord> Default for SyncRun<K> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_within_an_epoch_only() {
        let a = SyncCursor::new(5, 10);
        assert!(a < SyncCursor::new(5, 11));
        assert!(a > SyncCursor::new(5, 9));
        assert_eq!(a.partial_cmp(&a), Some(Ordering::Equal));
        assert_eq!(a.partial_cmp(&SyncCursor::new(6, 10)), None);
        assert_eq!(a.partial_cmp(&SyncCursor::NONE), None);
        assert!(a.same_epoch(SyncCursor::new(5, 0)));
        assert!(!SyncCursor::NONE.same_epoch(SyncCursor::NONE));
    }

    #[test]
    fn converts_to_and_from_the_wire() {
        assert_eq!(SyncCursor::NONE.to_wire(), None);
        assert_eq!(SyncCursor::from_wire(None), SyncCursor::NONE);
        let cursor = SyncCursor::new(u64::MAX, 3);
        let wire = cursor.to_wire().unwrap();
        assert_eq!((wire.epoch, wire.revision), (u64::MAX, 3));
        assert_eq!(SyncCursor::from_wire(Some(wire)), cursor);
    }

    #[test]
    fn live_events_move_the_cursor_forward() {
        let mut cursor = SyncCursor::new(5, 10);
        assert_eq!(cursor.advance(11), Advance::Applied);
        assert_eq!(cursor.advance(11), Advance::Stale);
        assert_eq!(cursor.advance(4), Advance::Stale);
        // A gap is applied too; events of one connection are never lost.
        assert_eq!(cursor.advance(20), Advance::Applied);
        assert_eq!(cursor, SyncCursor::new(5, 20));
    }

    #[test]
    fn the_owner_sends_a_delta_only_when_it_can() {
        let owner = SyncCursor::new(5, 100);
        let delta = |after| SyncAnswer::Delta { after };
        assert_eq!(owner.answer(SyncCursor::new(5, 60), 0), delta(60));
        assert_eq!(owner.answer(SyncCursor::new(5, 100), 0), delta(100));
        assert_eq!(owner.answer(SyncCursor::new(5, 60), 60), delta(60));
        // Tombstones after 60 are gone.
        assert_eq!(
            owner.answer(SyncCursor::new(5, 60), 61),
            SyncAnswer::Snapshot
        );
        // Another run of the log, no cursor, or a follower ahead of the owner.
        assert_eq!(
            owner.answer(SyncCursor::new(4, 60), 0),
            SyncAnswer::Snapshot
        );
        assert_eq!(owner.answer(SyncCursor::NONE, 0), SyncAnswer::Snapshot);
        assert_eq!(
            owner.answer(SyncCursor::new(5, 101), 0),
            SyncAnswer::Snapshot
        );
    }

    #[test]
    fn a_delta_run_keeps_everything() {
        let mut run = SyncRun::new();
        run.batch(false, [1, 2]);
        run.batch(false, [3]);
        assert!(!run.is_snapshot());
        assert_eq!(run.finish(), None);
    }

    #[test]
    fn a_snapshot_run_lists_what_to_keep() {
        let mut run = SyncRun::new();
        run.batch(true, [1, 2]);
        run.batch(false, [3]);
        assert!(run.is_snapshot());
        assert_eq!(run.finish(), Some(BTreeSet::from([1, 2, 3])));

        // A reset restarts the snapshot.
        let mut run = SyncRun::new();
        run.batch(true, [1]);
        run.batch(true, [2]);
        assert_eq!(run.finish(), Some(BTreeSet::from([2])));
    }
}
