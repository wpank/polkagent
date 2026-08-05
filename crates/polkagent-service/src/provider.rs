//! Provider registry for managing configured model providers.
//!
//! The [`ProviderRegistry`] holds configured AI model providers and exposes
//! them by identifier. Each provider maps to a [`ModelExecutor`] implementation
//! that the run engine uses to send inference requests.

use std::collections::HashMap;
use std::sync::Arc;

use polkagent_config::ProviderConfig;
use polkagent_executor_trait::ModelExecutor;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::error::ServiceError;

// ---------------------------------------------------------------------------
// ProviderInfo
// ---------------------------------------------------------------------------

/// Summary information about a registered provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderInfo {
    /// Stable identifier for this provider.
    pub id: String,
    /// Provider type (e.g. `"anthropic"`, `"openai_compatible"`, `"local"`).
    pub provider_type: String,
    /// Model identifiers this provider is configured to serve.
    pub models: Vec<String>,
    /// Current health status of the provider.
    pub status: ProviderStatus,
}

/// Health status of a provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderStatus {
    /// The provider has been registered but not yet health-checked.
    Registered,
    /// The provider passed its most recent health check.
    Healthy,
    /// The provider failed its most recent health check.
    Unhealthy,
}

// ---------------------------------------------------------------------------
// ProviderRegistry
// ---------------------------------------------------------------------------

/// Manages configured model providers and their executor instances.
///
/// The registry is the single source of truth for which providers are
/// available and routes requests to the appropriate [`ModelExecutor`].
///
/// # Thread safety
///
/// `ProviderRegistry` is `Send + Sync`. All internal state is protected by
/// the outer `Arc` wrapping. Registration is intended to happen at startup
/// before concurrent access begins.
#[derive(Debug)]
pub struct ProviderRegistry {
    /// Registered providers keyed by their stable identifier.
    entries: HashMap<String, ProviderEntry>,
}

struct ProviderEntry {
    config: ProviderConfig,
    executor: Arc<dyn ModelExecutor>,
    status: ProviderStatus,
}

impl std::fmt::Debug for ProviderEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderEntry")
            .field("config", &self.config)
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}

impl ProviderRegistry {
    /// Create an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    /// Register a provider with the given configuration and executor.
    ///
    /// If a provider with the same `id` is already registered, it is replaced
    /// and a warning is logged.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::Config`] if the provider config is missing a
    /// required field (e.g. empty `id`).
    pub fn register(
        &mut self,
        config: ProviderConfig,
        executor: Arc<dyn ModelExecutor>,
    ) -> Result<(), ServiceError> {
        if config.id.is_empty() {
            return Err(ServiceError::Config {
                message: "provider config must have a non-empty id".into(),
            });
        }

        if self.entries.contains_key(&config.id) {
            warn!(provider_id = %config.id, "replacing existing provider registration");
        }

        info!(
            provider_id = %config.id,
            provider_type = %config.provider_type,
            "registering provider"
        );

        self.entries.insert(
            config.id.clone(),
            ProviderEntry {
                config,
                executor,
                status: ProviderStatus::Registered,
            },
        );

        Ok(())
    }

    /// Retrieve the executor for a provider by its identifier.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::ProviderNotFound`] if no provider with the
    /// given `provider_id` is registered.
    pub fn get_executor(&self, provider_id: &str) -> Result<Arc<dyn ModelExecutor>, ServiceError> {
        self.entries
            .get(provider_id)
            .map(|entry| Arc::clone(&entry.executor))
            .ok_or_else(|| ServiceError::ProviderNotFound {
                provider_id: provider_id.to_owned(),
            })
    }

    /// List summary information for all registered providers.
    #[must_use]
    pub fn list_providers(&self) -> Vec<ProviderInfo> {
        self.entries
            .values()
            .map(|entry| ProviderInfo {
                id: entry.config.id.clone(),
                provider_type: entry.config.provider_type.clone(),
                models: if entry.config.default_model.is_empty() {
                    vec![]
                } else {
                    vec![entry.config.default_model.clone()]
                },
                status: entry.status,
            })
            .collect()
    }

    /// Update the health status of a provider.
    ///
    /// Returns `false` if the provider is not registered.
    pub fn set_status(&mut self, provider_id: &str, status: ProviderStatus) -> bool {
        if let Some(entry) = self.entries.get_mut(provider_id) {
            entry.status = status;
            true
        } else {
            false
        }
    }

    /// Return the number of registered providers.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Return `true` if no providers are registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl Default for ProviderRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use polkagent_executor_trait::{
        ExecutorError, InferenceRequest, InferenceResponse, StreamEvent, TokenUsage,
    };

    // ── Fake executor ────────────────────────────────────────────────────

    #[derive(Debug)]
    struct FakeExecutor;

    #[async_trait::async_trait]
    impl ModelExecutor for FakeExecutor {
        async fn complete(
            &self,
            _request: InferenceRequest,
        ) -> Result<InferenceResponse, ExecutorError> {
            Ok(InferenceResponse {
                text: "fake".into(),
                tool_calls: vec![],
                stop_reason: "end_turn".into(),
                usage: TokenUsage::default(),
                provider_request_id: None,
            })
        }

        async fn stream(
            &self,
            _request: InferenceRequest,
        ) -> Result<
            Box<dyn futures::Stream<Item = Result<StreamEvent, ExecutorError>> + Send + Unpin>,
            ExecutorError,
        > {
            Err(ExecutorError::Internal {
                message: "not implemented".into(),
            })
        }

        async fn health(&self) -> Result<(), ExecutorError> {
            Ok(())
        }
    }

    fn fake_config(id: &str) -> ProviderConfig {
        ProviderConfig {
            id: id.to_owned(),
            provider_type: "fake".into(),
            default_model: "fake-model-v1".into(),
            ..Default::default()
        }
    }

    // ── Tests ────────────────────────────────────────────────────────────

    #[test]
    fn register_and_get_executor() {
        let mut registry = ProviderRegistry::new();
        let executor: Arc<dyn ModelExecutor> = Arc::new(FakeExecutor);
        registry
            .register(fake_config("test-provider"), executor)
            .expect("register");

        let got = registry.get_executor("test-provider");
        assert!(got.is_ok());
    }

    #[test]
    fn get_executor_not_found() {
        let registry = ProviderRegistry::new();
        let result = registry.get_executor("nonexistent");
        assert!(matches!(result, Err(ServiceError::ProviderNotFound { .. })));
    }

    #[test]
    fn register_empty_id_returns_error() {
        let mut registry = ProviderRegistry::new();
        let executor: Arc<dyn ModelExecutor> = Arc::new(FakeExecutor);
        let result = registry.register(fake_config(""), executor);
        assert!(matches!(result, Err(ServiceError::Config { .. })));
    }

    #[test]
    fn list_providers_returns_all() {
        let mut registry = ProviderRegistry::new();
        let exec: Arc<dyn ModelExecutor> = Arc::new(FakeExecutor);

        registry
            .register(fake_config("provider-a"), Arc::clone(&exec))
            .expect("register a");
        registry
            .register(fake_config("provider-b"), Arc::clone(&exec))
            .expect("register b");

        let providers = registry.list_providers();
        assert_eq!(providers.len(), 2);

        let ids: Vec<&str> = providers.iter().map(|p| p.id.as_str()).collect();
        assert!(ids.contains(&"provider-a"));
        assert!(ids.contains(&"provider-b"));
    }

    #[test]
    fn set_status_updates_provider() {
        let mut registry = ProviderRegistry::new();
        let exec: Arc<dyn ModelExecutor> = Arc::new(FakeExecutor);
        registry
            .register(fake_config("p1"), exec)
            .expect("register");

        assert!(registry.set_status("p1", ProviderStatus::Healthy));
        let providers = registry.list_providers();
        assert_eq!(providers[0].status, ProviderStatus::Healthy);
    }

    #[test]
    fn set_status_returns_false_for_unknown() {
        let mut registry = ProviderRegistry::new();
        assert!(!registry.set_status("unknown", ProviderStatus::Unhealthy));
    }

    #[test]
    fn len_and_is_empty() {
        let mut registry = ProviderRegistry::new();
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);

        let exec: Arc<dyn ModelExecutor> = Arc::new(FakeExecutor);
        registry
            .register(fake_config("p1"), exec)
            .expect("register");
        assert!(!registry.is_empty());
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn replace_existing_provider() {
        let mut registry = ProviderRegistry::new();
        let exec: Arc<dyn ModelExecutor> = Arc::new(FakeExecutor);
        registry
            .register(fake_config("p1"), Arc::clone(&exec))
            .expect("register first");
        registry
            .register(fake_config("p1"), exec)
            .expect("register second (replace)");
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn provider_status_serde_round_trip() {
        let status = ProviderStatus::Healthy;
        let json = serde_json::to_string(&status).expect("serialize");
        let back: ProviderStatus = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(status, back);
    }

    #[test]
    fn provider_info_serializes() {
        let info = ProviderInfo {
            id: "test".into(),
            provider_type: "anthropic".into(),
            models: vec!["claude-opus-4-6".into()],
            status: ProviderStatus::Healthy,
        };
        let json = serde_json::to_string(&info).expect("serialize");
        assert!(json.contains("anthropic"));
        assert!(json.contains("healthy"));
    }
}
