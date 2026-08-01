//! Webhook subscription registry.
//!
//! [`WebhookRegistry`] manages multiple webhook subscriptions and provides
//! event-type filtering to determine which subscriptions should receive a
//! given event.

use std::collections::HashMap;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::config::WebhookConfig;
use crate::error::{Result, WebhookError};

// ---------------------------------------------------------------------------
// WebhookSubscription
// ---------------------------------------------------------------------------

/// A registered webhook subscription combining an ID with its configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookSubscription {
    /// Unique identifier for this subscription.
    pub id: Uuid,

    /// The webhook configuration (URL, secret, event filter, etc.).
    pub config: WebhookConfig,

    /// Whether this subscription is currently active.
    pub active: bool,
}

impl WebhookSubscription {
    /// Create a new active subscription with a random ID.
    #[must_use]
    pub fn new(config: WebhookConfig) -> Self {
        Self {
            id: Uuid::now_v7(),
            config,
            active: true,
        }
    }

    /// Create a subscription with a specific ID.
    #[must_use]
    pub fn with_id(id: Uuid, config: WebhookConfig) -> Self {
        Self {
            id,
            config,
            active: true,
        }
    }
}

// ---------------------------------------------------------------------------
// WebhookRegistry
// ---------------------------------------------------------------------------

/// Thread-safe registry of webhook subscriptions.
///
/// Supports registering, unregistering, and querying subscriptions by event
/// type. Uses `parking_lot::RwLock` for low-overhead synchronization.
#[derive(Debug, Default)]
pub struct WebhookRegistry {
    subscriptions: RwLock<HashMap<Uuid, WebhookSubscription>>,
}

impl WebhookRegistry {
    /// Create a new, empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a new webhook subscription.
    ///
    /// Returns the subscription ID.
    pub fn register(&self, config: WebhookConfig) -> Uuid {
        let sub = WebhookSubscription::new(config);
        let id = sub.id;
        self.subscriptions.write().insert(id, sub);
        id
    }

    /// Register a subscription with a specific ID.
    ///
    /// # Errors
    ///
    /// Returns [`WebhookError::Duplicate`] if a subscription with the same ID
    /// already exists.
    pub fn register_with_id(&self, id: Uuid, config: WebhookConfig) -> Result<()> {
        let mut map = self.subscriptions.write();
        if map.contains_key(&id) {
            return Err(WebhookError::Duplicate(id));
        }
        map.insert(id, WebhookSubscription::with_id(id, config));
        Ok(())
    }

    /// Remove a webhook subscription.
    ///
    /// # Errors
    ///
    /// Returns [`WebhookError::NotFound`] if no subscription with the given
    /// ID exists.
    pub fn unregister(&self, id: Uuid) -> Result<WebhookSubscription> {
        self.subscriptions
            .write()
            .remove(&id)
            .ok_or(WebhookError::NotFound(id))
    }

    /// Retrieve a subscription by its ID.
    ///
    /// # Errors
    ///
    /// Returns [`WebhookError::NotFound`] if not found.
    pub fn get(&self, id: Uuid) -> Result<WebhookSubscription> {
        self.subscriptions
            .read()
            .get(&id)
            .cloned()
            .ok_or(WebhookError::NotFound(id))
    }

    /// List all registered subscriptions.
    #[must_use]
    pub fn list(&self) -> Vec<WebhookSubscription> {
        self.subscriptions.read().values().cloned().collect()
    }

    /// Find all active subscriptions that accept the given event type.
    ///
    /// This is the primary lookup used by the delivery pipeline to determine
    /// which webhooks should receive a particular event.
    #[must_use]
    pub fn subscriptions_for_event(&self, event_type: &str) -> Vec<WebhookSubscription> {
        self.subscriptions
            .read()
            .values()
            .filter(|s| s.active && s.config.accepts_event(event_type))
            .cloned()
            .collect()
    }

    /// Set whether a subscription is active.
    ///
    /// # Errors
    ///
    /// Returns [`WebhookError::NotFound`] if the subscription does not exist.
    pub fn set_active(&self, id: Uuid, active: bool) -> Result<()> {
        let mut map = self.subscriptions.write();
        let sub = map.get_mut(&id).ok_or(WebhookError::NotFound(id))?;
        sub.active = active;
        Ok(())
    }

    /// Return the number of registered subscriptions.
    #[must_use]
    pub fn len(&self) -> usize {
        self.subscriptions.read().len()
    }

    /// Return `true` if there are no registered subscriptions.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.subscriptions.read().is_empty()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::WebhookConfig;

    fn cfg(events: &[&str]) -> WebhookConfig {
        let mut c = WebhookConfig::new("https://example.com/hook", "secret");
        c.events = events.iter().map(|s| (*s).to_string()).collect();
        c
    }

    #[test]
    fn register_and_get() {
        let reg = WebhookRegistry::new();
        let id = reg.register(cfg(&[]));
        let sub = reg.get(id).expect("get");
        assert_eq!(sub.id, id);
        assert!(sub.active);
    }

    #[test]
    fn register_with_duplicate_id_fails() {
        let reg = WebhookRegistry::new();
        let id = Uuid::now_v7();
        reg.register_with_id(id, cfg(&[])).expect("first");
        let result = reg.register_with_id(id, cfg(&[]));
        assert!(result.is_err());
    }

    #[test]
    fn unregister_removes_subscription() {
        let reg = WebhookRegistry::new();
        let id = reg.register(cfg(&[]));
        reg.unregister(id).expect("unregister");
        assert!(reg.get(id).is_err());
        assert!(reg.is_empty());
    }

    #[test]
    fn unregister_nonexistent_fails() {
        let reg = WebhookRegistry::new();
        assert!(reg.unregister(Uuid::nil()).is_err());
    }

    #[test]
    fn list_returns_all() {
        let reg = WebhookRegistry::new();
        reg.register(cfg(&[]));
        reg.register(cfg(&["run.started"]));
        assert_eq!(reg.list().len(), 2);
    }

    #[test]
    fn subscriptions_for_event_filters_by_event_type() {
        let reg = WebhookRegistry::new();
        reg.register(cfg(&["run.started"]));
        reg.register(cfg(&["run.completed"]));
        reg.register(cfg(&[])); // Accepts all events.

        let started = reg.subscriptions_for_event("run.started");
        assert_eq!(started.len(), 2); // The specific + the wildcard.

        let completed = reg.subscriptions_for_event("run.completed");
        assert_eq!(completed.len(), 2);

        let effect = reg.subscriptions_for_event("effect.executed");
        assert_eq!(effect.len(), 1); // Only the wildcard.
    }

    #[test]
    fn inactive_subscriptions_are_excluded() {
        let reg = WebhookRegistry::new();
        let id = reg.register(cfg(&[]));
        reg.set_active(id, false).expect("deactivate");

        let subs = reg.subscriptions_for_event("run.started");
        assert!(subs.is_empty());
    }

    #[test]
    fn set_active_toggles() {
        let reg = WebhookRegistry::new();
        let id = reg.register(cfg(&[]));

        reg.set_active(id, false).expect("deactivate");
        assert!(!reg.get(id).expect("get").active);

        reg.set_active(id, true).expect("activate");
        assert!(reg.get(id).expect("get").active);
    }

    #[test]
    fn set_active_nonexistent_fails() {
        let reg = WebhookRegistry::new();
        assert!(reg.set_active(Uuid::nil(), true).is_err());
    }

    #[test]
    fn len_and_is_empty() {
        let reg = WebhookRegistry::new();
        assert!(reg.is_empty());
        assert_eq!(reg.len(), 0);

        reg.register(cfg(&[]));
        assert!(!reg.is_empty());
        assert_eq!(reg.len(), 1);
    }
}
