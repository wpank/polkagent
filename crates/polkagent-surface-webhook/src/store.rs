//! Delivery store trait for tracking webhook delivery attempts and status.
//!
//! [`DeliveryStore`] is the persistence abstraction for delivery records. All
//! methods are async to support both in-memory and durable backends.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::Result;

// ---------------------------------------------------------------------------
// DeliveryStatus
// ---------------------------------------------------------------------------

/// Status of a webhook delivery attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryStatus {
    /// Delivery is queued and waiting to be sent.
    Pending,
    /// Delivery is currently being attempted.
    InFlight,
    /// Delivery was successfully acknowledged (2xx response).
    Delivered,
    /// Delivery failed but may be retried.
    Failed,
    /// All retry attempts have been exhausted.
    Exhausted,
}

// ---------------------------------------------------------------------------
// DeliveryRecord
// ---------------------------------------------------------------------------

/// A record tracking the state of a single webhook delivery.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeliveryRecord {
    /// Unique delivery identifier.
    pub id: Uuid,

    /// The webhook subscription this delivery belongs to.
    pub webhook_id: Uuid,

    /// The event type being delivered (e.g. `"run.started"`).
    pub event_type: String,

    /// Current delivery status.
    pub status: DeliveryStatus,

    /// Number of delivery attempts made so far.
    pub attempt_count: u32,

    /// HTTP status code from the most recent attempt, if any.
    pub last_status_code: Option<u16>,

    /// Error message from the most recent failed attempt, if any.
    pub last_error: Option<String>,

    /// When this delivery was first created.
    pub created_at: DateTime<Utc>,

    /// When the most recent attempt was made.
    pub updated_at: DateTime<Utc>,

    /// When the next retry should be attempted, if status is `Failed`.
    pub next_retry_at: Option<DateTime<Utc>>,
}

impl DeliveryRecord {
    /// Create a new pending delivery record.
    #[must_use]
    pub fn new(webhook_id: Uuid, event_type: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::now_v7(),
            webhook_id,
            event_type: event_type.into(),
            status: DeliveryStatus::Pending,
            attempt_count: 0,
            last_status_code: None,
            last_error: None,
            created_at: now,
            updated_at: now,
            next_retry_at: None,
        }
    }

    /// Mark this delivery as successfully delivered.
    pub fn mark_delivered(&mut self, status_code: u16) {
        self.status = DeliveryStatus::Delivered;
        self.last_status_code = Some(status_code);
        self.last_error = None;
        self.updated_at = Utc::now();
        self.next_retry_at = None;
    }

    /// Mark this delivery as failed with an optional next retry time.
    pub fn mark_failed(
        &mut self,
        status_code: Option<u16>,
        error: impl Into<String>,
        next_retry_at: Option<DateTime<Utc>>,
    ) {
        self.status = DeliveryStatus::Failed;
        self.last_status_code = status_code;
        self.last_error = Some(error.into());
        self.attempt_count += 1;
        self.updated_at = Utc::now();
        self.next_retry_at = next_retry_at;
    }

    /// Mark this delivery as exhausted (no more retries).
    pub fn mark_exhausted(&mut self) {
        self.status = DeliveryStatus::Exhausted;
        self.updated_at = Utc::now();
        self.next_retry_at = None;
    }
}

// ---------------------------------------------------------------------------
// DeliveryStore trait
// ---------------------------------------------------------------------------

/// Persistence abstraction for webhook delivery records.
///
/// Implementations must be `Send + Sync` so they can be shared across async
/// tasks.
#[async_trait]
pub trait DeliveryStore: Send + Sync {
    /// Persist a new delivery record.
    async fn create(&self, record: DeliveryRecord) -> Result<DeliveryRecord>;

    /// Retrieve a delivery record by its ID.
    async fn get(&self, id: Uuid) -> Result<DeliveryRecord>;

    /// Update an existing delivery record.
    async fn update(&self, record: DeliveryRecord) -> Result<DeliveryRecord>;

    /// List deliveries for a given webhook, most recent first.
    async fn list_by_webhook(&self, webhook_id: Uuid, limit: usize) -> Result<Vec<DeliveryRecord>>;

    /// List deliveries that are pending retry (status = Failed, `next_retry_at` <= now).
    async fn list_pending_retries(&self, limit: usize) -> Result<Vec<DeliveryRecord>>;

    /// Count deliveries by status for a given webhook.
    async fn count_by_status(&self, webhook_id: Uuid, status: DeliveryStatus) -> Result<usize>;
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "store tests intentionally panic when static delivery-status fixtures fail to serialize"
)]
mod tests {
    use super::*;

    #[test]
    fn new_delivery_record_is_pending() {
        let r = DeliveryRecord::new(Uuid::nil(), "run.started");
        assert_eq!(r.status, DeliveryStatus::Pending);
        assert_eq!(r.attempt_count, 0);
        assert!(r.last_status_code.is_none());
        assert!(r.last_error.is_none());
        assert!(r.next_retry_at.is_none());
    }

    #[test]
    fn mark_delivered_sets_status() {
        let mut r = DeliveryRecord::new(Uuid::nil(), "run.started");
        r.mark_delivered(200);
        assert_eq!(r.status, DeliveryStatus::Delivered);
        assert_eq!(r.last_status_code, Some(200));
        assert!(r.last_error.is_none());
    }

    #[test]
    fn mark_failed_increments_attempt_count() {
        let mut r = DeliveryRecord::new(Uuid::nil(), "run.started");
        r.mark_failed(Some(500), "server error", None);
        assert_eq!(r.status, DeliveryStatus::Failed);
        assert_eq!(r.attempt_count, 1);
        assert_eq!(r.last_status_code, Some(500));
        assert_eq!(r.last_error.as_deref(), Some("server error"));
    }

    #[test]
    fn mark_exhausted_clears_retry() {
        let mut r = DeliveryRecord::new(Uuid::nil(), "run.started");
        r.mark_failed(Some(500), "err", Some(Utc::now()));
        r.mark_exhausted();
        assert_eq!(r.status, DeliveryStatus::Exhausted);
        assert!(r.next_retry_at.is_none());
    }

    #[test]
    fn delivery_status_serde_round_trip() {
        let statuses = [
            DeliveryStatus::Pending,
            DeliveryStatus::InFlight,
            DeliveryStatus::Delivered,
            DeliveryStatus::Failed,
            DeliveryStatus::Exhausted,
        ];
        for s in statuses {
            let json = serde_json::to_string(&s).expect("serialize");
            let back: DeliveryStatus = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(s, back, "round-trip failed for {s:?}");
        }
    }
}
