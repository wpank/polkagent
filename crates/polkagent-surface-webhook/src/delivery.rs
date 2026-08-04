//! Webhook delivery engine.
//!
//! [`WebhookDelivery`] handles the actual HTTP POST of JSON payloads to webhook
//! endpoints. It signs every request with HMAC (via [`crate::signature`]),
//! tracks delivery attempts in a [`DeliveryStore`], and retries failed
//! deliveries according to the configured [`RetryPolicy`].

use std::sync::Arc;

use chrono::Utc;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::config::WebhookConfig;
use crate::error::{Result, WebhookError};
use crate::payload::WebhookPayload;
use crate::signature;
use crate::store::{DeliveryRecord, DeliveryStatus, DeliveryStore};

// ---------------------------------------------------------------------------
// WebhookDelivery
// ---------------------------------------------------------------------------

/// Sends webhook payloads over HTTP with signing, retries, and delivery
/// tracking.
pub struct WebhookDelivery<S> {
    client: reqwest::Client,
    store: Arc<S>,
}

impl<S: DeliveryStore> WebhookDelivery<S> {
    /// Create a new delivery engine backed by the given store.
    pub fn new(store: Arc<S>) -> Self {
        let client = reqwest::Client::builder()
            .user_agent("polkagent-webhook/0.1")
            .build()
            .unwrap_or_default();
        Self { client, store }
    }

    /// Create a delivery engine with a custom `reqwest::Client`.
    pub fn with_client(client: reqwest::Client, store: Arc<S>) -> Self {
        Self { client, store }
    }

    /// Deliver a webhook payload, creating a delivery record and performing
    /// the HTTP POST.
    ///
    /// Returns the delivery record (updated with the result of the attempt).
    ///
    /// # Errors
    ///
    /// Returns an error if the delivery record cannot be created or updated.
    /// HTTP failures are captured in the delivery record rather than returned
    /// as errors.
    pub async fn deliver(
        &self,
        config: &WebhookConfig,
        payload: &WebhookPayload,
    ) -> Result<DeliveryRecord> {
        let mut record = DeliveryRecord::new(payload.webhook_id, &payload.event_type);
        record = self.store.create(record).await?;

        self.attempt_delivery(config, payload, &mut record).await?;
        Ok(record)
    }

    /// Deliver a webhook payload using an existing delivery record
    /// (for idempotent re-delivery).
    ///
    /// If a delivery record with the same ID already exists and is in
    /// `Delivered` status, this is a no-op.
    pub async fn deliver_idempotent(
        &self,
        config: &WebhookConfig,
        payload: &WebhookPayload,
        delivery_id: Uuid,
    ) -> Result<DeliveryRecord> {
        // Check if already delivered.
        match self.store.get(delivery_id).await {
            Ok(existing) if existing.status == DeliveryStatus::Delivered => {
                debug!(
                    delivery_id = %delivery_id,
                    "delivery already completed, skipping"
                );
                return Ok(existing);
            }
            Ok(mut existing) => {
                // Re-attempt with the existing record.
                self.attempt_delivery(config, payload, &mut existing)
                    .await?;
                return Ok(existing);
            }
            Err(WebhookError::DeliveryNotFound(_)) => {
                // Fall through to create a new record.
            }
            Err(e) => return Err(e),
        }

        let mut record = DeliveryRecord::new(payload.webhook_id, &payload.event_type);
        record.id = delivery_id;
        record = self.store.create(record).await?;
        self.attempt_delivery(config, payload, &mut record).await?;
        Ok(record)
    }

    /// Retry all pending failed deliveries.
    ///
    /// Returns the number of deliveries retried.
    pub async fn retry_pending(
        &self,
        config: &WebhookConfig,
        payload_builder: &dyn Fn(&DeliveryRecord) -> Option<WebhookPayload>,
        limit: usize,
    ) -> Result<usize> {
        let pending = self.store.list_pending_retries(limit).await?;
        let mut retried = 0;

        for mut record in pending {
            if config.retry_policy.is_exhausted(record.attempt_count) {
                record.mark_exhausted();
                self.store.update(record).await?;
                continue;
            }

            if let Some(payload) = payload_builder(&record) {
                self.attempt_delivery(config, &payload, &mut record).await?;
                retried += 1;
            }
        }

        Ok(retried)
    }

    /// Perform a single delivery attempt, updating the record in the store.
    async fn attempt_delivery(
        &self,
        config: &WebhookConfig,
        payload: &WebhookPayload,
        record: &mut DeliveryRecord,
    ) -> Result<()> {
        let body = serde_json::to_vec(payload)?;
        let timestamp = Utc::now().to_rfc3339();
        let sig = signature::sign(&config.secret, &timestamp, &body);

        debug!(
            delivery_id = %record.id,
            url = %config.url,
            event_type = %record.event_type,
            "attempting webhook delivery"
        );

        let result = self
            .client
            .post(&config.url)
            .header("Content-Type", "application/json")
            .header(signature::SIGNATURE_HEADER, &sig)
            .header(signature::TIMESTAMP_HEADER, &timestamp)
            .body(body)
            .timeout(config.timeout)
            .send()
            .await;

        match result {
            Ok(response) => {
                let status = response.status().as_u16();
                if response.status().is_success() {
                    info!(
                        delivery_id = %record.id,
                        status,
                        "webhook delivered successfully"
                    );
                    record.mark_delivered(status);
                } else {
                    let body = response
                        .text()
                        .await
                        .unwrap_or_else(|_| String::from("<unreadable>"));
                    let truncated = if body.len() > 500 {
                        format!("{}...", &body[..500])
                    } else {
                        body
                    };
                    warn!(
                        delivery_id = %record.id,
                        status,
                        body = %truncated,
                        "webhook delivery received non-success response"
                    );
                    let next_retry = compute_next_retry(config, record.attempt_count);
                    record.mark_failed(
                        Some(status),
                        format!("HTTP {status}: {truncated}"),
                        next_retry,
                    );
                }
            }
            Err(e) => {
                let msg = e.to_string();
                error!(
                    delivery_id = %record.id,
                    error = %msg,
                    "webhook delivery HTTP error"
                );
                let is_timeout = e.is_timeout();
                let next_retry = compute_next_retry(config, record.attempt_count);
                if is_timeout {
                    record.mark_failed(None, format!("timeout: {msg}"), next_retry);
                } else {
                    record.mark_failed(None, msg, next_retry);
                }
            }
        }

        self.store.update(record.clone()).await?;
        Ok(())
    }
}

/// Compute the next retry time, or `None` if retries are exhausted.
fn compute_next_retry(
    config: &WebhookConfig,
    current_attempt_count: u32,
) -> Option<chrono::DateTime<Utc>> {
    if config.retry_policy.is_exhausted(current_attempt_count + 1) {
        None
    } else {
        let delay = config.retry_policy.delay_for_attempt(current_attempt_count);
        Some(
            Utc::now() + chrono::Duration::from_std(delay).unwrap_or(chrono::Duration::seconds(30)),
        )
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::WebhookConfig;
    use crate::memory_store::InMemoryDeliveryStore;
    use crate::payload::WebhookPayload;
    use serde_json::json;

    fn test_config(url: &str) -> WebhookConfig {
        WebhookConfig::new(url, "test-secret")
    }

    fn test_payload(webhook_id: Uuid) -> WebhookPayload {
        WebhookPayload::new(
            "run.started",
            webhook_id,
            Uuid::now_v7(),
            json!({"run_id": "test-run"}),
        )
    }

    #[tokio::test]
    async fn delivery_creates_record_in_store() {
        let store = Arc::new(InMemoryDeliveryStore::new());
        let engine = WebhookDelivery::new(Arc::clone(&store));
        let wid = Uuid::now_v7();
        let payload = test_payload(wid);

        // This will fail HTTP (no server) but should still create a record.
        let config = test_config("http://127.0.0.1:1/nonexistent");
        let record = engine.deliver(&config, &payload).await.expect("deliver");

        assert_eq!(record.webhook_id, wid);
        assert_eq!(record.event_type, "run.started");
        // Should be failed since no server is listening.
        assert_eq!(record.status, DeliveryStatus::Failed);
        assert_eq!(store.len(), 1);
    }

    #[tokio::test]
    async fn idempotent_delivery_skips_if_already_delivered() {
        let store = Arc::new(InMemoryDeliveryStore::new());
        let wid = Uuid::now_v7();
        let delivery_id = Uuid::now_v7();

        // Pre-create a delivered record.
        let mut record = DeliveryRecord::new(wid, "run.started");
        record.id = delivery_id;
        record.mark_delivered(200);
        store.create(record).await.expect("create");

        let engine = WebhookDelivery::new(Arc::clone(&store));
        let payload = WebhookPayload::new("run.started", wid, delivery_id, json!({}));
        let config = test_config("http://127.0.0.1:1/nonexistent");

        let result = engine
            .deliver_idempotent(&config, &payload, delivery_id)
            .await
            .expect("idempotent");

        // Should return the existing record without re-attempting.
        assert_eq!(result.status, DeliveryStatus::Delivered);
        assert_eq!(result.id, delivery_id);
        // Only 1 record in the store (the original).
        assert_eq!(store.len(), 1);
    }

    #[tokio::test]
    async fn failed_delivery_records_error() {
        let store = Arc::new(InMemoryDeliveryStore::new());
        let engine = WebhookDelivery::new(Arc::clone(&store));
        let wid = Uuid::now_v7();
        let payload = test_payload(wid);
        let config = test_config("http://127.0.0.1:1/nonexistent");

        let record = engine.deliver(&config, &payload).await.expect("deliver");
        assert_eq!(record.status, DeliveryStatus::Failed);
        assert!(record.last_error.is_some());
        assert_eq!(record.attempt_count, 1);
    }
}
