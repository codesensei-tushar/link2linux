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
//! rules on top of a store.

use std::collections::BTreeMap;
use std::collections::hash_map::RandomState;
use std::fmt;
use std::hash::{BuildHasher, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use link2linux_protocol::Payload;

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
    /// The store failed to read or write.
    #[error("outbox storage failed: {0}")]
    Store(String),
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
}

impl<S: OutboxStore> Outbox<S> {
    /// An outbox over `store`, with the entries it already holds.
    pub fn new(store: S) -> Self {
        Self { store }
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
}
