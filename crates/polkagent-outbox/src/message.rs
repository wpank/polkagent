//! Message types for the durable outbox.
//!
//! [`OutboxMessage`] is the envelope a producer submits. [`OutboxItem`] is the
//! envelope a consumer receives after the outbox has assigned an ID and
//! recorded metadata. [`OutboxId`] is a typed newtype around [`uuid::Uuid`]
//! (UUID v7) giving each outbox entry a time-ordered, globally unique
//! identity.

use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// OutboxId
// ---------------------------------------------------------------------------

/// Opaque, time-ordered identifier for a single entry in the outbox.
///
/// Backed by a UUID v7 so that natural sort order matches insertion order
/// within a process clock epoch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct OutboxId(Uuid);

impl OutboxId {
    /// Generate a new time-ordered (v7) outbox identifier.
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

impl Default for OutboxId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for OutboxId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for OutboxId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(s).map(Self)
    }
}

impl From<Uuid> for OutboxId {
    fn from(uuid: Uuid) -> Self {
        Self(uuid)
    }
}

impl From<OutboxId> for Uuid {
    fn from(id: OutboxId) -> Self {
        id.0
    }
}

// ---------------------------------------------------------------------------
// OutboxMessage
// ---------------------------------------------------------------------------

/// The envelope a producer submits to the outbox.
///
/// * `partition_key` — messages with the same key are delivered in FIFO order
///   relative to one another; messages across different keys may be delivered
///   in any order.
/// * `idempotency_key` — an opaque string that consumers (and the
///   [`DeduplicationLog`](crate::dedup::DeduplicationLog)) use to detect
///   duplicate deliveries.
/// * `payload` — arbitrary JSON value; interpretation is left to consumers.
/// * `max_retries` — how many total delivery attempts are allowed before the
///   message is moved to the dead-letter set (0 = try once, no retries).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutboxMessage {
    /// Key used to group messages into ordered partitions.
    pub partition_key: String,
    /// Caller-supplied idempotency token.
    pub idempotency_key: String,
    /// Arbitrary structured payload.
    pub payload: serde_json::Value,
    /// Wall-clock time at which the message was originally created.
    pub created_at: DateTime<Utc>,
    /// Maximum number of delivery attempts before dead-lettering.
    ///
    /// A value of `0` means the message is attempted once; any failure
    /// dead-letters it immediately.
    pub max_retries: u32,
    /// Tracks how many times delivery has been attempted so far. Callers
    /// should leave this at `0` when constructing a new message; the outbox
    /// increments it internally.
    pub retry_count: u32,
}

impl OutboxMessage {
    /// Build a new message with sensible defaults.
    ///
    /// `created_at` is set to [`Utc::now`] and `retry_count` is zeroed.
    #[must_use]
    pub fn new(
        partition_key: impl Into<String>,
        idempotency_key: impl Into<String>,
        payload: serde_json::Value,
        max_retries: u32,
    ) -> Self {
        Self {
            partition_key: partition_key.into(),
            idempotency_key: idempotency_key.into(),
            payload,
            created_at: Utc::now(),
            max_retries,
            retry_count: 0,
        }
    }

    /// Returns `true` when no further retry attempts are allowed.
    #[must_use]
    pub fn is_exhausted(&self) -> bool {
        self.retry_count > self.max_retries
    }
}

// ---------------------------------------------------------------------------
// OutboxItem
// ---------------------------------------------------------------------------

/// An outbox entry that has been persisted and is visible to consumers.
///
/// Fields prefixed with `claimed_` are `None` while the item is unclaimed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutboxItem {
    /// Stable identifier assigned by the outbox at enqueue time.
    pub id: OutboxId,
    /// The original message envelope.
    pub message: OutboxMessage,
    /// The consumer that currently holds a lease on this item, if any.
    pub claimed_by: Option<String>,
    /// The wall-clock instant at which the current lease expires.
    ///
    /// If `None`, the item is either unclaimed or the lease has been released.
    pub claimed_until: Option<DateTime<Utc>>,
    /// Total number of delivery attempts (claimed → released/nacked cycles)
    /// recorded for this item.
    pub attempt_count: u32,
    /// If `true` the item has exceeded `max_retries` and has been moved to the
    /// dead-letter set.
    pub dead_lettered: bool,
    /// Monotonically increasing sequence number within its partition, used to
    /// enforce FIFO ordering during claim.
    pub(crate) sequence: u64,
}

impl OutboxItem {
    /// Returns `true` when the item is currently leased and the lease has not
    /// yet expired.
    #[must_use]
    pub fn is_claimed(&self) -> bool {
        match self.claimed_until {
            Some(until) => until > Utc::now(),
            None => false,
        }
    }

    /// Returns `true` when the item has a lease but it has already expired.
    #[must_use]
    pub fn is_lease_expired(&self) -> bool {
        match self.claimed_until {
            Some(until) => until <= Utc::now(),
            None => false,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outbox_id_new_is_unique() {
        let a = OutboxId::new();
        let b = OutboxId::new();
        assert_ne!(a, b);
    }

    #[test]
    fn outbox_id_round_trip_display_parse() {
        let id = OutboxId::new();
        let s = id.to_string();
        let parsed: OutboxId = s.parse().expect("valid UUID string");
        assert_eq!(id, parsed);
    }

    #[test]
    fn outbox_id_serde_transparent() {
        let id = OutboxId::new();
        let json = serde_json::to_string(&id).expect("serialize");
        assert!(json.starts_with('"') && json.ends_with('"'));
        let back: OutboxId = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(id, back);
    }

    #[test]
    fn outbox_message_new_zeroes_retry_count() {
        let msg = OutboxMessage::new("part", "idem", serde_json::json!({}), 3);
        assert_eq!(msg.retry_count, 0);
        assert_eq!(msg.max_retries, 3);
    }

    #[test]
    fn outbox_message_is_exhausted() {
        let mut msg = OutboxMessage::new("p", "k", serde_json::json!(null), 2);
        assert!(!msg.is_exhausted());
        msg.retry_count = 2;
        assert!(!msg.is_exhausted()); // exactly at max — still allowed
        msg.retry_count = 3;
        assert!(msg.is_exhausted());
    }
}
