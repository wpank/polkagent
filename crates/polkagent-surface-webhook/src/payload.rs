//! Webhook payload types for different agent events.
//!
//! [`WebhookPayload`] is the top-level envelope that wraps every webhook
//! delivery. The `data` field carries event-specific content as a
//! [`serde_json::Value`] to allow arbitrary event schemas without coupling
//! the webhook layer to every event domain type.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// WebhookPayload
// ---------------------------------------------------------------------------

/// The top-level webhook delivery payload.
///
/// Every webhook POST body is a JSON-serialized `WebhookPayload`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WebhookPayload {
    /// Canonical event type in dot-notation (e.g. `"run.started"`,
    /// `"effect.executed"`, `"approval.requested"`).
    pub event_type: String,

    /// When the event was originally produced.
    pub timestamp: DateTime<Utc>,

    /// The webhook subscription this delivery belongs to.
    pub webhook_id: Uuid,

    /// Unique identifier for this specific delivery attempt.
    pub delivery_id: Uuid,

    /// Event-specific data.
    pub data: Value,
}

impl WebhookPayload {
    /// Create a new webhook payload.
    #[must_use]
    pub fn new(
        event_type: impl Into<String>,
        webhook_id: Uuid,
        delivery_id: Uuid,
        data: Value,
    ) -> Self {
        Self {
            event_type: event_type.into(),
            timestamp: Utc::now(),
            webhook_id,
            delivery_id,
            data,
        }
    }

    /// Create a payload with an explicit timestamp (useful for testing).
    #[must_use]
    pub fn with_timestamp(
        event_type: impl Into<String>,
        webhook_id: Uuid,
        delivery_id: Uuid,
        data: Value,
        timestamp: DateTime<Utc>,
    ) -> Self {
        Self {
            event_type: event_type.into(),
            timestamp,
            webhook_id,
            delivery_id,
            data,
        }
    }

    /// Serialize this payload to a JSON byte vector.
    ///
    /// # Errors
    ///
    /// Returns an error if serialization fails (should not happen for valid
    /// payloads).
    pub fn to_json_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }
}

// ---------------------------------------------------------------------------
// Convenience constructors for common event types
// ---------------------------------------------------------------------------

/// Create a `run.started` payload.
#[must_use]
pub fn run_started(webhook_id: Uuid, delivery_id: Uuid, data: Value) -> WebhookPayload {
    WebhookPayload::new("run.started", webhook_id, delivery_id, data)
}

/// Create a `run.completed` payload.
#[must_use]
pub fn run_completed(webhook_id: Uuid, delivery_id: Uuid, data: Value) -> WebhookPayload {
    WebhookPayload::new("run.completed", webhook_id, delivery_id, data)
}

/// Create an `effect.executed` payload.
#[must_use]
pub fn effect_executed(webhook_id: Uuid, delivery_id: Uuid, data: Value) -> WebhookPayload {
    WebhookPayload::new("effect.executed", webhook_id, delivery_id, data)
}

/// Create an `approval.requested` payload.
#[must_use]
pub fn approval_requested(webhook_id: Uuid, delivery_id: Uuid, data: Value) -> WebhookPayload {
    WebhookPayload::new("approval.requested", webhook_id, delivery_id, data)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn payload_serde_round_trip() {
        let p = WebhookPayload::new(
            "run.started",
            Uuid::nil(),
            Uuid::nil(),
            json!({"run_id": "abc"}),
        );
        let json_bytes = p.to_json_bytes().expect("serialize");
        let back: WebhookPayload = serde_json::from_slice(&json_bytes).expect("deserialize");
        assert_eq!(p.event_type, back.event_type);
        assert_eq!(p.webhook_id, back.webhook_id);
        assert_eq!(p.delivery_id, back.delivery_id);
        assert_eq!(p.data, back.data);
    }

    #[test]
    fn convenience_constructors_set_event_type() {
        let wid = Uuid::nil();
        let did = Uuid::nil();
        let data = json!({});
        assert_eq!(
            run_started(wid, did, data.clone()).event_type,
            "run.started"
        );
        assert_eq!(
            run_completed(wid, did, data.clone()).event_type,
            "run.completed"
        );
        assert_eq!(
            effect_executed(wid, did, data.clone()).event_type,
            "effect.executed"
        );
        assert_eq!(
            approval_requested(wid, did, data).event_type,
            "approval.requested"
        );
    }

    #[test]
    fn payload_with_explicit_timestamp() {
        let ts = DateTime::parse_from_rfc3339("2024-06-01T12:00:00Z")
            .expect("parse")
            .with_timezone(&Utc);
        let p =
            WebhookPayload::with_timestamp("run.started", Uuid::nil(), Uuid::nil(), json!({}), ts);
        assert_eq!(p.timestamp, ts);
    }

    #[test]
    fn payload_json_has_expected_fields() {
        let p = WebhookPayload::new(
            "effect.executed",
            Uuid::nil(),
            Uuid::nil(),
            json!({"effect_id": "xyz"}),
        );
        let v: Value = serde_json::to_value(&p).expect("to_value");
        assert!(v.get("event_type").is_some());
        assert!(v.get("timestamp").is_some());
        assert!(v.get("webhook_id").is_some());
        assert!(v.get("delivery_id").is_some());
        assert!(v.get("data").is_some());
    }
}
