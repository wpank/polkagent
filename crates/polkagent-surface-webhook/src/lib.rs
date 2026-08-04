//! HTTP webhook delivery surface for the Polkagent platform.
//!
//! This crate provides a complete webhook system for delivering agent events
//! and responses to external services over HTTP. Each delivery is signed with
//! HMAC (using BLAKE3 keyed hash) so that receivers can verify authenticity.
//!
//! # Overview
//!
//! - **[`WebhookConfig`]** — per-subscription configuration: URL, signing
//!   secret, event type filter, retry policy, and timeout.
//! - **[`WebhookPayload`]** — the JSON envelope sent in every webhook POST.
//! - **[`signature`]** — HMAC-SHA256 signing and verification utilities.
//! - **[`WebhookDelivery`]** — HTTP POST engine with signing, retries, and
//!   delivery tracking.
//! - **[`WebhookRegistry`]** — manage multiple webhook subscriptions and
//!   filter events by type.
//! - **[`DeliveryStore`]** — trait for persisting delivery attempt records.
//! - **[`InMemoryDeliveryStore`]** — in-memory store for tests and
//!   prototyping.
//! - **[`WebhookService`]** — high-level facade composing the registry,
//!   delivery engine, and store.
//!
//! # Headers
//!
//! Every webhook POST includes:
//!
//! | Header | Contents |
//! |--------|----------|
//! | `Content-Type` | `application/json` |
//! | `X-Polkagent-Signature` | hex-encoded HMAC of `{timestamp}.{body}` |
//! | `X-Polkagent-Timestamp` | RFC-3339 timestamp of the delivery attempt |
//!
//! # Quick start
//!
//! ```rust,no_run
//! use polkagent_surface_webhook::{WebhookConfig, WebhookRegistry, WebhookService};
//! use polkagent_surface_webhook::memory_store::InMemoryDeliveryStore;
//! use std::sync::Arc;
//!
//! let registry = Arc::new(WebhookRegistry::new());
//! let store = Arc::new(InMemoryDeliveryStore::new());
//! let service = WebhookService::new(registry, store);
//!
//! // Register a webhook.
//! let config = WebhookConfig::new("https://example.com/hook", "my-secret");
//! let id = service.register(config);
//! ```

pub mod config;
pub mod delivery;
pub mod error;
pub mod memory_store;
pub mod payload;
pub mod registry;
pub mod signature;
pub mod store;

// ---------------------------------------------------------------------------
// Flat re-exports — the public API surface
// ---------------------------------------------------------------------------

pub use config::{RetryPolicy, WebhookConfig};
pub use delivery::WebhookDelivery;
pub use error::WebhookError;
pub use memory_store::InMemoryDeliveryStore;
pub use payload::WebhookPayload;
pub use registry::{WebhookRegistry, WebhookSubscription};
pub use signature::{sign, verify, SIGNATURE_HEADER, TIMESTAMP_HEADER};
pub use store::{DeliveryRecord, DeliveryStatus, DeliveryStore};

// Re-export core types used in public signatures.
pub use polkagent_event::EventType;

use std::sync::Arc;

use serde_json::Value;
use tracing::debug;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// WebhookService
// ---------------------------------------------------------------------------

/// High-level facade that combines the registry, delivery engine, and store
/// into a single, easy-to-use service.
///
/// This is the recommended entry point for most users.
pub struct WebhookService<S: DeliveryStore> {
    registry: Arc<WebhookRegistry>,
    delivery: WebhookDelivery<S>,
}

impl<S: DeliveryStore> WebhookService<S> {
    /// Create a new webhook service.
    pub fn new(registry: Arc<WebhookRegistry>, store: Arc<S>) -> Self {
        let delivery = WebhookDelivery::new(store);
        Self { registry, delivery }
    }

    /// Register a new webhook subscription.
    ///
    /// Returns the subscription ID.
    pub fn register(&self, config: WebhookConfig) -> Uuid {
        let id = self.registry.register(config);
        debug!(webhook_id = %id, "registered webhook subscription");
        id
    }

    /// Unregister a webhook subscription.
    pub fn unregister(&self, id: Uuid) -> crate::error::Result<WebhookSubscription> {
        let sub = self.registry.unregister(id)?;
        debug!(webhook_id = %id, "unregistered webhook subscription");
        Ok(sub)
    }

    /// Dispatch an event to all matching webhook subscriptions.
    ///
    /// Returns the list of delivery records (one per matching subscription).
    pub async fn dispatch(
        &self,
        event_type: &str,
        data: Value,
    ) -> crate::error::Result<Vec<DeliveryRecord>> {
        let subscriptions = self.registry.subscriptions_for_event(event_type);

        debug!(
            event_type,
            subscription_count = subscriptions.len(),
            "dispatching event to webhooks"
        );

        let mut records = Vec::with_capacity(subscriptions.len());

        for sub in &subscriptions {
            let delivery_id = Uuid::now_v7();
            let payload = WebhookPayload::new(event_type, sub.id, delivery_id, data.clone());

            let record = self.delivery.deliver(&sub.config, &payload).await?;
            records.push(record);
        }

        Ok(records)
    }

    /// Get a reference to the underlying registry.
    pub fn registry(&self) -> &WebhookRegistry {
        &self.registry
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn service_register_and_unregister() {
        let registry = Arc::new(WebhookRegistry::new());
        let store = Arc::new(InMemoryDeliveryStore::new());
        let service = WebhookService::new(registry, store);

        let config = WebhookConfig::new("https://example.com/hook", "secret");
        let id = service.register(config);
        assert_eq!(service.registry().len(), 1);

        service.unregister(id).expect("unregister");
        assert!(service.registry().is_empty());
    }

    #[tokio::test]
    async fn service_dispatch_to_matching_subscriptions() {
        let registry = Arc::new(WebhookRegistry::new());
        let store = Arc::new(InMemoryDeliveryStore::new());
        let service = WebhookService::new(Arc::clone(&registry), Arc::clone(&store));

        // Register two webhooks: one for run events, one for all events.
        let mut run_cfg = WebhookConfig::new("http://127.0.0.1:1/run", "s1");
        run_cfg.events = vec!["run.started".into(), "run.completed".into()];
        service.register(run_cfg);

        let all_cfg = WebhookConfig::new("http://127.0.0.1:1/all", "s2");
        service.register(all_cfg);

        // Dispatch a run.started event -- both should match.
        let records = service
            .dispatch("run.started", json!({"run_id": "r1"}))
            .await
            .expect("dispatch");
        assert_eq!(records.len(), 2);

        // Dispatch an effect.executed event -- only the "all" one should match.
        let records = service
            .dispatch("effect.executed", json!({}))
            .await
            .expect("dispatch");
        assert_eq!(records.len(), 1);
    }

    #[tokio::test]
    async fn service_dispatch_no_matching_subscriptions() {
        let registry = Arc::new(WebhookRegistry::new());
        let store = Arc::new(InMemoryDeliveryStore::new());
        let service = WebhookService::new(registry, store);

        let mut cfg = WebhookConfig::new("http://127.0.0.1:1/hook", "s");
        cfg.events = vec!["run.started".into()];
        service.register(cfg);

        let records = service
            .dispatch("effect.executed", json!({}))
            .await
            .expect("dispatch");
        assert!(records.is_empty());
    }
}
