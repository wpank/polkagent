//! Dead Letter Queue (DLQ) for the durable outbox.
//!
//! Messages that fail delivery after exhausting their maximum retry budget are
//! moved to the DLQ for manual inspection and selective replay. This module
//! provides:
//!
//! * [`DeadLetter`] / [`DeliveryError`] — the envelope and error-history types
//!   stored in the DLQ.
//! * [`DeadLetterQueue`] — an async trait abstracting DLQ storage.
//! * [`InMemoryDlq`] — a `Vec`-backed in-memory implementation.
//! * [`DlqPolicy`] — configurable limits (max retries, retention, queue size).
//! * [`DlqMonitor`] — lightweight health monitor that fires when the queue
//!   grows beyond a configurable threshold.
//! * [`DlqStats`] — aggregate statistics over the current DLQ contents.
//!
//! # Storage note
//!
//! The in-memory backend is suitable for tests and single-process deployments.
//! A persistent backend (SQLite / Postgres) can be plugged in by implementing
//! [`DeadLetterQueue`] without changing consumer code.

use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::str::FromStr;
use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::Mutex;
use tracing::{debug, warn};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// DeadLetterId
// ---------------------------------------------------------------------------

/// Opaque, time-ordered identifier for a dead-letter entry.
///
/// Backed by a UUID v7 so that natural sort order matches insertion order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DeadLetterId(Uuid);

impl DeadLetterId {
    /// Generate a new time-ordered (v7) dead-letter identifier.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    /// Wrap an existing [`Uuid`].
    #[must_use]
    pub fn from_uuid(uuid: Uuid) -> Self {
        Self(uuid)
    }

    /// Return the inner [`Uuid`].
    #[must_use]
    pub fn as_uuid(&self) -> Uuid {
        self.0
    }
}

impl Default for DeadLetterId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for DeadLetterId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for DeadLetterId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(s).map(Self)
    }
}

impl From<Uuid> for DeadLetterId {
    fn from(uuid: Uuid) -> Self {
        Self(uuid)
    }
}

impl From<DeadLetterId> for Uuid {
    fn from(id: DeadLetterId) -> Self {
        id.0
    }
}

// ---------------------------------------------------------------------------
// DlqError
// ---------------------------------------------------------------------------

/// Errors returned by [`DeadLetterQueue`] operations.
#[derive(Debug, Error)]
pub enum DlqError {
    /// The DLQ has reached its configured maximum capacity.
    #[error("dead-letter queue is full (limit: {limit})")]
    QueueFull {
        /// The configured maximum queue size.
        limit: usize,
    },

    /// No dead-letter entry with the given [`DeadLetterId`] exists.
    #[error("dead-letter entry not found: {0}")]
    NotFound(DeadLetterId),

    /// A replay attempt failed for the specified entry.
    #[error("replay failed for {id}: {reason}")]
    ReplayFailed {
        /// The dead-letter entry that could not be replayed.
        id: DeadLetterId,
        /// Human-readable description of the failure.
        reason: String,
    },

    /// An opaque storage-layer error.
    #[error("storage error: {0}")]
    StorageError(String),
}

/// Convenience alias for DLQ results.
pub type DlqResult<T> = Result<T, DlqError>;

// ---------------------------------------------------------------------------
// DeliveryError
// ---------------------------------------------------------------------------

/// A single failed delivery attempt recorded in a dead letter's error history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeliveryError {
    /// Wall-clock time at which the delivery attempt failed.
    pub timestamp: DateTime<Utc>,
    /// Machine-readable error code (e.g. `"TIMEOUT"`, `"HTTP_503"`).
    pub error_code: String,
    /// Human-readable error description.
    pub error_message: String,
    /// The destination that was targeted by this delivery attempt.
    pub destination: String,
}

impl DeliveryError {
    /// Create a new delivery error snapshot.
    #[must_use]
    pub fn new(
        error_code: impl Into<String>,
        error_message: impl Into<String>,
        destination: impl Into<String>,
    ) -> Self {
        Self {
            timestamp: Utc::now(),
            error_code: error_code.into(),
            error_message: error_message.into(),
            destination: destination.into(),
        }
    }
}

// ---------------------------------------------------------------------------
// DeadLetter
// ---------------------------------------------------------------------------

/// A message that has been moved to the dead-letter queue after exhausting all
/// delivery retries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeadLetter {
    /// Unique identifier within the DLQ.
    pub id: DeadLetterId,
    /// The identifier of the original outbox message that failed.
    pub original_message_id: String,
    /// The message payload that could not be delivered.
    pub payload: serde_json::Value,
    /// Chronological list of delivery errors across all attempts.
    pub error_history: Vec<DeliveryError>,
    /// Wall-clock time of the very first delivery attempt.
    pub first_attempt_at: DateTime<Utc>,
    /// Wall-clock time of the most recent delivery attempt.
    pub last_attempt_at: DateTime<Utc>,
    /// Total number of delivery attempts made.
    pub attempt_count: u32,
    /// Arbitrary metadata attached to this dead letter (e.g. routing info,
    /// partition key, consumer tags).
    pub metadata: serde_json::Value,
}

impl DeadLetter {
    /// Build a new dead letter with sensible defaults.
    ///
    /// `first_attempt_at` and `last_attempt_at` are both set to [`Utc::now`].
    #[must_use]
    pub fn new(
        original_message_id: impl Into<String>,
        payload: serde_json::Value,
        attempt_count: u32,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: DeadLetterId::new(),
            original_message_id: original_message_id.into(),
            payload,
            error_history: Vec::new(),
            first_attempt_at: now,
            last_attempt_at: now,
            attempt_count,
            metadata: serde_json::Value::Null,
        }
    }

    /// Return the age of this dead letter measured from `first_attempt_at`.
    #[must_use]
    pub fn age(&self) -> Duration {
        Utc::now().signed_duration_since(self.first_attempt_at)
    }
}

// ---------------------------------------------------------------------------
// ReplayResult
// ---------------------------------------------------------------------------

/// Summary of a bulk replay operation.
#[derive(Debug, Clone, Default)]
pub struct ReplayResult {
    /// Total number of dead-letter entries that were attempted.
    pub total: usize,
    /// Number of entries that were successfully replayed.
    pub succeeded: usize,
    /// Number of entries that failed during replay.
    pub failed: usize,
    /// Per-entry errors for the entries that failed.
    pub errors: Vec<(DeadLetterId, String)>,
}

// ---------------------------------------------------------------------------
// DlqStats
// ---------------------------------------------------------------------------

/// Aggregate statistics over the current dead-letter queue contents.
#[derive(Debug, Clone)]
pub struct DlqStats {
    /// Total number of messages currently in the DLQ.
    pub total_messages: usize,
    /// Age of the oldest message (time since `first_attempt_at`), or `None`
    /// if the DLQ is empty.
    pub oldest_message_age: Option<Duration>,
    /// Timestamp of the newest message's `last_attempt_at`, or `None` if
    /// the DLQ is empty.
    pub newest_message_at: Option<DateTime<Utc>>,
    /// Count of dead-letter entries grouped by the most recent error code.
    pub by_error_code: HashMap<String, u64>,
}

// ---------------------------------------------------------------------------
// DlqPolicy
// ---------------------------------------------------------------------------

/// Configuration governing dead-letter queue behaviour.
#[derive(Debug, Clone)]
pub struct DlqPolicy {
    /// Maximum number of delivery retries before a message is dead-lettered.
    pub max_retries: u32,
    /// How long dead-letter entries are retained before they become eligible
    /// for purging.
    pub retention_period: Duration,
    /// Hard upper bound on the number of entries the DLQ may hold.
    pub max_queue_size: usize,
}

impl Default for DlqPolicy {
    fn default() -> Self {
        Self {
            max_retries: 5,
            retention_period: Duration::days(7),
            max_queue_size: 10_000,
        }
    }
}

// ---------------------------------------------------------------------------
// DeadLetterQueue trait
// ---------------------------------------------------------------------------

/// Async trait abstracting dead-letter queue storage.
///
/// Implementations must be `Send + Sync` so that they can be shared across
/// Tokio tasks.
pub trait DeadLetterQueue: Send + Sync {
    /// Insert a dead letter into the queue.
    ///
    /// Returns [`DlqError::QueueFull`] if the queue has reached its configured
    /// maximum capacity.
    fn enqueue(&self, letter: DeadLetter) -> impl Future<Output = DlqResult<()>> + Send;

    /// Return up to `limit` dead-letter entries ordered oldest-first without
    /// removing them.
    fn peek(&self, limit: usize) -> impl Future<Output = DlqResult<Vec<DeadLetter>>> + Send;

    /// Remove and return the dead-letter entry with the given `id`.
    fn dequeue(&self, id: &DeadLetterId) -> impl Future<Output = DlqResult<DeadLetter>> + Send;

    /// Return the number of entries currently in the DLQ.
    fn count(&self) -> impl Future<Output = DlqResult<usize>> + Send;

    /// Purge entries whose `first_attempt_at` is older than `age` and return
    /// the number of entries removed.
    fn purge_older_than(&self, age: Duration) -> impl Future<Output = DlqResult<u64>> + Send;

    /// Replay a single dead-letter entry by `id`.
    ///
    /// The default implementation removes the entry from the DLQ. Concrete
    /// implementations may re-enqueue the original message into the outbox
    /// before removing it.
    fn replay(&self, id: &DeadLetterId) -> impl Future<Output = DlqResult<()>> + Send;

    /// Replay all entries in the DLQ and return a summary of the results.
    fn replay_all(&self) -> impl Future<Output = DlqResult<ReplayResult>> + Send;
}

// ---------------------------------------------------------------------------
// InMemoryDlq
// ---------------------------------------------------------------------------

/// A `Vec`-backed in-memory [`DeadLetterQueue`] implementation.
///
/// Suitable for tests and single-process deployments. Thread-safe via an
/// internal `tokio::sync::Mutex`.
pub struct InMemoryDlq {
    inner: Arc<Mutex<Vec<DeadLetter>>>,
    policy: DlqPolicy,
}

impl InMemoryDlq {
    /// Create a new in-memory DLQ with the given policy.
    #[must_use]
    pub fn new(policy: DlqPolicy) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Vec::new())),
            policy,
        }
    }

    /// Create a new in-memory DLQ with default policy.
    #[must_use]
    pub fn with_defaults() -> Self {
        Self::new(DlqPolicy::default())
    }

    /// Compute aggregate statistics over the current DLQ contents.
    pub async fn stats(&self) -> DlqStats {
        let guard = self.inner.lock().await;
        let now = Utc::now();

        let total_messages = guard.len();
        let oldest_message_age = guard
            .iter()
            .map(|l| now.signed_duration_since(l.first_attempt_at))
            .max();
        let newest_message_at = guard.iter().map(|l| l.last_attempt_at).max();

        let mut by_error_code: HashMap<String, u64> = HashMap::new();
        for letter in guard.iter() {
            if let Some(last_err) = letter.error_history.last() {
                *by_error_code
                    .entry(last_err.error_code.clone())
                    .or_insert(0) += 1;
            }
        }

        DlqStats {
            total_messages,
            oldest_message_age,
            newest_message_at,
            by_error_code,
        }
    }
}

impl Clone for InMemoryDlq {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            policy: self.policy.clone(),
        }
    }
}

impl DeadLetterQueue for InMemoryDlq {
    async fn enqueue(&self, letter: DeadLetter) -> DlqResult<()> {
        let mut guard = self.inner.lock().await;
        if guard.len() >= self.policy.max_queue_size {
            return Err(DlqError::QueueFull {
                limit: self.policy.max_queue_size,
            });
        }
        debug!(dead_letter_id = %letter.id, "dead letter enqueued");
        guard.push(letter);
        Ok(())
    }

    async fn peek(&self, limit: usize) -> DlqResult<Vec<DeadLetter>> {
        let guard = self.inner.lock().await;
        // Sort by first_attempt_at ascending (oldest first).
        let mut sorted: Vec<DeadLetter> = guard.clone();
        sorted.sort_by_key(|l| l.first_attempt_at);
        Ok(sorted.into_iter().take(limit).collect())
    }

    async fn dequeue(&self, id: &DeadLetterId) -> DlqResult<DeadLetter> {
        let mut guard = self.inner.lock().await;
        let pos = guard
            .iter()
            .position(|l| l.id == *id)
            .ok_or(DlqError::NotFound(*id))?;
        Ok(guard.remove(pos))
    }

    async fn count(&self) -> DlqResult<usize> {
        let guard = self.inner.lock().await;
        Ok(guard.len())
    }

    async fn purge_older_than(&self, age: Duration) -> DlqResult<u64> {
        let mut guard = self.inner.lock().await;
        let now = Utc::now();
        let before = guard.len();
        guard.retain(|l| now.signed_duration_since(l.first_attempt_at) < age);
        let removed = (before - guard.len()) as u64;
        if removed > 0 {
            debug!(removed = removed, "purged old dead-letter entries");
        }
        Ok(removed)
    }

    async fn replay(&self, id: &DeadLetterId) -> DlqResult<()> {
        // In the in-memory implementation replay simply removes the entry,
        // simulating a successful re-enqueue into the outbox.
        let mut guard = self.inner.lock().await;
        let pos = guard
            .iter()
            .position(|l| l.id == *id)
            .ok_or(DlqError::NotFound(*id))?;
        let letter = guard.remove(pos);
        debug!(
            dead_letter_id = %letter.id,
            original_message_id = %letter.original_message_id,
            "dead letter replayed"
        );
        Ok(())
    }

    async fn replay_all(&self) -> DlqResult<ReplayResult> {
        let mut guard = self.inner.lock().await;
        let total = guard.len();
        let mut result = ReplayResult {
            total,
            succeeded: total,
            failed: 0,
            errors: Vec::new(),
        };

        // In-memory replay always succeeds — drain all entries.
        guard.clear();

        debug!(
            total = result.total,
            succeeded = result.succeeded,
            "replayed all dead-letter entries"
        );

        // If the queue was empty, still return a valid (zero) result.
        if total == 0 {
            result.succeeded = 0;
        }

        Ok(result)
    }
}

// ---------------------------------------------------------------------------
// DlqMonitor
// ---------------------------------------------------------------------------

/// Lightweight health monitor for a [`DeadLetterQueue`].
///
/// Tracks whether the queue size has grown beyond a configurable threshold and
/// provides an alerting callback hook.
pub struct DlqMonitor<Q: DeadLetterQueue> {
    queue: Q,
    /// When the DLQ size exceeds this value, [`check_health`](Self::check_health)
    /// returns an alert.
    alert_threshold: usize,
}

/// The result of a [`DlqMonitor::check_health`] call.
#[derive(Debug, Clone)]
pub struct HealthStatus {
    /// Current number of entries in the DLQ.
    pub queue_size: usize,
    /// The configured alert threshold.
    pub threshold: usize,
    /// `true` when `queue_size > threshold`.
    pub alert: bool,
}

impl<Q: DeadLetterQueue> DlqMonitor<Q> {
    /// Create a new monitor that alerts when the queue grows beyond
    /// `alert_threshold`.
    pub fn new(queue: Q, alert_threshold: usize) -> Self {
        Self {
            queue,
            alert_threshold,
        }
    }

    /// Check the current health of the DLQ.
    ///
    /// Returns a [`HealthStatus`] indicating whether the queue size exceeds the
    /// alert threshold.
    pub async fn check_health(&self) -> DlqResult<HealthStatus> {
        let size = self.queue.count().await?;
        let alert = size > self.alert_threshold;
        if alert {
            warn!(
                queue_size = size,
                threshold = self.alert_threshold,
                "DLQ size exceeds alert threshold"
            );
        }
        Ok(HealthStatus {
            queue_size: size,
            threshold: self.alert_threshold,
            alert,
        })
    }

    /// Return a reference to the underlying queue.
    pub fn queue(&self) -> &Q {
        &self.queue
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn sample_letter(msg_id: &str) -> DeadLetter {
        DeadLetter::new(msg_id, json!({"key": msg_id}), 3)
    }

    fn sample_letter_with_errors(msg_id: &str, error_codes: &[&str]) -> DeadLetter {
        let mut letter = sample_letter(msg_id);
        for code in error_codes {
            letter.error_history.push(DeliveryError::new(
                *code,
                format!("Error from {code}"),
                "https://example.com/webhook",
            ));
        }
        letter
    }

    fn letter_with_age(msg_id: &str, age: Duration) -> DeadLetter {
        let mut letter = sample_letter(msg_id);
        let past = Utc::now() - age;
        letter.first_attempt_at = past;
        letter.last_attempt_at = past;
        letter
    }

    fn small_policy() -> DlqPolicy {
        DlqPolicy {
            max_retries: 3,
            retention_period: Duration::hours(1),
            max_queue_size: 5,
        }
    }

    // -----------------------------------------------------------------------
    // DeadLetterId
    // -----------------------------------------------------------------------

    #[test]
    fn dead_letter_id_uniqueness() {
        let a = DeadLetterId::new();
        let b = DeadLetterId::new();
        assert_ne!(a, b);
    }

    #[test]
    fn dead_letter_id_round_trip_display_parse() {
        let id = DeadLetterId::new();
        let s = id.to_string();
        let parsed: DeadLetterId = s.parse().expect("valid UUID string");
        assert_eq!(id, parsed);
    }

    #[test]
    fn dead_letter_id_serde_transparent() {
        let id = DeadLetterId::new();
        let json_str = serde_json::to_string(&id).expect("serialize");
        assert!(json_str.starts_with('"') && json_str.ends_with('"'));
        let back: DeadLetterId = serde_json::from_str(&json_str).expect("deserialize");
        assert_eq!(id, back);
    }

    // -----------------------------------------------------------------------
    // Enqueue / Dequeue basics
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn enqueue_and_dequeue_single() {
        let dlq = InMemoryDlq::with_defaults();
        let letter = sample_letter("msg-1");
        let id = letter.id;

        dlq.enqueue(letter).await.expect("enqueue");
        assert_eq!(dlq.count().await.expect("count"), 1);

        let dequeued = dlq.dequeue(&id).await.expect("dequeue");
        assert_eq!(dequeued.id, id);
        assert_eq!(dequeued.original_message_id, "msg-1");
        assert_eq!(dlq.count().await.expect("count"), 0);
    }

    #[tokio::test]
    async fn enqueue_multiple_and_dequeue() {
        let dlq = InMemoryDlq::with_defaults();
        let l1 = sample_letter("msg-1");
        let l2 = sample_letter("msg-2");
        let l3 = sample_letter("msg-3");
        let id2 = l2.id;

        dlq.enqueue(l1).await.expect("e1");
        dlq.enqueue(l2).await.expect("e2");
        dlq.enqueue(l3).await.expect("e3");

        assert_eq!(dlq.count().await.expect("count"), 3);

        // Dequeue the middle one.
        let dequeued = dlq.dequeue(&id2).await.expect("dequeue");
        assert_eq!(dequeued.original_message_id, "msg-2");
        assert_eq!(dlq.count().await.expect("count"), 2);
    }

    #[tokio::test]
    async fn dequeue_not_found() {
        let dlq = InMemoryDlq::with_defaults();
        let phantom = DeadLetterId::new();
        let err = dlq.dequeue(&phantom).await.unwrap_err();
        assert!(
            matches!(err, DlqError::NotFound(_)),
            "expected NotFound, got {err:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Peek ordering
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn peek_returns_oldest_first() {
        let dlq = InMemoryDlq::with_defaults();

        // Insert letters with descending ages so insertion order != age order.
        let old = letter_with_age("old", Duration::hours(3));
        let mid = letter_with_age("mid", Duration::hours(1));
        let fresh = sample_letter("fresh");

        // Insert out of age order.
        dlq.enqueue(mid.clone()).await.expect("e1");
        dlq.enqueue(fresh.clone()).await.expect("e2");
        dlq.enqueue(old.clone()).await.expect("e3");

        let peeked = dlq.peek(10).await.expect("peek");
        assert_eq!(peeked.len(), 3);
        assert_eq!(peeked[0].original_message_id, "old");
        assert_eq!(peeked[1].original_message_id, "mid");
        assert_eq!(peeked[2].original_message_id, "fresh");
    }

    #[tokio::test]
    async fn peek_with_limit() {
        let dlq = InMemoryDlq::with_defaults();
        for i in 0..5 {
            dlq.enqueue(sample_letter(&format!("msg-{i}")))
                .await
                .expect("enqueue");
        }

        let peeked = dlq.peek(2).await.expect("peek");
        assert_eq!(peeked.len(), 2);
    }

    #[tokio::test]
    async fn peek_does_not_remove_entries() {
        let dlq = InMemoryDlq::with_defaults();
        dlq.enqueue(sample_letter("msg-1")).await.expect("enqueue");

        let _ = dlq.peek(10).await.expect("peek");
        assert_eq!(dlq.count().await.expect("count"), 1);
    }

    // -----------------------------------------------------------------------
    // Count
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn count_empty() {
        let dlq = InMemoryDlq::with_defaults();
        assert_eq!(dlq.count().await.expect("count"), 0);
    }

    #[tokio::test]
    async fn count_after_enqueue_dequeue() {
        let dlq = InMemoryDlq::with_defaults();
        let letter = sample_letter("msg-1");
        let id = letter.id;
        dlq.enqueue(letter).await.expect("enqueue");
        assert_eq!(dlq.count().await.expect("count"), 1);

        dlq.dequeue(&id).await.expect("dequeue");
        assert_eq!(dlq.count().await.expect("count"), 0);
    }

    // -----------------------------------------------------------------------
    // Queue size limit
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn enqueue_rejects_when_full() {
        let dlq = InMemoryDlq::new(small_policy()); // max_queue_size = 5

        for i in 0..5 {
            dlq.enqueue(sample_letter(&format!("msg-{i}")))
                .await
                .expect("enqueue");
        }

        let err = dlq
            .enqueue(sample_letter("msg-overflow"))
            .await
            .unwrap_err();
        assert!(
            matches!(err, DlqError::QueueFull { limit: 5 }),
            "expected QueueFull, got {err:?}"
        );
    }

    #[tokio::test]
    async fn enqueue_succeeds_after_dequeue_frees_space() {
        let dlq = InMemoryDlq::new(small_policy());
        let mut ids = Vec::new();

        for i in 0..5 {
            let letter = sample_letter(&format!("msg-{i}"));
            ids.push(letter.id);
            dlq.enqueue(letter).await.expect("enqueue");
        }

        // Queue is full. Dequeue one, then enqueue should succeed.
        dlq.dequeue(&ids[0]).await.expect("dequeue");
        dlq.enqueue(sample_letter("msg-new"))
            .await
            .expect("enqueue after space freed");
        assert_eq!(dlq.count().await.expect("count"), 5);
    }

    // -----------------------------------------------------------------------
    // Purge by age
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn purge_removes_old_entries() {
        let dlq = InMemoryDlq::with_defaults();

        dlq.enqueue(letter_with_age("old-1", Duration::hours(5)))
            .await
            .expect("e1");
        dlq.enqueue(letter_with_age("old-2", Duration::hours(3)))
            .await
            .expect("e2");
        dlq.enqueue(sample_letter("fresh"))
            .await
            .expect("e3");

        // Purge entries older than 2 hours.
        let removed = dlq.purge_older_than(Duration::hours(2)).await.expect("purge");
        assert_eq!(removed, 2);
        assert_eq!(dlq.count().await.expect("count"), 1);
    }

    #[tokio::test]
    async fn purge_with_nothing_old_removes_none() {
        let dlq = InMemoryDlq::with_defaults();
        dlq.enqueue(sample_letter("fresh")).await.expect("enqueue");

        let removed = dlq
            .purge_older_than(Duration::hours(1))
            .await
            .expect("purge");
        assert_eq!(removed, 0);
        assert_eq!(dlq.count().await.expect("count"), 1);
    }

    #[tokio::test]
    async fn purge_empty_queue() {
        let dlq = InMemoryDlq::with_defaults();
        let removed = dlq
            .purge_older_than(Duration::hours(1))
            .await
            .expect("purge");
        assert_eq!(removed, 0);
    }

    // -----------------------------------------------------------------------
    // Replay individual
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn replay_removes_entry() {
        let dlq = InMemoryDlq::with_defaults();
        let letter = sample_letter("msg-1");
        let id = letter.id;
        dlq.enqueue(letter).await.expect("enqueue");

        dlq.replay(&id).await.expect("replay");
        assert_eq!(dlq.count().await.expect("count"), 0);
    }

    #[tokio::test]
    async fn replay_not_found() {
        let dlq = InMemoryDlq::with_defaults();
        let phantom = DeadLetterId::new();
        let err = dlq.replay(&phantom).await.unwrap_err();
        assert!(
            matches!(err, DlqError::NotFound(_)),
            "expected NotFound, got {err:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Replay all
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn replay_all_clears_queue() {
        let dlq = InMemoryDlq::with_defaults();
        for i in 0..4 {
            dlq.enqueue(sample_letter(&format!("msg-{i}")))
                .await
                .expect("enqueue");
        }

        let result = dlq.replay_all().await.expect("replay_all");
        assert_eq!(result.total, 4);
        assert_eq!(result.succeeded, 4);
        assert_eq!(result.failed, 0);
        assert!(result.errors.is_empty());
        assert_eq!(dlq.count().await.expect("count"), 0);
    }

    #[tokio::test]
    async fn replay_all_empty_queue() {
        let dlq = InMemoryDlq::with_defaults();
        let result = dlq.replay_all().await.expect("replay_all");
        assert_eq!(result.total, 0);
        assert_eq!(result.succeeded, 0);
        assert_eq!(result.failed, 0);
    }

    // -----------------------------------------------------------------------
    // Error history tracking
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn error_history_preserved_through_enqueue_dequeue() {
        let dlq = InMemoryDlq::with_defaults();
        let letter = sample_letter_with_errors("msg-1", &["TIMEOUT", "HTTP_503", "TIMEOUT"]);
        let id = letter.id;
        assert_eq!(letter.error_history.len(), 3);

        dlq.enqueue(letter).await.expect("enqueue");
        let dequeued = dlq.dequeue(&id).await.expect("dequeue");

        assert_eq!(dequeued.error_history.len(), 3);
        assert_eq!(dequeued.error_history[0].error_code, "TIMEOUT");
        assert_eq!(dequeued.error_history[1].error_code, "HTTP_503");
        assert_eq!(dequeued.error_history[2].error_code, "TIMEOUT");
    }

    #[tokio::test]
    async fn error_history_destinations() {
        let dlq = InMemoryDlq::with_defaults();
        let mut letter = sample_letter("msg-1");
        letter.error_history.push(DeliveryError::new(
            "CONN_REFUSED",
            "connection refused",
            "https://api.example.com/v1/events",
        ));

        let id = letter.id;
        dlq.enqueue(letter).await.expect("enqueue");

        let dequeued = dlq.dequeue(&id).await.expect("dequeue");
        assert_eq!(
            dequeued.error_history[0].destination,
            "https://api.example.com/v1/events"
        );
    }

    // -----------------------------------------------------------------------
    // Stats computation
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn stats_empty_queue() {
        let dlq = InMemoryDlq::with_defaults();
        let stats = dlq.stats().await;

        assert_eq!(stats.total_messages, 0);
        assert!(stats.oldest_message_age.is_none());
        assert!(stats.newest_message_at.is_none());
        assert!(stats.by_error_code.is_empty());
    }

    #[tokio::test]
    async fn stats_single_entry() {
        let dlq = InMemoryDlq::with_defaults();
        let letter = sample_letter_with_errors("msg-1", &["HTTP_503"]);
        dlq.enqueue(letter).await.expect("enqueue");

        let stats = dlq.stats().await;
        assert_eq!(stats.total_messages, 1);
        assert!(stats.oldest_message_age.is_some());
        assert!(stats.newest_message_at.is_some());
        assert_eq!(stats.by_error_code.get("HTTP_503"), Some(&1));
    }

    #[tokio::test]
    async fn stats_multiple_entries_grouped_by_error_code() {
        let dlq = InMemoryDlq::with_defaults();
        dlq.enqueue(sample_letter_with_errors("msg-1", &["TIMEOUT"]))
            .await
            .expect("e1");
        dlq.enqueue(sample_letter_with_errors("msg-2", &["HTTP_503"]))
            .await
            .expect("e2");
        dlq.enqueue(sample_letter_with_errors("msg-3", &["TIMEOUT"]))
            .await
            .expect("e3");
        dlq.enqueue(sample_letter_with_errors("msg-4", &["HTTP_503", "TIMEOUT"]))
            .await
            .expect("e4");

        let stats = dlq.stats().await;
        assert_eq!(stats.total_messages, 4);
        // msg-4 last error is TIMEOUT, so: TIMEOUT=3, HTTP_503=1
        assert_eq!(stats.by_error_code.get("TIMEOUT"), Some(&3));
        assert_eq!(stats.by_error_code.get("HTTP_503"), Some(&1));
    }

    #[tokio::test]
    async fn stats_oldest_reflects_age() {
        let dlq = InMemoryDlq::with_defaults();
        dlq.enqueue(letter_with_age("old", Duration::hours(5)))
            .await
            .expect("e1");
        dlq.enqueue(sample_letter("fresh")).await.expect("e2");

        let stats = dlq.stats().await;
        let oldest = stats.oldest_message_age.expect("should have oldest");
        // The oldest entry is ~5 hours old; allow some slack.
        assert!(oldest >= Duration::hours(4));
    }

    // -----------------------------------------------------------------------
    // DlqMonitor
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn monitor_no_alert_below_threshold() {
        let dlq = InMemoryDlq::with_defaults();
        dlq.enqueue(sample_letter("msg-1")).await.expect("enqueue");

        let monitor = DlqMonitor::new(dlq, 10);
        let status = monitor.check_health().await.expect("health");

        assert_eq!(status.queue_size, 1);
        assert_eq!(status.threshold, 10);
        assert!(!status.alert);
    }

    #[tokio::test]
    async fn monitor_alert_above_threshold() {
        let dlq = InMemoryDlq::with_defaults();
        for i in 0..6 {
            dlq.enqueue(sample_letter(&format!("msg-{i}")))
                .await
                .expect("enqueue");
        }

        let monitor = DlqMonitor::new(dlq, 3);
        let status = monitor.check_health().await.expect("health");

        assert_eq!(status.queue_size, 6);
        assert_eq!(status.threshold, 3);
        assert!(status.alert);
    }

    #[tokio::test]
    async fn monitor_alert_at_threshold_is_not_triggered() {
        let dlq = InMemoryDlq::with_defaults();
        for i in 0..3 {
            dlq.enqueue(sample_letter(&format!("msg-{i}")))
                .await
                .expect("enqueue");
        }

        let monitor = DlqMonitor::new(dlq, 3);
        let status = monitor.check_health().await.expect("health");

        // Exactly at threshold (not above) — no alert.
        assert_eq!(status.queue_size, 3);
        assert!(!status.alert);
    }

    #[tokio::test]
    async fn monitor_queue_ref() {
        let dlq = InMemoryDlq::with_defaults();
        dlq.enqueue(sample_letter("msg-1")).await.expect("enqueue");

        let monitor = DlqMonitor::new(dlq, 10);
        let count = monitor.queue().count().await.expect("count");
        assert_eq!(count, 1);
    }

    // -----------------------------------------------------------------------
    // Concurrent access
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn concurrent_enqueue_dequeue() {
        let dlq = InMemoryDlq::with_defaults();

        // Spawn multiple enqueue tasks concurrently.
        let mut handles = Vec::new();
        for i in 0..10 {
            let dlq_clone = dlq.clone();
            handles.push(tokio::spawn(async move {
                let letter = sample_letter(&format!("msg-{i}"));
                let id = letter.id;
                dlq_clone.enqueue(letter).await.expect("enqueue");
                id
            }));
        }

        let mut ids = Vec::new();
        for handle in handles {
            ids.push(handle.await.expect("join"));
        }

        assert_eq!(dlq.count().await.expect("count"), 10);

        // Dequeue all concurrently.
        let mut dequeue_handles = Vec::new();
        for id in ids {
            let dlq_clone = dlq.clone();
            dequeue_handles.push(tokio::spawn(async move {
                dlq_clone.dequeue(&id).await.expect("dequeue");
            }));
        }

        for handle in dequeue_handles {
            handle.await.expect("join");
        }

        assert_eq!(dlq.count().await.expect("count"), 0);
    }

    #[tokio::test]
    async fn concurrent_enqueue_respects_size_limit() {
        let policy = DlqPolicy {
            max_retries: 3,
            retention_period: Duration::hours(1),
            max_queue_size: 5,
        };
        let dlq = InMemoryDlq::new(policy);

        // Attempt to enqueue 10 items concurrently with a limit of 5.
        let mut handles = Vec::new();
        for i in 0..10 {
            let dlq_clone = dlq.clone();
            handles.push(tokio::spawn(async move {
                let letter = sample_letter(&format!("msg-{i}"));
                dlq_clone.enqueue(letter).await
            }));
        }

        let mut success_count = 0usize;
        let mut full_count = 0usize;
        for handle in handles {
            match handle.await.expect("join") {
                Ok(()) => success_count += 1,
                Err(DlqError::QueueFull { .. }) => full_count += 1,
                Err(e) => panic!("unexpected error: {e:?}"),
            }
        }

        assert_eq!(success_count, 5);
        assert_eq!(full_count, 5);
        assert_eq!(dlq.count().await.expect("count"), 5);
    }

    // -----------------------------------------------------------------------
    // DlqPolicy defaults
    // -----------------------------------------------------------------------

    #[test]
    fn default_policy_values() {
        let policy = DlqPolicy::default();
        assert_eq!(policy.max_retries, 5);
        assert_eq!(policy.retention_period, Duration::days(7));
        assert_eq!(policy.max_queue_size, 10_000);
    }

    // -----------------------------------------------------------------------
    // DeadLetter construction and metadata
    // -----------------------------------------------------------------------

    #[test]
    fn dead_letter_new_defaults() {
        let letter = DeadLetter::new("msg-42", json!({"amount": 100}), 3);
        assert_eq!(letter.original_message_id, "msg-42");
        assert_eq!(letter.attempt_count, 3);
        assert!(letter.error_history.is_empty());
        assert_eq!(letter.metadata, serde_json::Value::Null);
    }

    #[test]
    fn dead_letter_with_metadata() {
        let mut letter = sample_letter("msg-1");
        letter.metadata = json!({"partition_key": "user:42", "consumer": "worker-1"});

        assert_eq!(letter.metadata["partition_key"], "user:42");
        assert_eq!(letter.metadata["consumer"], "worker-1");
    }

    // -----------------------------------------------------------------------
    // DlqError Display
    // -----------------------------------------------------------------------

    #[test]
    fn error_display_messages() {
        let err = DlqError::QueueFull { limit: 100 };
        assert!(err.to_string().contains("100"));

        let id = DeadLetterId::new();
        let err = DlqError::NotFound(id);
        assert!(err.to_string().contains(&id.to_string()));

        let err = DlqError::ReplayFailed {
            id,
            reason: "connection refused".into(),
        };
        assert!(err.to_string().contains("connection refused"));

        let err = DlqError::StorageError("disk full".into());
        assert!(err.to_string().contains("disk full"));
    }
}
