//! The durable outbox for actions the user takes on this device that have
//! to happen on the other one, such as dismissing a notification.
//!
//! An action is stored before it is sent, with an [`IdempotencyKey`] that
//! stays the same on every retry, so the other device carries it out once
//! however often it arrives. It is sent whenever a session is up, again
//! after each reconnect, and again after a retryable refusal, until an
//! answer ends it or it expires. `docs/protocol.md` ("Outbox") gives the
//! rules.
//!
//! [`OutboxStore`] is the storage, which a durable implementation keeps on
//! disk; [`MemoryOutboxStore`] keeps it in memory. [`Outbox`] holds the
//! rules on top of a store:
//!
//! 1. [`Outbox::push`] stores the action when the user takes it.
//! 2. While a session is up, [`Outbox::take_due`] hands out what to send
//!    now, and each answer goes back through [`Outbox::acked`] or
//!    [`Outbox::refused`], or [`Outbox::unanswered`] when the request timed
//!    out. [`Outbox::session_ended`] makes whatever was in flight go out
//!    again with the next session.
//! 3. [`Outbox::expire`] retires the actions that waited too long, so the
//!    user can be told they were not delivered.
//!
//! ```
//! use std::time::SystemTime;
//!
//! use link2linux_core::outbox::{MemoryOutboxStore, Outbox, Outcome};
//! use link2linux_protocol::Payload;
//! use link2linux_protocol::v1::NotificationDismissRequest;
//!
//! let mut outbox = Outbox::new(MemoryOutboxStore::default());
//! let now = SystemTime::now();
//! let dismiss = NotificationDismissRequest {
//!     key: "0|com.example|1|null|10001".into(),
//!     ..NotificationDismissRequest::default()
//! };
//! let key = outbox.push(Payload::NotificationDismissRequest(dismiss), now)?;
//!
//! // A session is up and notifications are ready on both devices.
//! let due = outbox.take_due(now, |_| true);
//! assert_eq!(due.len(), 1);
//! // ... send due[0].request with Session::send_request, then:
//! assert_eq!(outbox.acked(&key)?, Outcome::Done);
//! assert!(outbox.pending().is_empty());
//! # Ok::<(), link2linux_core::outbox::OutboxError>(())
//! ```

use std::collections::hash_map::RandomState;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::hash::{BuildHasher, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use link2linux_protocol::Payload;
use link2linux_protocol::v1::{ErrorCode, ErrorReply};

use crate::device::Feature;

/// How long an action waits for delivery unless its feature says
/// otherwise.
pub const DEFAULT_TTL: Duration = Duration::from_secs(60 * 60);

/// How long an action of `feature` waits for delivery before the user is
/// told it was not delivered. Notifications use 10 minutes, since a stale
/// dismissal or reply surprises more than a lost one.
#[must_use]
pub fn ttl_for(feature: Feature) -> Duration {
    match feature {
        Feature::Notifications => Duration::from_secs(10 * 60),
        _ => DEFAULT_TTL,
    }
}

/// An entry ends after a refusal with `INTERNAL` once it has been tried
/// this many times without success.
pub const MAX_ATTEMPTS_AFTER_INTERNAL: u32 = 3;

/// The wait before retrying an action that failed `attempts` times:
/// 1 second, doubling up to 60 seconds.
#[must_use]
pub fn backoff(attempts: u32) -> Duration {
    let doublings = attempts.saturating_sub(1).min(6);
    Duration::from_secs(1 << doublings).min(Duration::from_secs(60))
}

/// 16 bytes that name one action across all its retries. The receiver
/// remembers the keys it carried out, and answers a repeat without carrying
/// it out again.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct IdempotencyKey([u8; 16]);

impl IdempotencyKey {
    /// A new key, unique on this device and not guessable from outside the
    /// process: it hashes a counter and the time with keys the standard
    /// library seeds from the OS's random source. It is not meant for
    /// secrets.
    #[must_use]
    pub fn random() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let count = COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        let half = |index: u8| {
            // Every RandomState gets keys of its own.
            let mut hasher = RandomState::new().build_hasher();
            hasher.write_u64(count);
            hasher.write_u128(nanos);
            hasher.write_u8(index);
            u128::from(hasher.finish())
        };
        Self(((half(1) << 64) | half(0)).to_le_bytes())
    }

    /// A key with these bytes.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    /// Reads a key from a wire field. `None` unless it holds 16 bytes.
    #[must_use]
    pub fn from_slice(bytes: &[u8]) -> Option<Self> {
        bytes.try_into().ok().map(Self)
    }

    /// The key's bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }
}

impl fmt::Display for IdempotencyKey {
    /// Lowercase hex.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.iter().try_for_each(|byte| write!(f, "{byte:02x}"))
    }
}

impl fmt::Debug for IdempotencyKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "IdempotencyKey({self})")
    }
}

/// One action in the outbox.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct OutboxEntry {
    /// Names the action on every retry; the entry's id in the store.
    pub key: IdempotencyKey,
    /// The feature the request belongs to.
    pub feature: Feature,
    /// The request, with its idempotency key and creation time filled in.
    pub request: Payload,
    /// When the user acted.
    pub created_at: SystemTime,
    /// When the action stops waiting and the user is told it was not
    /// delivered.
    pub expires_at: SystemTime,
    /// How many times the request went unanswered or got an answer that
    /// asked for a retry.
    pub attempts: u32,
    /// Not sent again before this time, after a refusal that asked to wait
    /// or a request that went unanswered.
    pub retry_at: Option<SystemTime>,
    /// Kept back until the feature is ready again on the other device,
    /// after an answer of `DISABLED` or `PERMISSION_REQUIRED`.
    pub waiting_for_ready: bool,
}

/// Why the outbox could not take or update an action.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum OutboxError {
    /// The message is not a request that goes through the outbox.
    #[error("{0} does not go through the outbox")]
    NotAnOutboxRequest(&'static str),
    /// No entry has this key: its action ended or expired already.
    #[error("no outbox entry {0}")]
    UnknownEntry(IdempotencyKey),
    /// The store failed to read or write.
    #[error("outbox storage failed: {0}")]
    Store(String),
}

/// What became of an action after an answer, or the lack of one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Outcome {
    /// The other device carried it out. The entry is gone.
    Done,
    /// The other device refused it for good, with this code. The entry is
    /// gone, and the user should see the outcome.
    Failed(ErrorCode),
    /// Kept, and sent again from this time on.
    RetryAt(SystemTime),
    /// Kept until the feature is ready again on the other device.
    WaitForReady,
}

/// Storage for outbox entries, keyed by [`IdempotencyKey`].
///
/// A durable implementation writes each change through before it returns,
/// so an action the user took survives a restart of the app.
pub trait OutboxStore {
    /// Stores `entry`, replacing the entry with the same key.
    ///
    /// # Errors
    ///
    /// Fails when the store cannot be written.
    fn put(&mut self, entry: OutboxEntry) -> Result<(), OutboxError>;

    /// Removes the entry with `key`, returning it if there was one.
    ///
    /// # Errors
    ///
    /// Fails when the store cannot be written.
    fn remove(&mut self, key: &IdempotencyKey) -> Result<Option<OutboxEntry>, OutboxError>;

    /// The entry with `key`, if any.
    fn get(&self, key: &IdempotencyKey) -> Option<OutboxEntry>;

    /// Every entry, oldest first.
    fn entries(&self) -> Vec<OutboxEntry>;
}

/// An [`OutboxStore`] that lives in memory only, for tests and for running
/// before durable storage is wired up.
#[derive(Debug, Clone, Default)]
pub struct MemoryOutboxStore {
    entries: BTreeMap<IdempotencyKey, OutboxEntry>,
}

impl OutboxStore for MemoryOutboxStore {
    fn put(&mut self, entry: OutboxEntry) -> Result<(), OutboxError> {
        self.entries.insert(entry.key, entry);
        Ok(())
    }

    fn remove(&mut self, key: &IdempotencyKey) -> Result<Option<OutboxEntry>, OutboxError> {
        Ok(self.entries.remove(key))
    }

    fn get(&self, key: &IdempotencyKey) -> Option<OutboxEntry> {
        self.entries.get(key).cloned()
    }

    fn entries(&self) -> Vec<OutboxEntry> {
        let mut entries: Vec<_> = self.entries.values().cloned().collect();
        entries.sort_by_key(|entry| (entry.created_at, entry.key));
        entries
    }
}

/// Fills in the idempotency key and creation time of an outbox request,
/// and returns the feature it belongs to.
fn stamp(
    request: &mut Payload,
    key: IdempotencyKey,
    created_at: SystemTime,
) -> Result<Feature, OutboxError> {
    let created_unix_ms = created_at.duration_since(UNIX_EPOCH).map_or(0, |since| {
        i64::try_from(since.as_millis()).unwrap_or(i64::MAX)
    });
    let idempotency_key = key.as_bytes().to_vec();
    match request {
        Payload::NotificationDismissRequest(dismiss) => {
            dismiss.idempotency_key = idempotency_key;
            dismiss.created_unix_ms = created_unix_ms;
            Ok(Feature::Notifications)
        }
        Payload::NotificationActionRequest(action) => {
            action.idempotency_key = idempotency_key;
            action.created_unix_ms = created_unix_ms;
            Ok(Feature::Notifications)
        }
        other => Err(OutboxError::NotAnOutboxRequest(payload_name(other))),
    }
}

/// A short name for a payload in errors.
fn payload_name(payload: &Payload) -> &'static str {
    match payload {
        Payload::Hello(_) | Payload::Ping(_) | Payload::Pong(_) | Payload::Goodbye(_) => {
            "a session message"
        }
        Payload::Ack(_) | Payload::ErrorReply(_) => "an answer",
        _ => "this message",
    }
}

/// The outbox: the rules for storing, sending and retiring actions, on top
/// of an [`OutboxStore`].
#[derive(Debug, Default)]
pub struct Outbox<S> {
    store: S,
    /// Entries sent in the current session that wait for their answer.
    in_flight: BTreeSet<IdempotencyKey>,
}

impl<S: OutboxStore> Outbox<S> {
    /// An outbox over `store`, with the entries it already holds.
    pub fn new(store: S) -> Self {
        Self {
            store,
            in_flight: BTreeSet::new(),
        }
    }

    /// The store.
    pub fn store(&self) -> &S {
        &self.store
    }

    /// Stores a new action the user took at `now`: gives `request` a new
    /// idempotency key and its creation time, and keeps it until an answer
    /// ends it or it expires ([`ttl_for`] its feature). Returns the key.
    ///
    /// # Errors
    ///
    /// Fails with [`OutboxError::NotAnOutboxRequest`] for a message that
    /// does not go through the outbox (today `NotificationDismissRequest`
    /// and `NotificationActionRequest` do), and when the store fails.
    pub fn push(
        &mut self,
        request: Payload,
        now: SystemTime,
    ) -> Result<IdempotencyKey, OutboxError> {
        self.push_with_key(request, IdempotencyKey::random(), now)
    }

    /// Like [`Outbox::push`], with a key the caller made.
    ///
    /// # Errors
    ///
    /// As for [`Outbox::push`].
    pub fn push_with_key(
        &mut self,
        mut request: Payload,
        key: IdempotencyKey,
        now: SystemTime,
    ) -> Result<IdempotencyKey, OutboxError> {
        let feature = stamp(&mut request, key, now)?;
        self.store.put(OutboxEntry {
            key,
            feature,
            request,
            created_at: now,
            expires_at: now + ttl_for(feature),
            attempts: 0,
            retry_at: None,
            waiting_for_ready: false,
        })?;
        Ok(key)
    }

    /// The entries still waiting, oldest first, for showing them as
    /// pending.
    pub fn pending(&self) -> Vec<OutboxEntry> {
        self.store.entries()
    }

    /// The entries to send now, oldest first, which then count as in flight
    /// until their answer, or the lack of one, is recorded. An entry is due
    /// unless it is in flight, expired, waiting for its retry time, or
    /// waiting for its feature, and `is_ready` says its feature is ready on
    /// both devices (see `Session::is_ready`).
    pub fn take_due(
        &mut self,
        now: SystemTime,
        is_ready: impl Fn(Feature) -> bool,
    ) -> Vec<OutboxEntry> {
        let due: Vec<_> = self
            .store
            .entries()
            .into_iter()
            .filter(|entry| {
                !self.in_flight.contains(&entry.key)
                    && now < entry.expires_at
                    && !entry.waiting_for_ready
                    && entry.retry_at.is_none_or(|at| at <= now)
                    && is_ready(entry.feature)
            })
            .collect();
        self.in_flight.extend(due.iter().map(|entry| entry.key));
        due
    }

    /// Records that the other device answered the entry `key` with `Ack`.
    ///
    /// # Errors
    ///
    /// Fails with [`OutboxError::UnknownEntry`] when the entry is gone, and
    /// when the store fails.
    pub fn acked(&mut self, key: &IdempotencyKey) -> Result<Outcome, OutboxError> {
        self.in_flight.remove(key);
        self.store
            .remove(key)?
            .ok_or(OutboxError::UnknownEntry(*key))?;
        Ok(Outcome::Done)
    }

    /// Records that the other device refused the entry `key` with `error`
    /// at `now`, and decides what happens to it:
    ///
    /// - `NOT_FOUND`, `UNSUPPORTED`, `INVALID_ARGUMENT`, `DECLINED`,
    ///   `EXPIRED` and `NOT_ALLOWED` end it: retrying cannot help.
    /// - `BUSY` and `UNAVAILABLE` keep it for a retry after
    ///   `retry_after_ms`, or after the [`backoff`].
    /// - `DISABLED` and `PERMISSION_REQUIRED` keep it until
    ///   [`Outbox::feature_ready`].
    /// - `INTERNAL`, and codes this build does not know, retry it after the
    ///   [`backoff`] until it has failed [`MAX_ATTEMPTS_AFTER_INTERNAL`]
    ///   times.
    ///
    /// # Errors
    ///
    /// Fails with [`OutboxError::UnknownEntry`] when the entry is gone, and
    /// when the store fails.
    pub fn refused(
        &mut self,
        key: &IdempotencyKey,
        error: &ErrorReply,
        now: SystemTime,
    ) -> Result<Outcome, OutboxError> {
        self.in_flight.remove(key);
        let mut entry = self.store.get(key).ok_or(OutboxError::UnknownEntry(*key))?;
        let code = error.code();
        match code {
            ErrorCode::NotFound
            | ErrorCode::Unsupported
            | ErrorCode::InvalidArgument
            | ErrorCode::Declined
            | ErrorCode::Expired
            | ErrorCode::NotAllowed
            | ErrorCode::NoSpace => {
                self.store.remove(key)?;
                Ok(Outcome::Failed(code))
            }
            ErrorCode::Disabled | ErrorCode::PermissionRequired => {
                entry.waiting_for_ready = true;
                self.store.put(entry)?;
                Ok(Outcome::WaitForReady)
            }
            ErrorCode::Busy | ErrorCode::Unavailable => {
                let wait = Duration::from_millis(error.retry_after_ms.into());
                self.retry(entry, Some(wait).filter(|wait| !wait.is_zero()), now)
            }
            ErrorCode::Internal | ErrorCode::Unspecified => {
                if entry.attempts + 1 >= MAX_ATTEMPTS_AFTER_INTERNAL {
                    self.store.remove(key)?;
                    Ok(Outcome::Failed(ErrorCode::Internal))
                } else {
                    self.retry(entry, None, now)
                }
            }
        }
    }

    /// Records that the request for the entry `key` got no answer in time,
    /// and keeps the entry for a retry after the [`backoff`]. The other
    /// device may have carried it out; the idempotency key makes a retry
    /// safe.
    ///
    /// # Errors
    ///
    /// Fails with [`OutboxError::UnknownEntry`] when the entry is gone, and
    /// when the store fails.
    pub fn unanswered(
        &mut self,
        key: &IdempotencyKey,
        now: SystemTime,
    ) -> Result<Outcome, OutboxError> {
        self.in_flight.remove(key);
        let entry = self.store.get(key).ok_or(OutboxError::UnknownEntry(*key))?;
        self.retry(entry, None, now)
    }

    /// The session ended: requests in flight get no answer now, and go out
    /// again with the next session.
    pub fn session_ended(&mut self) {
        self.in_flight.clear();
    }

    /// The other device reports `feature` as ready again: entries kept back
    /// by `DISABLED` or `PERMISSION_REQUIRED` may go out again.
    ///
    /// # Errors
    ///
    /// Fails when the store fails.
    pub fn feature_ready(&mut self, feature: Feature) -> Result<(), OutboxError> {
        for mut entry in self.store.entries() {
            if entry.feature == feature && entry.waiting_for_ready {
                entry.waiting_for_ready = false;
                self.store.put(entry)?;
            }
        }
        Ok(())
    }

    /// Removes the entries that expired by `now` and returns them, so the
    /// user can be told those actions were not delivered.
    ///
    /// # Errors
    ///
    /// Fails when the store fails.
    pub fn expire(&mut self, now: SystemTime) -> Result<Vec<OutboxEntry>, OutboxError> {
        let expired: Vec<_> = self
            .store
            .entries()
            .into_iter()
            .filter(|entry| entry.expires_at <= now)
            .collect();
        for entry in &expired {
            self.in_flight.remove(&entry.key);
            self.store.remove(&entry.key)?;
        }
        Ok(expired)
    }

    /// The next time an entry becomes due or expires, for a timer. Entries
    /// in flight or waiting for their feature only count by their expiry.
    pub fn next_wakeup(&self) -> Option<SystemTime> {
        self.store
            .entries()
            .iter()
            .flat_map(|entry| {
                let retry = entry
                    .retry_at
                    .filter(|_| !entry.waiting_for_ready && !self.in_flight.contains(&entry.key));
                [retry, Some(entry.expires_at)]
            })
            .flatten()
            .min()
    }

    /// Keeps `entry` for another attempt, after `wait` or the backoff.
    fn retry(
        &mut self,
        mut entry: OutboxEntry,
        wait: Option<Duration>,
        now: SystemTime,
    ) -> Result<Outcome, OutboxError> {
        entry.attempts += 1;
        let at = now + wait.unwrap_or_else(|| backoff(entry.attempts));
        entry.retry_at = Some(at);
        self.store.put(entry)?;
        Ok(Outcome::RetryAt(at))
    }
}

#[cfg(test)]
mod tests {
    use link2linux_protocol::v1::{Ack, NotificationActionRequest, NotificationDismissRequest};

    use super::*;

    fn dismiss(key: &str) -> Payload {
        Payload::NotificationDismissRequest(NotificationDismissRequest {
            key: key.into(),
            ..NotificationDismissRequest::default()
        })
    }

    fn at(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(1_700_000_000 + secs)
    }

    #[test]
    fn keys_are_unique_and_read_back() {
        let a = IdempotencyKey::random();
        let b = IdempotencyKey::random();
        assert_ne!(a, b);
        assert_eq!(IdempotencyKey::from_slice(a.as_bytes()), Some(a));
        assert_eq!(IdempotencyKey::from_slice(&[0; 15]), None);
        let key = IdempotencyKey::from_bytes([0xab; 16]);
        assert_eq!(key.to_string(), "ab".repeat(16));
    }

    #[test]
    fn push_stamps_and_stores_the_request() {
        let mut outbox = Outbox::new(MemoryOutboxStore::default());
        let key = IdempotencyKey::from_bytes([1; 16]);
        outbox.push_with_key(dismiss("n1"), key, at(5)).unwrap();

        let entry = outbox.store().get(&key).unwrap();
        assert_eq!(entry.feature, Feature::Notifications);
        assert_eq!(entry.expires_at, at(5) + Duration::from_secs(600));
        let Payload::NotificationDismissRequest(request) = entry.request else {
            panic!("expected the dismissal, got {:?}", entry.request);
        };
        assert_eq!(request.idempotency_key, vec![1; 16]);
        assert_eq!(request.created_unix_ms, 1_700_000_005_000);
        assert_eq!(request.key, "n1");
    }

    #[test]
    fn only_outbox_requests_go_in() {
        let mut outbox = Outbox::new(MemoryOutboxStore::default());
        let action = Payload::NotificationActionRequest(NotificationActionRequest::default());
        outbox.push(action, at(0)).unwrap();
        let err = outbox.push(Payload::Ack(Ack {}), at(0)).unwrap_err();
        assert!(matches!(err, OutboxError::NotAnOutboxRequest(_)), "{err:?}");
        assert_eq!(outbox.pending().len(), 1);
    }

    #[test]
    fn pending_lists_the_oldest_first() {
        let mut outbox = Outbox::new(MemoryOutboxStore::default());
        let late = outbox.push(dismiss("late"), at(9)).unwrap();
        let early = outbox.push(dismiss("early"), at(1)).unwrap();
        let keys: Vec<_> = outbox.pending().iter().map(|entry| entry.key).collect();
        assert_eq!(keys, [early, late]);
    }

    fn refusal(code: ErrorCode, retry_after_ms: u32) -> ErrorReply {
        ErrorReply {
            code: code.into(),
            retry_after_ms,
            ..ErrorReply::default()
        }
    }

    /// An outbox with one dismissal pushed at `at(0)`, already taken as due.
    fn one_in_flight() -> (Outbox<MemoryOutboxStore>, IdempotencyKey) {
        let mut outbox = Outbox::new(MemoryOutboxStore::default());
        let key = outbox.push(dismiss("n1"), at(0)).unwrap();
        assert_eq!(outbox.take_due(at(0), |_| true).len(), 1);
        (outbox, key)
    }

    #[test]
    fn hands_each_entry_out_once_per_session() {
        let (mut outbox, key) = one_in_flight();
        assert!(outbox.take_due(at(1), |_| true).is_empty());
        outbox.session_ended();
        let due = outbox.take_due(at(1), |_| true);
        assert_eq!(due.iter().map(|e| e.key).collect::<Vec<_>>(), [key]);
        // Not while the feature is not ready on both devices.
        outbox.session_ended();
        assert!(outbox.take_due(at(1), |_| false).is_empty());
    }

    #[test]
    fn an_ack_ends_the_entry() {
        let (mut outbox, key) = one_in_flight();
        assert_eq!(outbox.acked(&key).unwrap(), Outcome::Done);
        assert!(outbox.pending().is_empty());
        let err = outbox.acked(&key).unwrap_err();
        assert!(
            matches!(err, OutboxError::UnknownEntry(k) if k == key),
            "{err:?}"
        );
    }

    #[test]
    fn final_refusals_end_the_entry() {
        for code in [
            ErrorCode::NotFound,
            ErrorCode::Unsupported,
            ErrorCode::InvalidArgument,
            ErrorCode::Declined,
            ErrorCode::Expired,
            ErrorCode::NotAllowed,
            ErrorCode::NoSpace,
        ] {
            let (mut outbox, key) = one_in_flight();
            let outcome = outbox.refused(&key, &refusal(code, 0), at(1)).unwrap();
            assert_eq!(outcome, Outcome::Failed(code));
            assert!(outbox.pending().is_empty(), "{code:?}");
        }
    }

    #[test]
    fn busy_waits_as_long_as_asked() {
        let (mut outbox, key) = one_in_flight();
        let busy = refusal(ErrorCode::Busy, 5_000);
        let outcome = outbox.refused(&key, &busy, at(1)).unwrap();
        assert_eq!(outcome, Outcome::RetryAt(at(6)));
        assert_eq!(outbox.next_wakeup(), Some(at(6)));
        assert!(outbox.take_due(at(5), |_| true).is_empty());
        assert_eq!(outbox.take_due(at(6), |_| true).len(), 1);
    }

    #[test]
    fn retries_back_off() {
        let (mut outbox, key) = one_in_flight();
        let unavailable = refusal(ErrorCode::Unavailable, 0);
        let outcome = outbox.refused(&key, &unavailable, at(10)).unwrap();
        assert_eq!(outcome, Outcome::RetryAt(at(11)));
        let outcome = outbox.unanswered(&key, at(20)).unwrap();
        assert_eq!(outcome, Outcome::RetryAt(at(22)));
        assert_eq!(outbox.store().get(&key).unwrap().attempts, 2);
        assert_eq!(backoff(3), Duration::from_secs(4));
        assert_eq!(backoff(7), Duration::from_secs(60));
        assert_eq!(backoff(50), Duration::from_secs(60));
    }

    #[test]
    fn internal_errors_are_retried_a_few_times() {
        let (mut outbox, key) = one_in_flight();
        let internal = refusal(ErrorCode::Internal, 0);
        // A code from a newer version counts as INTERNAL.
        let unknown = ErrorReply {
            code: 99,
            ..ErrorReply::default()
        };
        assert!(matches!(
            outbox.refused(&key, &internal, at(1)).unwrap(),
            Outcome::RetryAt(_)
        ));
        assert!(matches!(
            outbox.refused(&key, &unknown, at(2)).unwrap(),
            Outcome::RetryAt(_)
        ));
        let outcome = outbox.refused(&key, &internal, at(3)).unwrap();
        assert_eq!(outcome, Outcome::Failed(ErrorCode::Internal));
        assert!(outbox.pending().is_empty());
    }

    #[test]
    fn a_feature_turned_off_holds_the_entry_back() {
        let (mut outbox, key) = one_in_flight();
        let disabled = refusal(ErrorCode::Disabled, 0);
        let outcome = outbox.refused(&key, &disabled, at(1)).unwrap();
        assert_eq!(outcome, Outcome::WaitForReady);
        assert!(outbox.take_due(at(2), |_| true).is_empty());
        outbox.feature_ready(Feature::Clipboard).unwrap();
        assert!(outbox.take_due(at(2), |_| true).is_empty());
        outbox.feature_ready(Feature::Notifications).unwrap();
        assert_eq!(outbox.take_due(at(2), |_| true).len(), 1);
    }

    #[test]
    fn old_entries_expire() {
        let (mut outbox, key) = one_in_flight();
        let other = outbox.push(dismiss("n2"), at(300)).unwrap();
        assert_eq!(outbox.next_wakeup(), Some(at(600)));
        assert!(outbox.expire(at(599)).unwrap().is_empty());
        let expired = outbox.expire(at(600)).unwrap();
        assert_eq!(expired.iter().map(|e| e.key).collect::<Vec<_>>(), [key]);
        // Expired entries are never due, even before expire removes them.
        assert!(outbox.take_due(at(900), |_| true).is_empty());
        assert_eq!(outbox.expire(at(900)).unwrap().len(), 1);
        assert!(outbox.store().get(&other).is_none());
    }
}
