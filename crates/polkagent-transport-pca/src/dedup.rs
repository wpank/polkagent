//! Deduplication store for at-least-once delivery.
//!
//! When messages are delivered at-least-once, the same delivery ID may arrive
//! more than once. A [`DedupStore`] tracks which delivery IDs have already
//! been processed so that duplicate deliveries can be silently dropped.

use std::collections::HashSet;

use parking_lot::Mutex;

use crate::error::PcaError;

/// A store that tracks processed delivery IDs for deduplication.
///
/// Implementations must be safe to call from multiple threads.
pub trait DedupStore: Send + Sync {
    /// Check whether `delivery_id` has already been processed. If not,
    /// atomically mark it as known and return `Ok(true)` (meaning "this
    /// is new, go ahead and process it"). If the ID is already known,
    /// return `Ok(false)` (duplicate).
    fn check_and_mark(&self, delivery_id: &str) -> Result<bool, PcaError>;

    /// Return whether `delivery_id` has been seen before, without marking it.
    fn is_known(&self, delivery_id: &str) -> bool;

    /// Return the number of tracked delivery IDs.
    fn len(&self) -> usize;

    /// Return whether the store is empty.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Return all known delivery IDs (for persistence).
    fn known_ids(&self) -> Vec<String>;

    /// Bulk-load delivery IDs (for restoring from persistent state).
    fn restore(&self, ids: &[String]);
}

/// An in-memory dedup store backed by a `HashSet`.
///
/// Suitable for tests and short-lived processes. For production use with
/// crash safety, pair with [`PersistentState`](super::persistence::PersistentState).
pub struct InMemoryDedupStore {
    seen: Mutex<HashSet<String>>,
}

impl InMemoryDedupStore {
    /// Create a new empty in-memory dedup store.
    pub fn new() -> Self {
        Self {
            seen: Mutex::new(HashSet::new()),
        }
    }
}

impl Default for InMemoryDedupStore {
    fn default() -> Self {
        Self::new()
    }
}

impl DedupStore for InMemoryDedupStore {
    fn check_and_mark(&self, delivery_id: &str) -> Result<bool, PcaError> {
        let mut seen = self.seen.lock();
        Ok(seen.insert(delivery_id.to_owned()))
    }

    fn is_known(&self, delivery_id: &str) -> bool {
        self.seen.lock().contains(delivery_id)
    }

    fn len(&self) -> usize {
        self.seen.lock().len()
    }

    fn known_ids(&self) -> Vec<String> {
        self.seen.lock().iter().cloned().collect()
    }

    fn restore(&self, ids: &[String]) {
        let mut seen = self.seen.lock();
        for id in ids {
            seen.insert(id.clone());
        }
    }
}

/// Accept an inbound message atomically: check dedup, mark the delivery ID,
/// and record the pending turn, all in one logical operation.
///
/// Returns `Ok(true)` if the message is new and was accepted.
/// Returns `Ok(false)` if the message is a duplicate.
pub struct AtomicAccept<'a> {
    dedup: &'a dyn DedupStore,
}

impl<'a> AtomicAccept<'a> {
    /// Create a new atomic accept operation backed by the given dedup store.
    pub fn new(dedup: &'a dyn DedupStore) -> Self {
        Self { dedup }
    }

    /// Accept an inbound delivery. Returns `true` if the delivery is new
    /// (not a duplicate) and was successfully recorded.
    ///
    /// `delivery_id` is the unique ID of the incoming message.
    /// `pending_turn` is an opaque payload representing the turn to be
    /// processed (stored for crash recovery).
    pub fn accept_inbound(
        &self,
        delivery_id: &str,
        pending_turn: &[u8],
        pending_turns: &Mutex<Vec<PendingTurn>>,
    ) -> Result<bool, PcaError> {
        // Step 1: check-and-mark is atomic within the dedup store.
        let is_new = self.dedup.check_and_mark(delivery_id)?;
        if !is_new {
            return Ok(false);
        }

        // Step 2: record the pending turn for crash recovery.
        pending_turns.lock().push(PendingTurn {
            delivery_id: delivery_id.to_owned(),
            payload: pending_turn.to_vec(),
        });

        Ok(true)
    }
}

/// A turn that has been accepted but not yet fully processed.
///
/// Stored for crash recovery so that incomplete processing can be
/// resumed or retried after a restart.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PendingTurn {
    /// The delivery ID that produced this turn.
    pub delivery_id: String,
    /// Opaque serialized turn payload.
    pub payload: Vec<u8>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_delivery_is_accepted() {
        let store = InMemoryDedupStore::new();
        assert!(store.check_and_mark("d-1").expect("check"));
        assert!(store.is_known("d-1"));
    }

    #[test]
    fn duplicate_delivery_is_rejected() {
        let store = InMemoryDedupStore::new();
        assert!(store.check_and_mark("d-1").expect("first"));
        assert!(!store.check_and_mark("d-1").expect("second"));
    }

    #[test]
    fn different_ids_are_independent() {
        let store = InMemoryDedupStore::new();
        assert!(store.check_and_mark("d-1").expect("d-1"));
        assert!(store.check_and_mark("d-2").expect("d-2"));
        assert!(store.is_known("d-1"));
        assert!(store.is_known("d-2"));
        assert!(!store.is_known("d-3"));
    }

    #[test]
    fn len_tracks_entries() {
        let store = InMemoryDedupStore::new();
        assert_eq!(store.len(), 0);
        assert!(store.is_empty());

        store.check_and_mark("d-1").expect("check");
        assert_eq!(store.len(), 1);
        assert!(!store.is_empty());

        store.check_and_mark("d-2").expect("check");
        assert_eq!(store.len(), 2);

        // Duplicate should not increase length.
        store.check_and_mark("d-1").expect("check");
        assert_eq!(store.len(), 2);
    }

    #[test]
    fn known_ids_returns_all() {
        let store = InMemoryDedupStore::new();
        store.check_and_mark("d-1").expect("check");
        store.check_and_mark("d-2").expect("check");
        store.check_and_mark("d-3").expect("check");

        let mut ids = store.known_ids();
        ids.sort();
        assert_eq!(ids, vec!["d-1", "d-2", "d-3"]);
    }

    #[test]
    fn restore_loads_ids() {
        let store = InMemoryDedupStore::new();
        store.restore(&["d-a".into(), "d-b".into()]);

        assert!(store.is_known("d-a"));
        assert!(store.is_known("d-b"));
        assert!(!store.is_known("d-c"));
        assert_eq!(store.len(), 2);

        // After restore, check_and_mark should treat them as duplicates.
        assert!(!store.check_and_mark("d-a").expect("dup"));
    }

    #[test]
    fn restore_is_additive() {
        let store = InMemoryDedupStore::new();
        store.check_and_mark("d-existing").expect("check");
        store.restore(&["d-restored".into()]);

        assert!(store.is_known("d-existing"));
        assert!(store.is_known("d-restored"));
        assert_eq!(store.len(), 2);
    }

    #[test]
    fn atomic_accept_new_message() {
        let store = InMemoryDedupStore::new();
        let accept = AtomicAccept::new(&store);
        let pending = Mutex::new(Vec::new());

        let result = accept
            .accept_inbound("d-1", b"turn-payload", &pending)
            .expect("accept");
        assert!(result);
        assert!(store.is_known("d-1"));
        assert_eq!(pending.lock().len(), 1);
        assert_eq!(pending.lock()[0].delivery_id, "d-1");
        assert_eq!(pending.lock()[0].payload, b"turn-payload");
    }

    #[test]
    fn atomic_accept_rejects_duplicate() {
        let store = InMemoryDedupStore::new();
        let accept = AtomicAccept::new(&store);
        let pending = Mutex::new(Vec::new());

        accept
            .accept_inbound("d-1", b"first", &pending)
            .expect("first");
        let result = accept
            .accept_inbound("d-1", b"duplicate", &pending)
            .expect("second");
        assert!(!result);
        // Only one pending turn should exist.
        assert_eq!(pending.lock().len(), 1);
    }

    #[test]
    fn atomic_accept_multiple_different() {
        let store = InMemoryDedupStore::new();
        let accept = AtomicAccept::new(&store);
        let pending = Mutex::new(Vec::new());

        for i in 0..5 {
            let id = format!("d-{i}");
            let payload = format!("turn-{i}");
            let result = accept
                .accept_inbound(&id, payload.as_bytes(), &pending)
                .expect("accept");
            assert!(result);
        }
        assert_eq!(pending.lock().len(), 5);
        assert_eq!(store.len(), 5);
    }

    #[test]
    fn default_creates_empty_store() {
        let store = InMemoryDedupStore::default();
        assert!(store.is_empty());
    }

    #[test]
    fn pending_turn_serialization() {
        let turn = PendingTurn {
            delivery_id: "d-test".into(),
            payload: b"hello".to_vec(),
        };
        let json = serde_json::to_string(&turn).expect("serialize");
        let back: PendingTurn = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.delivery_id, "d-test");
        assert_eq!(back.payload, b"hello");
    }
}
