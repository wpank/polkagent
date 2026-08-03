//! Capability negotiation system (PRD-04 §8).
//!
//! This module resolves whether a given model + provider combination can
//! satisfy the capability requirements collected from an agent specification,
//! its skills, and its tools. The algorithm:
//!
//! 1. Collect all requirements from the agent spec / skills / tools.
//! 2. Query the model catalog for the candidate model's descriptor.
//! 3. Check provider restrictions (available models).
//! 4. Intersect with grant restrictions (if supplied).
//! 5. Report satisfied capabilities, missing capabilities, and warnings.
//!
//! Results are cached per `(provider, model)` pair via [`ProbeCache`].

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use polkagent_config::model_registry::{ModelCatalog, ModelDescriptor};

// ---------------------------------------------------------------------------
// Capability
// ---------------------------------------------------------------------------

/// A single model/provider capability that can be required or offered.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    /// Structured tool calling.
    Tools,
    /// Extended thinking / chain-of-thought mode.
    Thinking,
    /// Image/vision input.
    Vision,
    /// Streaming responses.
    Streaming,
    /// Prompt caching.
    Caching,
    /// JSON schema constrained output.
    StructuredOutput,
    /// Built-in web search.
    WebSearch,
    /// Minimum context window size (in tokens).
    MinContextWindow(u64),
    /// Minimum max output tokens.
    MinMaxOutput(u64),
}

impl fmt::Display for Capability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Tools => write!(f, "tools"),
            Self::Thinking => write!(f, "thinking"),
            Self::Vision => write!(f, "vision"),
            Self::Streaming => write!(f, "streaming"),
            Self::Caching => write!(f, "caching"),
            Self::StructuredOutput => write!(f, "structured_output"),
            Self::WebSearch => write!(f, "web_search"),
            Self::MinContextWindow(n) => write!(f, "min_context_window({n})"),
            Self::MinMaxOutput(n) => write!(f, "min_max_output({n})"),
        }
    }
}

// ---------------------------------------------------------------------------
// CapabilityRequirement
// ---------------------------------------------------------------------------

/// A single capability requirement with its source and whether it is hard
/// (blocking) or soft (warning-only).
#[derive(Debug, Clone)]
pub struct CapabilityRequirement {
    /// The capability being required.
    pub capability: Capability,
    /// Human-readable label of the source that requires this capability
    /// (e.g. `"skill:chain-query"`, `"tool:polkagent.chain.decode"`).
    pub required_by: String,
    /// If `true`, missing this capability is a hard failure.
    /// If `false`, a warning is emitted but negotiation succeeds.
    pub hard: bool,
}

// ---------------------------------------------------------------------------
// MissingCapability
// ---------------------------------------------------------------------------

/// Describes a capability that the model/provider combination does not satisfy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MissingCapability {
    /// The capability that is missing.
    pub capability: Capability,
    /// What requires this capability.
    pub required_by: String,
    /// Whether this is a hard requirement (blocks execution) or soft (warning).
    pub hard: bool,
}

impl fmt::Display for MissingCapability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let severity = if self.hard { "HARD" } else { "SOFT" };
        write!(
            f,
            "[{severity}] {cap} required by {src}",
            cap = self.capability,
            src = self.required_by,
        )
    }
}

// ---------------------------------------------------------------------------
// NegotiatedCapabilities
// ---------------------------------------------------------------------------

/// The result of capability negotiation for a `(model, provider)` pair.
#[derive(Debug, Clone)]
pub struct NegotiatedCapabilities {
    /// The model slug that was evaluated.
    pub model: String,
    /// The provider that serves this model.
    pub provider: String,
    /// Capabilities the model actually offers.
    pub available: Vec<Capability>,
    /// Required capabilities the model does not offer.
    pub missing: Vec<MissingCapability>,
    /// Whether all hard requirements are met.
    pub satisfied: bool,
    /// Warning messages for soft mismatches or other advisory information.
    pub warnings: Vec<String>,
}

impl NegotiatedCapabilities {
    /// Return only hard missing capabilities.
    pub fn hard_missing(&self) -> Vec<&MissingCapability> {
        self.missing.iter().filter(|m| m.hard).collect()
    }

    /// Return only soft missing capabilities (warnings).
    pub fn soft_missing(&self) -> Vec<&MissingCapability> {
        self.missing.iter().filter(|m| !m.hard).collect()
    }
}

impl fmt::Display for NegotiatedCapabilities {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "model={model} provider={provider} satisfied={sat} available={avail} missing={miss} warnings={warn}",
            model = self.model,
            provider = self.provider,
            sat = self.satisfied,
            avail = self.available.len(),
            miss = self.missing.len(),
            warn = self.warnings.len(),
        )
    }
}

// ---------------------------------------------------------------------------
// ProviderRestrictions
// ---------------------------------------------------------------------------

/// Optional restrictions imposed by a provider or grant on what capabilities
/// are allowed.
#[derive(Debug, Clone, Default)]
pub struct ProviderRestrictions {
    /// If non-empty, only models in this list are allowed.
    pub allowed_models: Vec<String>,
    /// Capabilities explicitly denied by the provider/grant.
    pub denied_capabilities: Vec<Capability>,
}

// ---------------------------------------------------------------------------
// Negotiation algorithm
// ---------------------------------------------------------------------------

/// Extract the set of capabilities a [`ModelDescriptor`] offers.
pub fn capabilities_from_descriptor(desc: &ModelDescriptor) -> Vec<Capability> {
    let mut caps = Vec::new();
    if desc.supports_tools {
        caps.push(Capability::Tools);
    }
    if desc.supports_thinking {
        caps.push(Capability::Thinking);
    }
    if desc.supports_vision {
        caps.push(Capability::Vision);
    }
    if desc.supports_streaming {
        caps.push(Capability::Streaming);
    }
    if desc.supports_caching {
        caps.push(Capability::Caching);
    }
    if desc.supports_structured_output {
        caps.push(Capability::StructuredOutput);
    }
    if desc.supports_web_search {
        caps.push(Capability::WebSearch);
    }
    caps
}

/// Check whether a model descriptor satisfies a single capability requirement.
fn descriptor_has_capability(desc: &ModelDescriptor, cap: &Capability) -> bool {
    match cap {
        Capability::Tools => desc.supports_tools,
        Capability::Thinking => desc.supports_thinking,
        Capability::Vision => desc.supports_vision,
        Capability::Streaming => desc.supports_streaming,
        Capability::Caching => desc.supports_caching,
        Capability::StructuredOutput => desc.supports_structured_output,
        Capability::WebSearch => desc.supports_web_search,
        Capability::MinContextWindow(min) => desc.context_window >= *min,
        Capability::MinMaxOutput(min) => {
            desc.max_output.map_or(true, |max| max >= *min)
        }
    }
}

/// Run the full negotiation algorithm.
///
/// # Arguments
///
/// * `catalog` — the model catalog to look up the model descriptor.
/// * `model_slug` — the model identifier to evaluate.
/// * `requirements` — collected capability requirements from agent/skills/tools.
/// * `restrictions` — optional provider or grant restrictions.
///
/// # Returns
///
/// A [`NegotiatedCapabilities`] describing what is available, what is missing,
/// and whether all hard requirements are satisfied.
pub fn negotiate(
    catalog: &dyn ModelCatalog,
    model_slug: &str,
    requirements: &[CapabilityRequirement],
    restrictions: Option<&ProviderRestrictions>,
) -> NegotiatedCapabilities {
    // Step 1: Look up the model in the catalog.
    let desc = match catalog.get(model_slug) {
        Some(d) => d,
        None => {
            // Model not found — all hard requirements are missing.
            let missing: Vec<MissingCapability> = requirements
                .iter()
                .map(|r| MissingCapability {
                    capability: r.capability.clone(),
                    required_by: r.required_by.clone(),
                    hard: r.hard,
                })
                .collect();
            let has_hard = missing.iter().any(|m| m.hard);
            return NegotiatedCapabilities {
                model: model_slug.to_string(),
                provider: String::new(),
                available: vec![],
                missing,
                satisfied: !has_hard,
                warnings: vec![format!("model '{model_slug}' not found in catalog")],
            };
        }
    };

    let mut warnings = Vec::new();

    // Step 2: Check provider restrictions.
    if let Some(restrictions) = restrictions {
        if !restrictions.allowed_models.is_empty()
            && !restrictions.allowed_models.iter().any(|m| m == model_slug)
        {
            let missing: Vec<MissingCapability> = requirements
                .iter()
                .filter(|r| r.hard)
                .map(|r| MissingCapability {
                    capability: r.capability.clone(),
                    required_by: r.required_by.clone(),
                    hard: true,
                })
                .collect();
            return NegotiatedCapabilities {
                model: model_slug.to_string(),
                provider: desc.provider.clone(),
                available: capabilities_from_descriptor(desc),
                missing,
                satisfied: false,
                warnings: vec![format!(
                    "model '{model_slug}' is not in the allowed models list"
                )],
            };
        }
    }

    // Step 3: Check each requirement against the model descriptor.
    let available = capabilities_from_descriptor(desc);
    let mut missing = Vec::new();

    for req in requirements {
        // Check if the capability is denied by provider restrictions.
        let denied = restrictions
            .map_or(false, |r| r.denied_capabilities.contains(&req.capability));

        if denied || !descriptor_has_capability(desc, &req.capability) {
            let m = MissingCapability {
                capability: req.capability.clone(),
                required_by: req.required_by.clone(),
                hard: req.hard,
            };
            if !req.hard {
                warnings.push(format!(
                    "soft requirement '{}' from '{}' not satisfied",
                    req.capability, req.required_by
                ));
            }
            missing.push(m);
        }
    }

    let satisfied = !missing.iter().any(|m| m.hard);

    NegotiatedCapabilities {
        model: model_slug.to_string(),
        provider: desc.provider.clone(),
        available,
        missing,
        satisfied,
        warnings,
    }
}

// ---------------------------------------------------------------------------
// ProbeCache
// ---------------------------------------------------------------------------

/// Cached negotiation result for a `(provider, model)` pair.
#[derive(Debug, Clone)]
struct CachedProbe {
    result: NegotiatedCapabilities,
    cached_at: Instant,
}

/// A cache of capability probe results, keyed by `(provider_id, model_slug)`.
///
/// Results expire after a configurable TTL.
#[derive(Debug, Clone)]
pub struct ProbeCache {
    entries: Arc<RwLock<HashMap<(String, String), CachedProbe>>>,
    ttl: Duration,
}

impl ProbeCache {
    /// Create a new cache with the given TTL.
    pub fn new(ttl: Duration) -> Self {
        Self {
            entries: Arc::new(RwLock::new(HashMap::new())),
            ttl,
        }
    }

    /// Look up a cached result. Returns `None` if not present or expired.
    pub fn get(&self, provider_id: &str, model_slug: &str) -> Option<NegotiatedCapabilities> {
        let entries = self.entries.read();
        let key = (provider_id.to_string(), model_slug.to_string());
        entries.get(&key).and_then(|cached| {
            if cached.cached_at.elapsed() < self.ttl {
                Some(cached.result.clone())
            } else {
                None
            }
        })
    }

    /// Insert or update a cached result.
    pub fn put(
        &self,
        provider_id: &str,
        model_slug: &str,
        result: NegotiatedCapabilities,
    ) {
        let key = (provider_id.to_string(), model_slug.to_string());
        let mut entries = self.entries.write();
        entries.insert(
            key,
            CachedProbe {
                result,
                cached_at: Instant::now(),
            },
        );
    }

    /// Remove expired entries from the cache.
    pub fn evict_expired(&self) {
        let mut entries = self.entries.write();
        entries.retain(|_, v| v.cached_at.elapsed() < self.ttl);
    }

    /// Return the number of entries currently in the cache (including expired).
    pub fn len(&self) -> usize {
        self.entries.read().len()
    }

    /// Return `true` if the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.read().is_empty()
    }

    /// Clear all entries.
    pub fn clear(&self) {
        self.entries.write().clear();
    }
}

impl Default for ProbeCache {
    fn default() -> Self {
        Self::new(Duration::from_secs(300))
    }
}

// ---------------------------------------------------------------------------
// Negotiate with cache
// ---------------------------------------------------------------------------

/// Run negotiation with caching. If a cached result exists for the
/// `(provider_id, model_slug)` pair, it is returned immediately.
/// Otherwise, negotiation is performed and the result is cached.
pub fn negotiate_cached(
    cache: &ProbeCache,
    catalog: &dyn ModelCatalog,
    provider_id: &str,
    model_slug: &str,
    requirements: &[CapabilityRequirement],
    restrictions: Option<&ProviderRestrictions>,
) -> NegotiatedCapabilities {
    if let Some(cached) = cache.get(provider_id, model_slug) {
        return cached;
    }
    let result = negotiate(catalog, model_slug, requirements, restrictions);
    cache.put(provider_id, model_slug, result.clone());
    result
}

// ---------------------------------------------------------------------------
// find_best_model helper
// ---------------------------------------------------------------------------

/// Search the catalog for the best model that satisfies all hard requirements.
///
/// Returns the first model (by catalog order) whose negotiation result has
/// `satisfied == true`. If no model satisfies all hard requirements, returns
/// `None`.
pub fn find_best_model(
    catalog: &dyn ModelCatalog,
    requirements: &[CapabilityRequirement],
    restrictions: Option<&ProviderRestrictions>,
) -> Option<NegotiatedCapabilities> {
    let models = catalog.list();
    models
        .into_iter()
        .map(|desc| negotiate(catalog, &desc.slug, requirements, restrictions))
        .find(|result| result.satisfied)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use polkagent_config::model_registry::BuiltInModelCatalog;

    fn catalog() -> BuiltInModelCatalog {
        BuiltInModelCatalog::new()
    }

    // ── capabilities_from_descriptor ──────────────────────────────────

    #[test]
    fn capabilities_from_opus() {
        let cat = catalog();
        let desc = cat.get("claude-opus-4-6").expect("exists");
        let caps = capabilities_from_descriptor(desc);
        assert!(caps.contains(&Capability::Tools));
        assert!(caps.contains(&Capability::Thinking));
        assert!(caps.contains(&Capability::Vision));
        assert!(caps.contains(&Capability::Streaming));
        assert!(caps.contains(&Capability::Caching));
        assert!(!caps.contains(&Capability::StructuredOutput));
        assert!(!caps.contains(&Capability::WebSearch));
    }

    #[test]
    fn capabilities_from_sonar() {
        let cat = catalog();
        let desc = cat.get("sonar").expect("exists");
        let caps = capabilities_from_descriptor(desc);
        assert!(!caps.contains(&Capability::Tools));
        assert!(caps.contains(&Capability::WebSearch));
    }

    #[test]
    fn capabilities_from_gemini_pro() {
        let cat = catalog();
        let desc = cat.get("gemini-2.5-pro").expect("exists");
        let caps = capabilities_from_descriptor(desc);
        assert!(caps.contains(&Capability::Tools));
        assert!(caps.contains(&Capability::StructuredOutput));
        assert!(caps.contains(&Capability::WebSearch));
        assert!(caps.contains(&Capability::Caching));
    }

    // ── negotiate: basic satisfaction ──────────────────────────────────

    #[test]
    fn negotiate_all_satisfied() {
        let cat = catalog();
        let reqs = vec![
            CapabilityRequirement {
                capability: Capability::Tools,
                required_by: "agent-spec".into(),
                hard: true,
            },
            CapabilityRequirement {
                capability: Capability::Vision,
                required_by: "skill:image-analyze".into(),
                hard: true,
            },
        ];
        let result = negotiate(&cat, "claude-opus-4-6", &reqs, None);
        assert!(result.satisfied);
        assert!(result.missing.is_empty());
        assert_eq!(result.model, "claude-opus-4-6");
        assert_eq!(result.provider, "anthropic");
    }

    #[test]
    fn negotiate_hard_missing() {
        let cat = catalog();
        let reqs = vec![CapabilityRequirement {
            capability: Capability::Tools,
            required_by: "agent-spec".into(),
            hard: true,
        }];
        let result = negotiate(&cat, "sonar", &reqs, None);
        assert!(!result.satisfied);
        assert_eq!(result.missing.len(), 1);
        assert!(result.missing[0].hard);
        assert_eq!(result.missing[0].capability, Capability::Tools);
    }

    #[test]
    fn negotiate_soft_missing_still_satisfied() {
        let cat = catalog();
        let reqs = vec![CapabilityRequirement {
            capability: Capability::WebSearch,
            required_by: "optional-feature".into(),
            hard: false,
        }];
        let result = negotiate(&cat, "claude-opus-4-6", &reqs, None);
        assert!(result.satisfied);
        assert_eq!(result.missing.len(), 1);
        assert!(!result.missing[0].hard);
        assert_eq!(result.warnings.len(), 1);
    }

    #[test]
    fn negotiate_model_not_found() {
        let cat = catalog();
        let reqs = vec![CapabilityRequirement {
            capability: Capability::Tools,
            required_by: "agent-spec".into(),
            hard: true,
        }];
        let result = negotiate(&cat, "nonexistent-model", &reqs, None);
        assert!(!result.satisfied);
        assert_eq!(result.missing.len(), 1);
        assert!(result.warnings[0].contains("not found"));
    }

    #[test]
    fn negotiate_model_not_found_no_hard_reqs() {
        let cat = catalog();
        let reqs = vec![CapabilityRequirement {
            capability: Capability::Tools,
            required_by: "optional".into(),
            hard: false,
        }];
        let result = negotiate(&cat, "nonexistent-model", &reqs, None);
        // No hard requirements, so still satisfied despite model missing.
        assert!(result.satisfied);
    }

    #[test]
    fn negotiate_empty_requirements() {
        let cat = catalog();
        let result = negotiate(&cat, "claude-opus-4-6", &[], None);
        assert!(result.satisfied);
        assert!(result.missing.is_empty());
        assert!(!result.available.is_empty());
    }

    // ── negotiate: min context window ─────────────────────────────────

    #[test]
    fn negotiate_min_context_window_satisfied() {
        let cat = catalog();
        let reqs = vec![CapabilityRequirement {
            capability: Capability::MinContextWindow(100_000),
            required_by: "long-context-task".into(),
            hard: true,
        }];
        let result = negotiate(&cat, "claude-opus-4-6", &reqs, None);
        assert!(result.satisfied);
    }

    #[test]
    fn negotiate_min_context_window_not_satisfied() {
        let cat = catalog();
        let reqs = vec![CapabilityRequirement {
            capability: Capability::MinContextWindow(500_000),
            required_by: "huge-context-task".into(),
            hard: true,
        }];
        // claude-opus has 200k context
        let result = negotiate(&cat, "claude-opus-4-6", &reqs, None);
        assert!(!result.satisfied);
        assert_eq!(result.missing.len(), 1);
    }

    #[test]
    fn negotiate_min_context_gemini_million() {
        let cat = catalog();
        let reqs = vec![CapabilityRequirement {
            capability: Capability::MinContextWindow(500_000),
            required_by: "huge-context-task".into(),
            hard: true,
        }];
        // Gemini has 1M context
        let result = negotiate(&cat, "gemini-2.5-pro", &reqs, None);
        assert!(result.satisfied);
    }

    // ── negotiate: min max output ─────────────────────────────────────

    #[test]
    fn negotiate_min_max_output_satisfied() {
        let cat = catalog();
        let reqs = vec![CapabilityRequirement {
            capability: Capability::MinMaxOutput(16_000),
            required_by: "long-output".into(),
            hard: true,
        }];
        let result = negotiate(&cat, "claude-opus-4-6", &reqs, None);
        assert!(result.satisfied); // opus has 32k max output
    }

    #[test]
    fn negotiate_min_max_output_not_satisfied() {
        let cat = catalog();
        let reqs = vec![CapabilityRequirement {
            capability: Capability::MinMaxOutput(50_000),
            required_by: "very-long-output".into(),
            hard: true,
        }];
        let result = negotiate(&cat, "claude-opus-4-6", &reqs, None);
        assert!(!result.satisfied); // opus max output is 32k
    }

    // ── negotiate: provider restrictions ──────────────────────────────

    #[test]
    fn negotiate_provider_model_not_allowed() {
        let cat = catalog();
        let reqs = vec![CapabilityRequirement {
            capability: Capability::Tools,
            required_by: "agent-spec".into(),
            hard: true,
        }];
        let restrictions = ProviderRestrictions {
            allowed_models: vec!["gpt-4o".into()],
            denied_capabilities: vec![],
        };
        let result = negotiate(&cat, "claude-opus-4-6", &reqs, Some(&restrictions));
        assert!(!result.satisfied);
        assert!(result.warnings[0].contains("not in the allowed models list"));
    }

    #[test]
    fn negotiate_provider_model_allowed() {
        let cat = catalog();
        let reqs = vec![CapabilityRequirement {
            capability: Capability::Tools,
            required_by: "agent-spec".into(),
            hard: true,
        }];
        let restrictions = ProviderRestrictions {
            allowed_models: vec!["claude-opus-4-6".into()],
            denied_capabilities: vec![],
        };
        let result = negotiate(&cat, "claude-opus-4-6", &reqs, Some(&restrictions));
        assert!(result.satisfied);
    }

    #[test]
    fn negotiate_provider_denies_capability() {
        let cat = catalog();
        let reqs = vec![CapabilityRequirement {
            capability: Capability::Tools,
            required_by: "agent-spec".into(),
            hard: true,
        }];
        let restrictions = ProviderRestrictions {
            allowed_models: vec![],
            denied_capabilities: vec![Capability::Tools],
        };
        let result = negotiate(&cat, "claude-opus-4-6", &reqs, Some(&restrictions));
        assert!(!result.satisfied);
        assert_eq!(result.missing.len(), 1);
    }

    #[test]
    fn negotiate_provider_empty_allowed_means_all() {
        let cat = catalog();
        let reqs = vec![CapabilityRequirement {
            capability: Capability::Tools,
            required_by: "agent-spec".into(),
            hard: true,
        }];
        let restrictions = ProviderRestrictions {
            allowed_models: vec![],
            denied_capabilities: vec![],
        };
        let result = negotiate(&cat, "claude-opus-4-6", &reqs, Some(&restrictions));
        assert!(result.satisfied);
    }

    // ── negotiate: mixed hard and soft ────────────────────────────────

    #[test]
    fn negotiate_mixed_hard_soft() {
        let cat = catalog();
        let reqs = vec![
            CapabilityRequirement {
                capability: Capability::Tools,
                required_by: "agent-spec".into(),
                hard: true,
            },
            CapabilityRequirement {
                capability: Capability::WebSearch,
                required_by: "nice-to-have".into(),
                hard: false,
            },
        ];
        let result = negotiate(&cat, "claude-opus-4-6", &reqs, None);
        assert!(result.satisfied); // tools satisfied, web search soft-missing
        assert_eq!(result.missing.len(), 1);
        assert!(!result.missing[0].hard);
    }

    #[test]
    fn negotiate_both_hard_missing() {
        let cat = catalog();
        let reqs = vec![
            CapabilityRequirement {
                capability: Capability::Tools,
                required_by: "agent-spec".into(),
                hard: true,
            },
            CapabilityRequirement {
                capability: Capability::StructuredOutput,
                required_by: "json-mode".into(),
                hard: true,
            },
        ];
        // opus: tools yes, structured_output no
        let result = negotiate(&cat, "claude-opus-4-6", &reqs, None);
        assert!(!result.satisfied);
        assert_eq!(result.hard_missing().len(), 1);
    }

    // ── NegotiatedCapabilities helpers ────────────────────────────────

    #[test]
    fn hard_missing_and_soft_missing_partition() {
        let cat = catalog();
        let reqs = vec![
            CapabilityRequirement {
                capability: Capability::WebSearch,
                required_by: "soft".into(),
                hard: false,
            },
            CapabilityRequirement {
                capability: Capability::StructuredOutput,
                required_by: "hard".into(),
                hard: true,
            },
        ];
        let result = negotiate(&cat, "claude-opus-4-6", &reqs, None);
        assert_eq!(result.hard_missing().len(), 1);
        assert_eq!(result.soft_missing().len(), 1);
        assert_eq!(result.hard_missing()[0].required_by, "hard");
        assert_eq!(result.soft_missing()[0].required_by, "soft");
    }

    #[test]
    fn negotiated_capabilities_display() {
        let result = NegotiatedCapabilities {
            model: "test-model".into(),
            provider: "test-provider".into(),
            available: vec![Capability::Tools],
            missing: vec![],
            satisfied: true,
            warnings: vec![],
        };
        let display = format!("{result}");
        assert!(display.contains("test-model"));
        assert!(display.contains("satisfied=true"));
    }

    // ── Capability Display ────────────────────────────────────────────

    #[test]
    fn capability_display() {
        assert_eq!(format!("{}", Capability::Tools), "tools");
        assert_eq!(format!("{}", Capability::Thinking), "thinking");
        assert_eq!(format!("{}", Capability::Vision), "vision");
        assert_eq!(format!("{}", Capability::Streaming), "streaming");
        assert_eq!(format!("{}", Capability::Caching), "caching");
        assert_eq!(format!("{}", Capability::StructuredOutput), "structured_output");
        assert_eq!(format!("{}", Capability::WebSearch), "web_search");
        assert_eq!(
            format!("{}", Capability::MinContextWindow(100_000)),
            "min_context_window(100000)"
        );
        assert_eq!(
            format!("{}", Capability::MinMaxOutput(8192)),
            "min_max_output(8192)"
        );
    }

    #[test]
    fn missing_capability_display() {
        let m = MissingCapability {
            capability: Capability::Tools,
            required_by: "my-agent".into(),
            hard: true,
        };
        let s = format!("{m}");
        assert!(s.contains("[HARD]"));
        assert!(s.contains("tools"));
        assert!(s.contains("my-agent"));

        let m2 = MissingCapability {
            capability: Capability::Vision,
            required_by: "optional".into(),
            hard: false,
        };
        let s2 = format!("{m2}");
        assert!(s2.contains("[SOFT]"));
    }

    // ── Capability serde round-trip ───────────────────────────────────

    #[test]
    fn capability_serde_round_trip() {
        let caps = vec![
            Capability::Tools,
            Capability::Thinking,
            Capability::Vision,
            Capability::Streaming,
            Capability::Caching,
            Capability::StructuredOutput,
            Capability::WebSearch,
            Capability::MinContextWindow(200_000),
            Capability::MinMaxOutput(32_000),
        ];
        for cap in &caps {
            let json = serde_json::to_string(cap).expect("serialize");
            let back: Capability = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(*cap, back, "round-trip failed for {cap}");
        }
    }

    // ── ProbeCache ────────────────────────────────────────────────────

    #[test]
    fn probe_cache_put_and_get() {
        let cache = ProbeCache::new(Duration::from_secs(60));
        let result = NegotiatedCapabilities {
            model: "test".into(),
            provider: "prov".into(),
            available: vec![Capability::Tools],
            missing: vec![],
            satisfied: true,
            warnings: vec![],
        };
        cache.put("prov", "test", result.clone());
        let cached = cache.get("prov", "test");
        assert!(cached.is_some());
        assert_eq!(cached.as_ref().expect("cached").model, "test");
    }

    #[test]
    fn probe_cache_miss() {
        let cache = ProbeCache::new(Duration::from_secs(60));
        assert!(cache.get("prov", "nonexistent").is_none());
    }

    #[test]
    fn probe_cache_expired() {
        let cache = ProbeCache::new(Duration::from_millis(1));
        let result = NegotiatedCapabilities {
            model: "test".into(),
            provider: "prov".into(),
            available: vec![],
            missing: vec![],
            satisfied: true,
            warnings: vec![],
        };
        cache.put("prov", "test", result);
        std::thread::sleep(Duration::from_millis(10));
        assert!(cache.get("prov", "test").is_none());
    }

    #[test]
    fn probe_cache_evict_expired() {
        let cache = ProbeCache::new(Duration::from_millis(1));
        let result = NegotiatedCapabilities {
            model: "test".into(),
            provider: "prov".into(),
            available: vec![],
            missing: vec![],
            satisfied: true,
            warnings: vec![],
        };
        cache.put("prov", "test", result);
        assert_eq!(cache.len(), 1);
        std::thread::sleep(Duration::from_millis(10));
        cache.evict_expired();
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn probe_cache_clear() {
        let cache = ProbeCache::new(Duration::from_secs(60));
        let result = NegotiatedCapabilities {
            model: "a".into(),
            provider: "p".into(),
            available: vec![],
            missing: vec![],
            satisfied: true,
            warnings: vec![],
        };
        cache.put("p", "a", result.clone());
        cache.put("p", "b", result);
        assert_eq!(cache.len(), 2);
        cache.clear();
        assert!(cache.is_empty());
    }

    #[test]
    fn probe_cache_default_ttl() {
        let cache = ProbeCache::default();
        assert_eq!(cache.ttl, Duration::from_secs(300));
    }

    // ── negotiate_cached ──────────────────────────────────────────────

    #[test]
    fn negotiate_cached_populates_cache() {
        let cat = catalog();
        let cache = ProbeCache::new(Duration::from_secs(60));
        let reqs = vec![CapabilityRequirement {
            capability: Capability::Tools,
            required_by: "test".into(),
            hard: true,
        }];
        assert!(cache.is_empty());
        let result = negotiate_cached(&cache, &cat, "anthropic", "claude-opus-4-6", &reqs, None);
        assert!(result.satisfied);
        assert_eq!(cache.len(), 1);
        // Second call should hit cache.
        let result2 = negotiate_cached(&cache, &cat, "anthropic", "claude-opus-4-6", &reqs, None);
        assert!(result2.satisfied);
        assert_eq!(cache.len(), 1);
    }

    // ── find_best_model ───────────────────────────────────────────────

    #[test]
    fn find_best_model_with_web_search() {
        let cat = catalog();
        let reqs = vec![CapabilityRequirement {
            capability: Capability::WebSearch,
            required_by: "search-task".into(),
            hard: true,
        }];
        let result = find_best_model(&cat, &reqs, None);
        assert!(result.is_some());
        let neg = result.expect("should find a model");
        assert!(neg.satisfied);
        // Should be one of the models that supports web search
        let web_models = ["gemini-2.5-pro", "gemini-2.5-flash", "sonar-pro", "sonar"];
        assert!(
            web_models.contains(&neg.model.as_str()),
            "unexpected model: {}",
            neg.model
        );
    }

    #[test]
    fn find_best_model_impossible_reqs() {
        let cat = catalog();
        let reqs = vec![CapabilityRequirement {
            capability: Capability::MinContextWindow(10_000_000),
            required_by: "impossible".into(),
            hard: true,
        }];
        let result = find_best_model(&cat, &reqs, None);
        assert!(result.is_none());
    }

    #[test]
    fn find_best_model_tools_and_structured_output() {
        let cat = catalog();
        let reqs = vec![
            CapabilityRequirement {
                capability: Capability::Tools,
                required_by: "spec".into(),
                hard: true,
            },
            CapabilityRequirement {
                capability: Capability::StructuredOutput,
                required_by: "json-mode".into(),
                hard: true,
            },
        ];
        let result = find_best_model(&cat, &reqs, None);
        assert!(result.is_some());
        let neg = result.expect("found");
        assert!(neg.satisfied);
    }

    #[test]
    fn find_best_model_no_requirements() {
        let cat = catalog();
        let result = find_best_model(&cat, &[], None);
        // Any model will satisfy no requirements
        assert!(result.is_some());
    }
}
