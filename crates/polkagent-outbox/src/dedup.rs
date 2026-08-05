//! Consumer-side deduplication for the durable outbox.
//!
//! [`DeduplicationLog`] tracks `(consumer_id, idempotency_key)` pairs that
//! have already been processed. Before a consumer applies a message it calls
//! [`is_duplicate`](DeduplicationLog::is_duplicate); if `true` it can safely
//! skip processing and call [`acknowledge`](crate::outbox::DurableOutbox::acknowledge)
//! without re-executing side effects.
//!
//! Entries are retained for a configurable window (default 24 hours). Entries
//! older than the window are pruned lazily during
//! [`DeduplicationLog::record_processed`] and [`DeduplicationLog::is_duplicate`]
//! calls to avoid unbounded memory growth.

use std::collections::HashMap;

use chrono::{DateTime, Duration, Utc};
use thiserror::Error;
use tracing::debug;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors returned by [`DeduplicationLog`].
#[derive(Debug, Error)]
pub enum DedupError {
    /// The provided consumer identifier was empty.
    #[error("consumer_id must not be empty")]
    EmptyConsumerId,
    /// The provided idempotency key was empty.
    #[error("idempotency_key must not be empty")]
    EmptyIdempotencyKey,
}

// ---------------------------------------------------------------------------
// Entry
// ---------------------------------------------------------------------------

/// Internal record for a single processed idempotency key.
#[derive(Debug, Clone)]
struct Entry {
    processed_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// DeduplicationLog
// ---------------------------------------------------------------------------

/// In-memory deduplication log keyed by `(consumer_id, idempotency_key)`.
///
/// # Thread safety
///
/// This type uses interior mutability only through `&mut self` methods; callers
/// that require shared access across tasks should wrap it in a
/// `tokio::sync::Mutex` or `Arc<Mutex<_>>`.
///
/// # Window semantics
///
/// The dedup window is a sliding window: each call to
/// [`record_processed`](Self::record_processed) records the wall-clock time at
/// which the entry was written. A later call to
/// [`is_duplicate`](Self::is_duplicate) returns `true` only if that recorded
/// time is within `window` of `Utc::now()` at the time of the query.
pub struct DeduplicationLog {
    /// How long a processed entry is remembered.
    window: Duration,
    /// `consumer_id -> idempotency_key -> Entry`
    entries: HashMap<String, HashMap<String, Entry>>,
}

impl DeduplicationLog {
    /// Create a log with a custom retention window.
    #[must_use]
    pub fn new(window: Duration) -> Self {
        Self {
            window,
            entries: HashMap::new(),
        }
    }

    /// Create a log with the default 24-hour retention window.
    #[must_use]
    pub fn with_default_window() -> Self {
        Self::new(Duration::hours(24))
    }

    /// Returns `true` if `(consumer_id, idempotency_key)` was processed within
    /// the current window.
    ///
    /// This call also opportunistically prunes expired entries for the given
    /// `consumer_id`.
    pub fn is_duplicate(&mut self, consumer_id: &str, idempotency_key: &str) -> bool {
        self.prune_consumer(consumer_id);
        self.entries
            .get(consumer_id)
            .and_then(|m| m.get(idempotency_key))
            .is_some_and(|entry| self.is_within_window(entry.processed_at))
    }

    /// Record that `(consumer_id, idempotency_key)` has been successfully
    /// processed.
    ///
    /// Overwrites any pre-existing entry for the same pair (idempotent to call
    /// multiple times).
    ///
    /// # Errors
    ///
    /// Returns [`DedupError::EmptyConsumerId`] or
    /// [`DedupError::EmptyIdempotencyKey`] when the arguments are empty.
    pub fn record_processed(
        &mut self,
        consumer_id: &str,
        idempotency_key: &str,
    ) -> Result<(), DedupError> {
        if consumer_id.is_empty() {
            return Err(DedupError::EmptyConsumerId);
        }
        if idempotency_key.is_empty() {
            return Err(DedupError::EmptyIdempotencyKey);
        }

        self.prune_consumer(consumer_id);

        let consumer_map = self.entries.entry(consumer_id.to_owned()).or_default();
        consumer_map.insert(
            idempotency_key.to_owned(),
            Entry {
                processed_at: Utc::now(),
            },
        );

        debug!(
            consumer_id = consumer_id,
            idempotency_key = idempotency_key,
            "dedup entry recorded"
        );
        Ok(())
    }

    /// Return the number of live (within-window) entries across all consumers.
    ///
    /// Primarily useful for diagnostics and tests.
    pub fn live_entry_count(&self) -> usize {
        let now = Utc::now();
        self.entries
            .values()
            .flat_map(|m| m.values())
            .filter(|e| now.signed_duration_since(e.processed_at) < self.window)
            .count()
    }

    // -----------------------------------------------------------------------
    // Private helpers
    // -----------------------------------------------------------------------

    fn is_within_window(&self, processed_at: DateTime<Utc>) -> bool {
        let age = Utc::now().signed_duration_since(processed_at);
        age < self.window
    }

    /// Remove all expired entries for a single consumer.
    fn prune_consumer(&mut self, consumer_id: &str) {
        let window = self.window;
        let now = Utc::now();
        if let Some(map) = self.entries.get_mut(consumer_id) {
            map.retain(|_, entry| now.signed_duration_since(entry.processed_at) < window);
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn log_1h() -> DeduplicationLog {
        DeduplicationLog::new(Duration::hours(1))
    }

    #[test]
    fn fresh_key_is_not_duplicate() {
        let mut log = log_1h();
        assert!(!log.is_duplicate("consumer-1", "key-abc"));
    }

    #[test]
    fn recorded_key_is_duplicate() {
        let mut log = log_1h();
        log.record_processed("consumer-1", "key-abc")
            .expect("record");
        assert!(log.is_duplicate("consumer-1", "key-abc"));
    }

    #[test]
    fn different_consumer_is_not_duplicate() {
        let mut log = log_1h();
        log.record_processed("consumer-1", "key-abc")
            .expect("record");
        // Same idempotency key but different consumer — not a duplicate.
        assert!(!log.is_duplicate("consumer-2", "key-abc"));
    }

    #[test]
    fn different_key_same_consumer_is_not_duplicate() {
        let mut log = log_1h();
        log.record_processed("consumer-1", "key-abc")
            .expect("record");
        assert!(!log.is_duplicate("consumer-1", "key-xyz"));
    }

    #[test]
    fn expired_entry_is_not_duplicate() {
        // Window of 1 nanosecond — effectively zero.
        let mut log = DeduplicationLog::new(Duration::nanoseconds(1));
        log.record_processed("c", "k").expect("record");
        // Sleep briefly so the entry ages past the window.
        std::thread::sleep(std::time::Duration::from_millis(5));
        assert!(!log.is_duplicate("c", "k"));
    }

    #[test]
    fn empty_consumer_id_returns_error() {
        let mut log = log_1h();
        let err = log.record_processed("", "key").unwrap_err();
        assert!(matches!(err, DedupError::EmptyConsumerId));
    }

    #[test]
    fn empty_idempotency_key_returns_error() {
        let mut log = log_1h();
        let err = log.record_processed("consumer", "").unwrap_err();
        assert!(matches!(err, DedupError::EmptyIdempotencyKey));
    }

    #[test]
    fn live_entry_count_correct() {
        let mut log = log_1h();
        assert_eq!(log.live_entry_count(), 0);
        log.record_processed("c1", "k1").expect("ok");
        log.record_processed("c1", "k2").expect("ok");
        log.record_processed("c2", "k1").expect("ok");
        assert_eq!(log.live_entry_count(), 3);
    }

    #[test]
    fn record_processed_is_idempotent() {
        let mut log = log_1h();
        log.record_processed("c", "k").expect("first");
        log.record_processed("c", "k").expect("second — no error");
        assert_eq!(log.live_entry_count(), 1);
        assert!(log.is_duplicate("c", "k"));
    }
}
