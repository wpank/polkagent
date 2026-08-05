//! Model router with health-aware routing (PRD-04 §12).
//!
//! The [`ModelRouter`] trait defines the contract for selecting a model +
//! provider for a given request. [`DefaultModelRouter`] implements the
//! standard algorithm:
//!
//! 1. Filter models by required capabilities.
//! 2. Filter by provider health (circuit breaker state).
//! 3. Apply routing policy scoring.
//! 4. Record the decision for audit.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::negotiate::{negotiate, Capability, CapabilityRequirement, NegotiatedCapabilities};
use polkagent_config::model_registry::{ModelCatalog, ModelDescriptor};
use polkagent_retry::provider_health::HealthState;

// ---------------------------------------------------------------------------
// RouteError
// ---------------------------------------------------------------------------

/// Errors that can occur during routing.
#[derive(Debug, Error)]
pub enum RouteError {
    /// No model satisfies the required capabilities.
    #[error("no model satisfies required capabilities: {missing:?}")]
    NoCapableModel {
        /// The capabilities that could not be satisfied.
        missing: Vec<Capability>,
    },

    /// All capable models are unhealthy.
    #[error("all capable models are unhealthy")]
    AllUnhealthy,

    /// The requested model was not found in the catalog.
    #[error("model not found: {model}")]
    ModelNotFound {
        /// The model slug that was requested.
        model: String,
    },

    /// The requested provider is not registered.
    #[error("provider not found: {provider}")]
    ProviderNotFound {
        /// The provider identifier that was requested.
        provider: String,
    },

    /// An internal routing error.
    #[error("routing error: {message}")]
    Internal {
        /// Human-readable description.
        message: String,
    },
}

// ---------------------------------------------------------------------------
// RoutingPolicy
// ---------------------------------------------------------------------------

/// The policy used to score and rank candidate models.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum RoutingPolicy {
    /// Use the exact model specified; fail if unavailable.
    Exact,
    /// Minimize cost per million tokens.
    #[default]
    CostOptimized,
    /// Prefer models with lower latency (based on health metrics).
    LatencyOptimized,
    /// Maximize the number of capabilities available.
    CapabilityMaximized,
    /// A named custom policy. The router treats this the same as
    /// `CostOptimized` unless a custom scorer is registered.
    Custom(String),
}

impl fmt::Display for RoutingPolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Exact => write!(f, "exact"),
            Self::CostOptimized => write!(f, "cost_optimized"),
            Self::LatencyOptimized => write!(f, "latency_optimized"),
            Self::CapabilityMaximized => write!(f, "capability_maximized"),
            Self::Custom(name) => write!(f, "custom({name})"),
        }
    }
}

// ---------------------------------------------------------------------------
// RouteRequest
// ---------------------------------------------------------------------------

/// A request for the router to select a model + provider.
#[derive(Debug, Clone)]
pub struct RouteRequest {
    /// Capabilities the model must support.
    pub required_capabilities: Vec<CapabilityRequirement>,
    /// If set, the router should prefer this specific model.
    pub preferred_model: Option<String>,
    /// If set, the router should prefer this specific provider.
    pub preferred_provider: Option<String>,
    /// Maximum cost per million input tokens (USD).
    pub max_cost_input_per_m: Option<f64>,
    /// The routing policy to apply.
    pub policy: RoutingPolicy,
}

// ---------------------------------------------------------------------------
// SelectedRoute
// ---------------------------------------------------------------------------

/// The selected model + provider route.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelectedRoute {
    /// The model slug selected.
    pub model: String,
    /// The provider that serves this model.
    pub provider: String,
    /// Why this route was selected.
    pub reason: RouteReason,
}

// ---------------------------------------------------------------------------
// RouteReason
// ---------------------------------------------------------------------------

/// Explains why a particular route was selected.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteReason {
    /// The exact requested model was available and healthy.
    ExactMatch,
    /// Selected based on cost optimization.
    LowestCost {
        /// Cost per million input tokens.
        cost_input_per_m: f64,
    },
    /// Selected based on latency optimization.
    LowestLatency,
    /// Selected because it has the most capabilities.
    MostCapabilities {
        /// Number of capabilities available.
        capability_count: usize,
    },
    /// A preferred model was chosen because it was healthy.
    PreferredAndHealthy,
    /// The only model that was capable and healthy.
    OnlyOption,
}

impl fmt::Display for RouteReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ExactMatch => write!(f, "exact match"),
            Self::LowestCost { cost_input_per_m } => {
                write!(f, "lowest cost (${cost_input_per_m}/M input)")
            }
            Self::LowestLatency => write!(f, "lowest latency"),
            Self::MostCapabilities { capability_count } => {
                write!(f, "most capabilities ({capability_count})")
            }
            Self::PreferredAndHealthy => write!(f, "preferred model is healthy"),
            Self::OnlyOption => write!(f, "only available option"),
        }
    }
}

// ---------------------------------------------------------------------------
// RouteDecision (audit trail)
// ---------------------------------------------------------------------------

/// A recorded routing decision for audit and observability.
#[derive(Debug, Clone)]
pub struct RouteDecision {
    /// The route that was selected.
    pub selected: SelectedRoute,
    /// Other routes that were considered but not selected.
    pub alternatives: Vec<SelectedRoute>,
    /// The policy that was used.
    pub policy: RoutingPolicy,
    /// When the decision was made.
    pub timestamp: Instant,
}

// ---------------------------------------------------------------------------
// ModelRouter trait
// ---------------------------------------------------------------------------

/// The model router port trait.
///
/// Implementations select the best `(model, provider)` pair for a given
/// request, considering capabilities, health, and routing policy.
#[async_trait]
pub trait ModelRouter: Send + Sync + 'static {
    /// Select a model + provider for the given request.
    async fn route(&self, request: &RouteRequest) -> Result<RouteDecision, RouteError>;
}

// ---------------------------------------------------------------------------
// ProviderHealth (for the router to query)
// ---------------------------------------------------------------------------

/// Health information about a provider, used by the router to filter out
/// unhealthy providers.
#[derive(Debug, Clone)]
pub struct ProviderHealthInfo {
    /// The health state of this provider.
    pub state: HealthState,
    /// P50 latency in milliseconds (0 if unknown).
    pub latency_p50_ms: u64,
}

impl Default for ProviderHealthInfo {
    fn default() -> Self {
        Self {
            state: HealthState::Unknown,
            latency_p50_ms: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// DefaultModelRouter
// ---------------------------------------------------------------------------

/// The default model router implementation.
///
/// It uses a [`ModelCatalog`] to look up models, a health map to filter
/// unhealthy providers, and applies the requested [`RoutingPolicy`].
pub struct DefaultModelRouter {
    catalog: Arc<dyn ModelCatalog + Send + Sync>,
    health: Arc<RwLock<HashMap<String, ProviderHealthInfo>>>,
    decisions: Arc<RwLock<Vec<RouteDecision>>>,
}

impl DefaultModelRouter {
    /// Create a new router with the given catalog.
    pub fn new(catalog: Arc<dyn ModelCatalog + Send + Sync>) -> Self {
        Self {
            catalog,
            health: Arc::new(RwLock::new(HashMap::new())),
            decisions: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Update the health status for a provider.
    pub fn set_health(&self, provider_id: &str, info: ProviderHealthInfo) {
        self.health.write().insert(provider_id.to_string(), info);
    }

    /// Get the health status for a provider.
    pub fn get_health(&self, provider_id: &str) -> ProviderHealthInfo {
        self.health
            .read()
            .get(provider_id)
            .cloned()
            .unwrap_or_default()
    }

    /// Return all recorded decisions (for audit).
    pub fn decisions(&self) -> Vec<RouteDecision> {
        self.decisions.read().clone()
    }

    /// Clear recorded decisions.
    pub fn clear_decisions(&self) {
        self.decisions.write().clear();
    }

    /// Record a decision.
    fn record_decision(&self, decision: &RouteDecision) {
        self.decisions.write().push(decision.clone());
    }

    /// Check if a provider is usable (not unhealthy).
    fn is_provider_usable(&self, provider_id: &str) -> bool {
        let health = self.health.read();
        match health.get(provider_id) {
            Some(info) => info.state != HealthState::Unhealthy,
            None => true, // Unknown means we haven't checked; assume usable.
        }
    }

    /// Score a model candidate according to the routing policy.
    #[allow(
        clippy::cast_precision_loss,
        reason = "routing scores are approximate ordering values; source latency and capability counts remain exact"
    )]
    fn score_candidate(
        &self,
        desc: &ModelDescriptor,
        neg: &NegotiatedCapabilities,
        policy: &RoutingPolicy,
    ) -> (f64, RouteReason) {
        match policy {
            RoutingPolicy::Exact => {
                // Exact doesn't score; handled separately.
                (0.0, RouteReason::ExactMatch)
            }
            RoutingPolicy::CostOptimized => {
                let cost = desc.cost_input_per_m.unwrap_or(f64::MAX);
                let reason = RouteReason::LowestCost {
                    cost_input_per_m: cost,
                };
                // Lower cost = better, so negate for max-sort or use directly.
                (cost, reason)
            }
            RoutingPolicy::LatencyOptimized => {
                let health = self.health.read();
                let latency = health.get(&desc.provider).map_or(u64::MAX, |h| {
                    if h.latency_p50_ms == 0 {
                        u64::MAX
                    } else {
                        h.latency_p50_ms
                    }
                });
                (latency as f64, RouteReason::LowestLatency)
            }
            RoutingPolicy::CapabilityMaximized => {
                let count = neg.available.len();
                let reason = RouteReason::MostCapabilities {
                    capability_count: count,
                };
                // Higher count = better, so negate for min-sort.
                (-(count as f64), reason)
            }
            RoutingPolicy::Custom(_) => {
                // Custom policies fall back to cost optimization scoring.
                let cost = desc.cost_input_per_m.unwrap_or(f64::MAX);
                let reason = RouteReason::LowestCost {
                    cost_input_per_m: cost,
                };
                (cost, reason)
            }
        }
    }
}

#[async_trait]
impl ModelRouter for DefaultModelRouter {
    #[allow(
        clippy::too_many_lines,
        reason = "the routing transaction stays contiguous so filtering, health fallback, ranking, and audit recording cannot diverge"
    )]
    async fn route(&self, request: &RouteRequest) -> Result<RouteDecision, RouteError> {
        // Step 1: Handle Exact policy.
        if request.policy == RoutingPolicy::Exact {
            let model_slug =
                request
                    .preferred_model
                    .as_deref()
                    .ok_or_else(|| RouteError::Internal {
                        message: "Exact policy requires preferred_model".into(),
                    })?;

            let desc = self
                .catalog
                .get(model_slug)
                .ok_or_else(|| RouteError::ModelNotFound {
                    model: model_slug.to_string(),
                })?;

            // Check health.
            if !self.is_provider_usable(&desc.provider) {
                return Err(RouteError::AllUnhealthy);
            }

            // Check capabilities.
            let neg = negotiate(
                self.catalog.as_ref(),
                model_slug,
                &request.required_capabilities,
                None,
            );
            if !neg.satisfied {
                let missing = neg
                    .hard_missing()
                    .into_iter()
                    .map(|m| m.capability.clone())
                    .collect();
                return Err(RouteError::NoCapableModel { missing });
            }

            let selected = SelectedRoute {
                model: model_slug.to_string(),
                provider: desc.provider.clone(),
                reason: RouteReason::ExactMatch,
            };
            let decision = RouteDecision {
                selected,
                alternatives: vec![],
                policy: RoutingPolicy::Exact,
                timestamp: Instant::now(),
            };
            self.record_decision(&decision);
            return Ok(decision);
        }

        // Step 2: Collect all models from catalog.
        let all_models = self.catalog.list();

        // Step 3: Filter by capabilities.
        let mut candidates: Vec<(&ModelDescriptor, NegotiatedCapabilities)> = all_models
            .into_iter()
            .map(|desc| {
                let neg = negotiate(
                    self.catalog.as_ref(),
                    &desc.slug,
                    &request.required_capabilities,
                    None,
                );
                (desc, neg)
            })
            .filter(|(_, neg)| neg.satisfied)
            .collect();

        if candidates.is_empty() {
            let missing = request
                .required_capabilities
                .iter()
                .filter(|r| r.hard)
                .map(|r| r.capability.clone())
                .collect();
            return Err(RouteError::NoCapableModel { missing });
        }

        // Step 4: Filter by budget constraint.
        if let Some(max_cost) = request.max_cost_input_per_m {
            candidates.retain(|(desc, _)| desc.cost_input_per_m.is_some_and(|c| c <= max_cost));
            if candidates.is_empty() {
                return Err(RouteError::NoCapableModel { missing: vec![] });
            }
        }

        // Step 5: Filter by provider health.
        let healthy_candidates: Vec<_> = candidates
            .iter()
            .filter(|(desc, _)| self.is_provider_usable(&desc.provider))
            .collect();

        let effective: Vec<(&ModelDescriptor, &NegotiatedCapabilities)> =
            if healthy_candidates.is_empty() {
                // All unhealthy — use all candidates anyway.
                candidates.iter().map(|(desc, neg)| (*desc, neg)).collect()
            } else {
                healthy_candidates
                    .iter()
                    .map(|(desc, neg)| (*desc, neg))
                    .collect()
            };

        // Step 6: Prefer the requested model/provider if available.
        if let Some(pref_model) = &request.preferred_model {
            if let Some((desc, _neg)) = effective.iter().find(|(desc, _)| desc.slug == *pref_model)
            {
                let selected = SelectedRoute {
                    model: desc.slug.clone(),
                    provider: desc.provider.clone(),
                    reason: RouteReason::PreferredAndHealthy,
                };
                let alternatives: Vec<SelectedRoute> = effective
                    .iter()
                    .filter(|(d, _)| d.slug != *pref_model)
                    .map(|(d, n)| {
                        let (_, reason) = self.score_candidate(d, n, &request.policy);
                        SelectedRoute {
                            model: d.slug.clone(),
                            provider: d.provider.clone(),
                            reason,
                        }
                    })
                    .collect();
                let decision = RouteDecision {
                    selected,
                    alternatives,
                    policy: request.policy.clone(),
                    timestamp: Instant::now(),
                };
                self.record_decision(&decision);
                return Ok(decision);
            }
        }

        // Step 7: Score and sort by policy.
        let mut scored: Vec<(f64, &ModelDescriptor, &NegotiatedCapabilities, RouteReason)> =
            effective
                .iter()
                .map(|(desc, neg)| {
                    let (score, reason) = self.score_candidate(desc, neg, &request.policy);
                    (score, *desc, *neg, reason)
                })
                .collect();

        scored.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

        // Step 8: Build the decision.
        if scored.is_empty() {
            return Err(RouteError::AllUnhealthy);
        }

        let (_, best_desc, _, best_reason) = &scored[0];
        let reason = if scored.len() == 1 {
            RouteReason::OnlyOption
        } else {
            best_reason.clone()
        };

        let selected = SelectedRoute {
            model: best_desc.slug.clone(),
            provider: best_desc.provider.clone(),
            reason,
        };

        let alternatives: Vec<SelectedRoute> = scored[1..]
            .iter()
            .map(|(_, desc, _, reason)| SelectedRoute {
                model: desc.slug.clone(),
                provider: desc.provider.clone(),
                reason: reason.clone(),
            })
            .collect();

        let decision = RouteDecision {
            selected,
            alternatives,
            policy: request.policy.clone(),
            timestamp: Instant::now(),
        };
        self.record_decision(&decision);
        Ok(decision)
    }
}

impl fmt::Debug for DefaultModelRouter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DefaultModelRouter")
            .field("health_entries", &self.health.read().len())
            .field("recorded_decisions", &self.decisions.read().len())
            .finish_non_exhaustive()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::negotiate::capabilities_from_descriptor;
    use polkagent_config::model_registry::BuiltInModelCatalog;

    fn test_catalog() -> Arc<dyn ModelCatalog + Send + Sync> {
        Arc::new(BuiltInModelCatalog::new())
    }

    fn tools_req() -> CapabilityRequirement {
        CapabilityRequirement {
            capability: Capability::Tools,
            required_by: "test".into(),
            hard: true,
        }
    }

    fn vision_req() -> CapabilityRequirement {
        CapabilityRequirement {
            capability: Capability::Vision,
            required_by: "test".into(),
            hard: true,
        }
    }

    fn web_search_req() -> CapabilityRequirement {
        CapabilityRequirement {
            capability: Capability::WebSearch,
            required_by: "test".into(),
            hard: true,
        }
    }

    fn structured_output_req() -> CapabilityRequirement {
        CapabilityRequirement {
            capability: Capability::StructuredOutput,
            required_by: "test".into(),
            hard: true,
        }
    }

    // ── Exact routing ─────────────────────────────────────────────────

    #[tokio::test]
    async fn exact_route_succeeds() {
        let router = DefaultModelRouter::new(test_catalog());
        let request = RouteRequest {
            required_capabilities: vec![tools_req()],
            preferred_model: Some("claude-opus-4-6".into()),
            preferred_provider: None,
            max_cost_input_per_m: None,
            policy: RoutingPolicy::Exact,
        };
        let decision = router.route(&request).await.expect("should succeed");
        assert_eq!(decision.selected.model, "claude-opus-4-6");
        assert_eq!(decision.selected.provider, "anthropic");
        assert!(matches!(decision.selected.reason, RouteReason::ExactMatch));
        assert!(decision.alternatives.is_empty());
    }

    #[tokio::test]
    async fn exact_route_model_not_found() {
        let router = DefaultModelRouter::new(test_catalog());
        let request = RouteRequest {
            required_capabilities: vec![],
            preferred_model: Some("nonexistent-model".into()),
            preferred_provider: None,
            max_cost_input_per_m: None,
            policy: RoutingPolicy::Exact,
        };
        let err = router.route(&request).await.unwrap_err();
        assert!(matches!(err, RouteError::ModelNotFound { .. }));
    }

    #[tokio::test]
    async fn exact_route_no_preferred_model() {
        let router = DefaultModelRouter::new(test_catalog());
        let request = RouteRequest {
            required_capabilities: vec![],
            preferred_model: None,
            preferred_provider: None,
            max_cost_input_per_m: None,
            policy: RoutingPolicy::Exact,
        };
        let err = router.route(&request).await.unwrap_err();
        assert!(matches!(err, RouteError::Internal { .. }));
    }

    #[tokio::test]
    async fn exact_route_capability_mismatch() {
        let router = DefaultModelRouter::new(test_catalog());
        let request = RouteRequest {
            required_capabilities: vec![tools_req()],
            preferred_model: Some("sonar".into()),
            preferred_provider: None,
            max_cost_input_per_m: None,
            policy: RoutingPolicy::Exact,
        };
        // sonar doesn't support tools
        let err = router.route(&request).await.unwrap_err();
        assert!(matches!(err, RouteError::NoCapableModel { .. }));
    }

    #[tokio::test]
    async fn exact_route_unhealthy_provider() {
        let router = DefaultModelRouter::new(test_catalog());
        router.set_health(
            "anthropic",
            ProviderHealthInfo {
                state: HealthState::Unhealthy,
                latency_p50_ms: 0,
            },
        );
        let request = RouteRequest {
            required_capabilities: vec![],
            preferred_model: Some("claude-opus-4-6".into()),
            preferred_provider: None,
            max_cost_input_per_m: None,
            policy: RoutingPolicy::Exact,
        };
        let err = router.route(&request).await.unwrap_err();
        assert!(matches!(err, RouteError::AllUnhealthy));
    }

    // ── Cost optimized routing ────────────────────────────────────────

    #[tokio::test]
    async fn cost_optimized_picks_cheapest() {
        let router = DefaultModelRouter::new(test_catalog());
        let request = RouteRequest {
            required_capabilities: vec![tools_req(), vision_req()],
            preferred_model: None,
            preferred_provider: None,
            max_cost_input_per_m: None,
            policy: RoutingPolicy::CostOptimized,
        };
        let decision = router.route(&request).await.expect("should succeed");
        // The cheapest models with tools+vision are gemini-2.5-flash and
        // gpt-5.4-mini, both at $0.15/M. HashMap ordering is non-deterministic,
        // so accept either.
        let cheapest = &["gemini-2.5-flash", "gpt-5.4-mini"];
        assert!(
            cheapest.contains(&decision.selected.model.as_str()),
            "expected one of {cheapest:?}, got {}",
            decision.selected.model,
        );
        assert!(matches!(
            decision.selected.reason,
            RouteReason::LowestCost { .. }
        ));
        assert!(!decision.alternatives.is_empty());
    }

    #[tokio::test]
    async fn cost_optimized_with_budget_filter() {
        let router = DefaultModelRouter::new(test_catalog());
        let request = RouteRequest {
            required_capabilities: vec![tools_req()],
            preferred_model: None,
            preferred_provider: None,
            max_cost_input_per_m: Some(1.0),
            policy: RoutingPolicy::CostOptimized,
        };
        let decision = router.route(&request).await.expect("should succeed");
        // Only models under $1/M input should be included
        let catalog = BuiltInModelCatalog::new();
        let desc = catalog.get(&decision.selected.model).expect("model exists");
        assert!(desc.cost_input_per_m.unwrap_or(0.0) <= 1.0);
    }

    // ── Capability maximized routing ──────────────────────────────────

    #[tokio::test]
    async fn capability_maximized_picks_most_capable() {
        let router = DefaultModelRouter::new(test_catalog());
        let request = RouteRequest {
            required_capabilities: vec![],
            preferred_model: None,
            preferred_provider: None,
            max_cost_input_per_m: None,
            policy: RoutingPolicy::CapabilityMaximized,
        };
        let decision = router.route(&request).await.expect("should succeed");
        // Gemini models have the most capabilities (tools, thinking, vision,
        // streaming, caching, structured_output, web_search = 7)
        let cap_count = capabilities_from_descriptor(
            BuiltInModelCatalog::new()
                .get(&decision.selected.model)
                .expect("exists"),
        )
        .len();
        assert!(
            cap_count >= 7,
            "selected model should have >= 7 capabilities, has {cap_count}"
        );
    }

    // ── Latency optimized routing ─────────────────────────────────────

    #[tokio::test]
    async fn latency_optimized_picks_fastest_provider() {
        let router = DefaultModelRouter::new(test_catalog());
        router.set_health(
            "anthropic",
            ProviderHealthInfo {
                state: HealthState::Healthy,
                latency_p50_ms: 500,
            },
        );
        router.set_health(
            "openai",
            ProviderHealthInfo {
                state: HealthState::Healthy,
                latency_p50_ms: 200,
            },
        );
        router.set_health(
            "gemini",
            ProviderHealthInfo {
                state: HealthState::Healthy,
                latency_p50_ms: 300,
            },
        );
        let request = RouteRequest {
            required_capabilities: vec![tools_req()],
            preferred_model: None,
            preferred_provider: None,
            max_cost_input_per_m: None,
            policy: RoutingPolicy::LatencyOptimized,
        };
        let decision = router.route(&request).await.expect("should succeed");
        // Should pick an OpenAI model (lowest latency at 200ms)
        assert_eq!(decision.selected.provider, "openai");
    }

    // ── Preferred model/provider ──────────────────────────────────────

    #[tokio::test]
    async fn preferred_model_chosen_when_healthy() {
        let router = DefaultModelRouter::new(test_catalog());
        let request = RouteRequest {
            required_capabilities: vec![tools_req()],
            preferred_model: Some("gpt-4o".into()),
            preferred_provider: None,
            max_cost_input_per_m: None,
            policy: RoutingPolicy::CostOptimized,
        };
        let decision = router.route(&request).await.expect("should succeed");
        assert_eq!(decision.selected.model, "gpt-4o");
        assert!(matches!(
            decision.selected.reason,
            RouteReason::PreferredAndHealthy
        ));
    }

    #[tokio::test]
    async fn preferred_model_skipped_when_unhealthy() {
        let router = DefaultModelRouter::new(test_catalog());
        router.set_health(
            "openai",
            ProviderHealthInfo {
                state: HealthState::Unhealthy,
                latency_p50_ms: 0,
            },
        );
        let request = RouteRequest {
            required_capabilities: vec![tools_req()],
            preferred_model: Some("gpt-4o".into()),
            preferred_provider: None,
            max_cost_input_per_m: None,
            policy: RoutingPolicy::CostOptimized,
        };
        let decision = router.route(&request).await.expect("should succeed");
        // Should pick something other than gpt-4o since openai is unhealthy
        assert_ne!(decision.selected.provider, "openai");
    }

    #[tokio::test]
    async fn preferred_model_skipped_when_incapable() {
        let router = DefaultModelRouter::new(test_catalog());
        let request = RouteRequest {
            required_capabilities: vec![web_search_req()],
            preferred_model: Some("claude-opus-4-6".into()),
            preferred_provider: None,
            max_cost_input_per_m: None,
            policy: RoutingPolicy::CostOptimized,
        };
        // claude-opus doesn't support web search, so it should be filtered out
        let decision = router.route(&request).await.expect("should succeed");
        assert_ne!(decision.selected.model, "claude-opus-4-6");
    }

    // ── Health filtering ──────────────────────────────────────────────

    #[tokio::test]
    async fn degraded_provider_is_still_usable() {
        let router = DefaultModelRouter::new(test_catalog());
        // Mark all providers unhealthy except anthropic which is degraded.
        router.set_health(
            "anthropic",
            ProviderHealthInfo {
                state: HealthState::Degraded,
                latency_p50_ms: 100,
            },
        );
        router.set_health(
            "openai",
            ProviderHealthInfo {
                state: HealthState::Unhealthy,
                latency_p50_ms: 0,
            },
        );
        router.set_health(
            "gemini",
            ProviderHealthInfo {
                state: HealthState::Unhealthy,
                latency_p50_ms: 0,
            },
        );
        router.set_health(
            "perplexity",
            ProviderHealthInfo {
                state: HealthState::Unhealthy,
                latency_p50_ms: 0,
            },
        );
        let request = RouteRequest {
            required_capabilities: vec![tools_req()],
            preferred_model: None,
            preferred_provider: None,
            max_cost_input_per_m: None,
            policy: RoutingPolicy::CostOptimized,
        };
        let decision = router.route(&request).await.expect("should succeed");
        // Degraded is not unhealthy, so anthropic should still be usable.
        assert_eq!(decision.selected.provider, "anthropic");
    }

    #[tokio::test]
    async fn unknown_health_treated_as_usable() {
        let router = DefaultModelRouter::new(test_catalog());
        // No health data set; should treat all as usable
        let request = RouteRequest {
            required_capabilities: vec![tools_req()],
            preferred_model: None,
            preferred_provider: None,
            max_cost_input_per_m: None,
            policy: RoutingPolicy::CostOptimized,
        };
        let decision = router.route(&request).await.expect("should succeed");
        assert!(!decision.selected.model.is_empty());
    }

    // ── No capable model ──────────────────────────────────────────────

    #[tokio::test]
    async fn no_capable_model_returns_error() {
        let router = DefaultModelRouter::new(test_catalog());
        let request = RouteRequest {
            required_capabilities: vec![CapabilityRequirement {
                capability: Capability::MinContextWindow(100_000_000),
                required_by: "impossible".into(),
                hard: true,
            }],
            preferred_model: None,
            preferred_provider: None,
            max_cost_input_per_m: None,
            policy: RoutingPolicy::CostOptimized,
        };
        let err = router.route(&request).await.unwrap_err();
        assert!(matches!(err, RouteError::NoCapableModel { .. }));
    }

    // ── Decision recording ────────────────────────────────────────────

    #[tokio::test]
    async fn decisions_are_recorded() {
        let router = DefaultModelRouter::new(test_catalog());
        let request = RouteRequest {
            required_capabilities: vec![tools_req()],
            preferred_model: None,
            preferred_provider: None,
            max_cost_input_per_m: None,
            policy: RoutingPolicy::CostOptimized,
        };
        assert!(router.decisions().is_empty());
        let _ = router.route(&request).await;
        assert_eq!(router.decisions().len(), 1);
        let _ = router.route(&request).await;
        assert_eq!(router.decisions().len(), 2);
        router.clear_decisions();
        assert!(router.decisions().is_empty());
    }

    // ── RoutingPolicy serde ───────────────────────────────────────────

    #[test]
    fn routing_policy_serde_round_trip() {
        let policies = vec![
            RoutingPolicy::Exact,
            RoutingPolicy::CostOptimized,
            RoutingPolicy::LatencyOptimized,
            RoutingPolicy::CapabilityMaximized,
            RoutingPolicy::Custom("my_scorer".into()),
        ];
        for policy in &policies {
            let json = serde_json::to_string(policy).expect("serialize");
            let back: RoutingPolicy = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(*policy, back);
        }
    }

    #[test]
    fn routing_policy_default_is_cost_optimized() {
        assert_eq!(RoutingPolicy::default(), RoutingPolicy::CostOptimized);
    }

    #[test]
    fn routing_policy_display() {
        assert_eq!(RoutingPolicy::Exact.to_string(), "exact");
        assert_eq!(RoutingPolicy::CostOptimized.to_string(), "cost_optimized");
        assert_eq!(
            RoutingPolicy::LatencyOptimized.to_string(),
            "latency_optimized"
        );
        assert_eq!(
            RoutingPolicy::CapabilityMaximized.to_string(),
            "capability_maximized"
        );
        assert_eq!(
            RoutingPolicy::Custom("weighted".into()).to_string(),
            "custom(weighted)"
        );
    }

    // ── RouteReason Display ───────────────────────────────────────────

    #[test]
    fn route_reason_display() {
        assert_eq!(format!("{}", RouteReason::ExactMatch), "exact match");
        assert!(format!(
            "{}",
            RouteReason::LowestCost {
                cost_input_per_m: 1.5
            }
        )
        .contains("1.5"));
        assert_eq!(format!("{}", RouteReason::LowestLatency), "lowest latency");
        assert!(format!(
            "{}",
            RouteReason::MostCapabilities {
                capability_count: 7
            }
        )
        .contains('7'));
        assert!(format!("{}", RouteReason::PreferredAndHealthy).contains("preferred"));
        assert!(format!("{}", RouteReason::OnlyOption).contains("only"));
    }

    // ── RouteError Display ────────────────────────────────────────────

    #[test]
    fn route_error_display() {
        let e1 = RouteError::NoCapableModel {
            missing: vec![Capability::Tools],
        };
        assert!(format!("{e1}").contains("no model"));

        let e2 = RouteError::AllUnhealthy;
        assert!(format!("{e2}").contains("unhealthy"));

        let e3 = RouteError::ModelNotFound {
            model: "test".into(),
        };
        assert!(format!("{e3}").contains("test"));

        let e4 = RouteError::ProviderNotFound {
            provider: "prov".into(),
        };
        assert!(format!("{e4}").contains("prov"));

        let e5 = RouteError::Internal {
            message: "oops".into(),
        };
        assert!(format!("{e5}").contains("oops"));
    }

    // ── SelectedRoute serde ───────────────────────────────────────────

    #[test]
    fn selected_route_serde_round_trip() {
        let route = SelectedRoute {
            model: "claude-opus-4-6".into(),
            provider: "anthropic".into(),
            reason: RouteReason::ExactMatch,
        };
        let json = serde_json::to_string(&route).expect("serialize");
        let back: SelectedRoute = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.model, "claude-opus-4-6");
        assert_eq!(back.provider, "anthropic");
    }

    // ── DefaultModelRouter health management ──────────────────────────

    #[test]
    fn set_and_get_health() {
        let router = DefaultModelRouter::new(test_catalog());
        router.set_health(
            "test-prov",
            ProviderHealthInfo {
                state: HealthState::Healthy,
                latency_p50_ms: 42,
            },
        );
        let info = router.get_health("test-prov");
        assert_eq!(info.state, HealthState::Healthy);
        assert_eq!(info.latency_p50_ms, 42);
    }

    #[test]
    fn get_health_unknown_returns_default() {
        let router = DefaultModelRouter::new(test_catalog());
        let info = router.get_health("nonexistent");
        assert_eq!(info.state, HealthState::Unknown);
        assert_eq!(info.latency_p50_ms, 0);
    }

    // ── Debug impl ────────────────────────────────────────────────────

    #[test]
    fn default_router_debug() {
        let router = DefaultModelRouter::new(test_catalog());
        let debug = format!("{router:?}");
        assert!(debug.contains("DefaultModelRouter"));
    }

    // ── Empty requirements routes to cheapest ─────────────────────────

    #[tokio::test]
    async fn no_requirements_routes_to_cheapest_model() {
        let router = DefaultModelRouter::new(test_catalog());
        let request = RouteRequest {
            required_capabilities: vec![],
            preferred_model: None,
            preferred_provider: None,
            max_cost_input_per_m: None,
            policy: RoutingPolicy::CostOptimized,
        };
        let decision = router.route(&request).await.expect("should succeed");
        // With no requirements, the cheapest model should be selected
        assert!(!decision.selected.model.is_empty());
    }

    // ── Structured output filtering ───────────────────────────────────

    #[tokio::test]
    async fn structured_output_filters_correctly() {
        let router = DefaultModelRouter::new(test_catalog());
        let request = RouteRequest {
            required_capabilities: vec![structured_output_req()],
            preferred_model: None,
            preferred_provider: None,
            max_cost_input_per_m: None,
            policy: RoutingPolicy::CostOptimized,
        };
        let decision = router.route(&request).await.expect("should succeed");
        // Anthropic models don't support structured output, so they should be excluded
        let catalog = BuiltInModelCatalog::new();
        let desc = catalog.get(&decision.selected.model).expect("exists");
        assert!(desc.supports_structured_output);
    }

    // ── Custom policy ────────────────────────────────────────────────

    #[tokio::test]
    async fn custom_policy_falls_back_to_cost_scoring() {
        let router = DefaultModelRouter::new(test_catalog());
        let request = RouteRequest {
            required_capabilities: vec![tools_req()],
            preferred_model: None,
            preferred_provider: None,
            max_cost_input_per_m: None,
            policy: RoutingPolicy::Custom("my_scorer".into()),
        };
        let decision = router.route(&request).await.expect("should succeed");
        assert_eq!(decision.policy, RoutingPolicy::Custom("my_scorer".into()));
        // Should behave like cost-optimized (fallback)
        assert!(!decision.selected.model.is_empty());
    }

    // ── Decision timestamp ───────────────────────────────────────────

    #[tokio::test]
    async fn decision_has_timestamp() {
        let router = DefaultModelRouter::new(test_catalog());
        let before = Instant::now();
        let request = RouteRequest {
            required_capabilities: vec![tools_req()],
            preferred_model: None,
            preferred_provider: None,
            max_cost_input_per_m: None,
            policy: RoutingPolicy::CostOptimized,
        };
        let decision = router.route(&request).await.expect("should succeed");
        assert!(decision.timestamp >= before);
    }

    // ── Budget eliminates all models ─────────────────────────────────

    #[tokio::test]
    async fn budget_too_low_returns_no_capable() {
        let router = DefaultModelRouter::new(test_catalog());
        let request = RouteRequest {
            required_capabilities: vec![tools_req()],
            preferred_model: None,
            preferred_provider: None,
            max_cost_input_per_m: Some(0.001), // impossibly low
            policy: RoutingPolicy::CostOptimized,
        };
        let err = router.route(&request).await.unwrap_err();
        assert!(matches!(err, RouteError::NoCapableModel { .. }));
    }

    // ── Alternatives are populated ───────────────────────────────────

    #[tokio::test]
    async fn decision_has_alternatives() {
        let router = DefaultModelRouter::new(test_catalog());
        let request = RouteRequest {
            required_capabilities: vec![tools_req()],
            preferred_model: None,
            preferred_provider: None,
            max_cost_input_per_m: None,
            policy: RoutingPolicy::CostOptimized,
        };
        let decision = router.route(&request).await.expect("should succeed");
        // Many models support tools, so there should be alternatives
        assert!(
            !decision.alternatives.is_empty(),
            "should have alternatives"
        );
    }

    // ── ProviderHealthInfo default ───────────────────────────────────

    #[test]
    fn provider_health_info_default() {
        let info = ProviderHealthInfo::default();
        assert_eq!(info.state, HealthState::Unknown);
        assert_eq!(info.latency_p50_ms, 0);
    }

    // ── RouteReason serde roundtrip ──────────────────────────────────

    #[test]
    fn route_reason_serde_round_trip() {
        let reasons = vec![
            RouteReason::ExactMatch,
            RouteReason::LowestCost {
                cost_input_per_m: 3.0,
            },
            RouteReason::LowestLatency,
            RouteReason::MostCapabilities {
                capability_count: 5,
            },
            RouteReason::PreferredAndHealthy,
            RouteReason::OnlyOption,
        ];
        for reason in &reasons {
            let json = serde_json::to_string(reason).expect("serialize");
            let back: RouteReason = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(format!("{back}"), format!("{reason}"));
        }
    }

    // ── Multiple capability filter ──────────────────────────────────

    #[tokio::test]
    async fn multiple_capability_filter() {
        let router = DefaultModelRouter::new(test_catalog());
        let request = RouteRequest {
            required_capabilities: vec![
                tools_req(),
                vision_req(),
                CapabilityRequirement {
                    capability: Capability::Thinking,
                    required_by: "test".into(),
                    hard: true,
                },
            ],
            preferred_model: None,
            preferred_provider: None,
            max_cost_input_per_m: None,
            policy: RoutingPolicy::CostOptimized,
        };
        let decision = router.route(&request).await.expect("should succeed");
        // Model should support all three capabilities
        let catalog = BuiltInModelCatalog::new();
        let desc = catalog.get(&decision.selected.model).expect("exists");
        assert!(desc.supports_tools);
        assert!(desc.supports_vision);
        assert!(desc.supports_thinking);
    }

    // ── All unhealthy falls back to all candidates ───────────────────

    #[tokio::test]
    async fn all_unhealthy_still_routes() {
        let router = DefaultModelRouter::new(test_catalog());
        router.set_health(
            "anthropic",
            ProviderHealthInfo {
                state: HealthState::Unhealthy,
                latency_p50_ms: 0,
            },
        );
        router.set_health(
            "openai",
            ProviderHealthInfo {
                state: HealthState::Unhealthy,
                latency_p50_ms: 0,
            },
        );
        router.set_health(
            "gemini",
            ProviderHealthInfo {
                state: HealthState::Unhealthy,
                latency_p50_ms: 0,
            },
        );
        router.set_health(
            "perplexity",
            ProviderHealthInfo {
                state: HealthState::Unhealthy,
                latency_p50_ms: 0,
            },
        );
        let request = RouteRequest {
            required_capabilities: vec![tools_req()],
            preferred_model: None,
            preferred_provider: None,
            max_cost_input_per_m: None,
            policy: RoutingPolicy::CostOptimized,
        };
        // Should still succeed by falling back to all candidates
        let decision = router.route(&request).await.expect("should fall back");
        assert!(!decision.selected.model.is_empty());
    }

    // ── Health update overwrites previous ────────────────────────────

    #[test]
    fn health_update_overwrites() {
        let router = DefaultModelRouter::new(test_catalog());
        router.set_health(
            "prov",
            ProviderHealthInfo {
                state: HealthState::Healthy,
                latency_p50_ms: 10,
            },
        );
        assert_eq!(router.get_health("prov").state, HealthState::Healthy);

        router.set_health(
            "prov",
            ProviderHealthInfo {
                state: HealthState::Unhealthy,
                latency_p50_ms: 9999,
            },
        );
        assert_eq!(router.get_health("prov").state, HealthState::Unhealthy);
        assert_eq!(router.get_health("prov").latency_p50_ms, 9999);
    }
}
