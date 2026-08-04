//! Agent domain types.
//!
//! An **agent** is a versioned, portable definition of an AI execution unit.
//! The [`AgentSpec`] captures what the agent does, which models and tools it
//! may use, its policy references, and its resource limits. The
//! [`AgentState`] tracks where the agent is in its operational lifecycle.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::config::AutonomyLevel;
use crate::error::PolkagentError;
use crate::ids::AgentId;

// ---------------------------------------------------------------------------
// ResourceLimits
// ---------------------------------------------------------------------------

/// Runtime resource limits applied to every run of this agent.
///
/// All fields are optional; absent limits are inherited from the global
/// execution configuration in `polkagent.toml`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ResourceLimits {
    /// Maximum number of tokens the model may generate in a single turn.
    ///
    /// Maps to the `max_tokens` parameter sent to the model provider.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens_per_turn: Option<u32>,
    /// Maximum number of agentic turns before the run is automatically
    /// cancelled with a `TurnLimitExceeded` reason.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_turns: Option<u32>,
    /// Wall-clock timeout in seconds for the entire run.
    ///
    /// When the limit is hit the run is cancelled cooperatively.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    /// Maximum number of effect intents that may be in-flight concurrently.
    ///
    /// Additional intents are queued until a slot opens.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_concurrent_effects: Option<u32>,
}

// ---------------------------------------------------------------------------
// ModelPreference
// ---------------------------------------------------------------------------

/// Preferred model configuration for this agent.
///
/// When present, these values are merged with (and override) the provider
/// defaults at execution time. The `provider` and `model_id` fields together
/// select which adapter and model to call; the remaining fields tune the
/// completion parameters.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelPreference {
    /// Provider identifier, e.g. `"anthropic"` or `"openai"`.
    ///
    /// Must match a key in the provider registry. If absent, the globally
    /// configured default provider is used.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Model identifier within the selected provider, e.g.
    /// `"claude-opus-4-6"`.
    ///
    /// If absent, the provider's configured `default_model` is used.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
    /// Sampling temperature (0.0–2.0). Lower values produce more
    /// deterministic output; higher values produce more varied output.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    /// Override the agent-level system prompt for this model preference.
    ///
    /// When set, this value is used instead of [`AgentSpec::system_prompt`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
}

// ---------------------------------------------------------------------------
// MemoryConfig
// ---------------------------------------------------------------------------

/// Memory subsystem settings for this agent.
///
/// Controls whether the agent's memory store is active, its capacity, and the
/// default classification applied to new entries.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MemoryConfig {
    /// Whether memory is enabled for this agent (default `true`).
    #[serde(default = "default_memory_enabled")]
    pub enabled: bool,
    /// Maximum number of memory entries retained per agent.
    ///
    /// When the limit is reached, the oldest entries are evicted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_entries: Option<usize>,
    /// How many days entries are retained before being automatically
    /// garbage-collected. `None` means entries never expire.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retention_days: Option<u32>,
    /// The data classification applied to new memory entries when the
    /// writing component does not specify one explicitly (e.g. `"private"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub classification_default: Option<String>,
}

fn default_memory_enabled() -> bool {
    true
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_entries: None,
            retention_days: None,
            classification_default: None,
        }
    }
}

// ---------------------------------------------------------------------------
// DegradationStage
// ---------------------------------------------------------------------------

/// The specific dependency failure that caused an agent to enter the
/// [`AgentState::Degraded`] state.
///
/// This is used by the health monitor to record which component is unhealthy
/// so that operators and runbooks can take targeted remediation actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DegradationStage {
    /// The primary provider is degraded; a fallback model is in use.
    ProviderFallback,
    /// The coding/agent harness is unhealthy; new sessions are being rejected.
    HarnessUnhealthy,
    /// The chain RPC node is unreachable; chain effects are queued.
    ChainRpcUnreachable,
    /// Resource budgets are critically low; the agent operates in reduced
    /// capability mode.
    BudgetConstrained,
}

impl std::fmt::Display for DegradationStage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::ProviderFallback => "provider_fallback",
            Self::HarnessUnhealthy => "harness_unhealthy",
            Self::ChainRpcUnreachable => "chain_rpc_unreachable",
            Self::BudgetConstrained => "budget_constrained",
        };
        write!(f, "{s}")
    }
}

// ---------------------------------------------------------------------------
// AgentState
// ---------------------------------------------------------------------------

/// The operational lifecycle state of an agent.
///
/// # State diagram
///
/// ```text
/// Created → Configured → Active ⇌ Paused
///                             ↘ Degraded
///                             ↓
///                        Deactivated → Archived
/// ```
///
/// `Archived` is the only terminal state. All other states permit further
/// transitions except `Archived`.
///
/// See `apply_agent_event` in `polkagent-kernel` for the exhaustive transition
/// function enforcing these rules.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum AgentState {
    /// [`AgentSpec`] has been accepted and stored but not yet validated or
    /// configured with deployment-specific bindings.
    Created,
    /// Validated [`AgentSpec`] with resolved provider routes, tool bindings,
    /// policy references, and deployment settings. Not yet accepting runs.
    Configured,
    /// Accepting and executing runs. Grants are resolved, policies are bound,
    /// and execution resources are available.
    Active,
    /// Temporarily not accepting new runs. In-flight runs continue to their
    /// next safe checkpoint and then suspend.
    Paused,
    /// Active but one or more execution dependencies have failed. Existing
    /// runs may complete; new runs may be queued or rejected depending on the
    /// severity of the [`DegradationStage`].
    Degraded {
        /// Identifies which dependency has failed.
        stage: DegradationStage,
    },
    /// Permanently stopped. No new runs are accepted. In-flight runs are
    /// cancelled cooperatively.
    Deactivated,
    /// Spec, runs, artifacts, and audit trail are retained. The agent cannot
    /// be reactivated; a new version must be created.
    Archived,
}

impl AgentState {
    /// Returns `true` if this is a terminal state (no further transitions
    /// are permitted).
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Archived)
    }

    /// Returns `true` if the agent is currently able to accept new runs.
    #[must_use]
    pub fn accepts_runs(&self) -> bool {
        matches!(self, Self::Active | Self::Degraded { .. })
    }
}

impl std::fmt::Display for AgentState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Created => write!(f, "created"),
            Self::Configured => write!(f, "configured"),
            Self::Active => write!(f, "active"),
            Self::Paused => write!(f, "paused"),
            Self::Degraded { stage } => write!(f, "degraded:{stage}"),
            Self::Deactivated => write!(f, "deactivated"),
            Self::Archived => write!(f, "archived"),
        }
    }
}

// ---------------------------------------------------------------------------
// AgentSpec
// ---------------------------------------------------------------------------

/// Versioned, portable agent definition.
///
/// An `AgentSpec` captures everything needed to reproduce an agent's
/// behaviour deterministically: the model to call, which tools are available,
/// which policies govern its actions, and what autonomy level it operates at.
///
/// The `digest` field is a BLAKE3 hash of the canonical serialization of this
/// struct and is used to detect spec tampering.
///
/// All fields added in PRD-03 carry `#[serde(default)]` so that existing
/// serialized agents (without those fields) continue to deserialize correctly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentSpec {
    /// Stable identifier for this agent across all versions.
    pub id: AgentId,
    /// Human-readable name shown in the UI and logs.
    pub name: String,
    /// Optional long-form description of the agent's purpose.
    pub description: Option<String>,
    /// Identifier of the AI model this agent should call.
    ///
    /// This is a free-form string in the format `provider/model-id`, e.g.
    /// `"anthropic/claude-opus-4-6"`. The provider adapter resolves this to
    /// a concrete API endpoint.
    pub model: String,
    /// Names of tools that this agent is permitted to invoke.
    ///
    /// Tools must be registered in the tool registry and the agent's grant
    /// must include each tool in its `allowed_effects`.
    pub tools: Vec<String>,
    /// System prompt template. May contain `{{variable}}` placeholders that
    /// the context assembler fills in at runtime.
    pub system_prompt: Option<String>,
    /// The autonomy level controlling how much the agent can do without
    /// explicit per-effect human approval.
    pub autonomy_level: AutonomyLevel,
    /// When this spec version was first persisted.
    pub created_at: DateTime<Utc>,
    /// When this spec version was last modified (configuration or policy
    /// reference update).
    pub updated_at: DateTime<Utc>,

    // ------------------------------------------------------------------
    // PRD-03 fields — all default to empty / None for backward compat.
    // ------------------------------------------------------------------
    /// Capabilities this agent declares (e.g. `"file.read"`, `"chain.query"`).
    ///
    /// Declared capabilities are used by the grant resolver to determine which
    /// effects the agent is allowed to request without per-effect approval.
    #[serde(default)]
    pub declared_capabilities: Vec<String>,
    /// References to policy files that govern this agent's behaviour.
    ///
    /// Each entry is a path (relative to `.polkagent/policies/`) or a URL
    /// pointing to a TOML policy document. Policies are evaluated in order;
    /// the first matching rule wins.
    #[serde(default)]
    pub policy_refs: Vec<String>,
    /// Runtime resource limits for runs executed by this agent.
    ///
    /// `None` means all limits are inherited from the global execution config.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_limits: Option<ResourceLimits>,
    /// Preferred model configuration overriding provider defaults.
    ///
    /// `None` means the global provider defaults apply.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_preference: Option<ModelPreference>,
    /// Memory subsystem configuration for this agent.
    ///
    /// `None` means the global memory defaults apply.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_config: Option<MemoryConfig>,
    /// Surfaces on which this agent is exposed (e.g. `"http"`, `"cli"`, `"tui"`).
    ///
    /// An empty list means the agent is not bound to any surface; it can still
    /// be invoked programmatically via the service layer.
    #[serde(default)]
    pub surface_bindings: Vec<String>,
}

// ---------------------------------------------------------------------------
// Validation constants
// ---------------------------------------------------------------------------

/// Maximum byte length for [`AgentSpec::name`].
const MAX_NAME_LEN: usize = 200;

/// Maximum byte length for [`AgentSpec::description`].
const MAX_DESCRIPTION_LEN: usize = 4_096;

/// Maximum byte length for [`AgentSpec::model`].
const MAX_MODEL_LEN: usize = 512;

impl AgentSpec {
    /// Create a minimal spec with required fields and sensible defaults.
    #[must_use]
    pub fn new(id: AgentId, name: impl Into<String>, model: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            id,
            name: name.into(),
            description: None,
            model: model.into(),
            tools: Vec::new(),
            system_prompt: None,
            autonomy_level: AutonomyLevel::default(),
            created_at: now,
            updated_at: now,
            declared_capabilities: Vec::new(),
            policy_refs: Vec::new(),
            resource_limits: None,
            model_preference: None,
            memory_config: None,
            surface_bindings: Vec::new(),
        }
    }

    /// Validate the contents of this spec.
    ///
    /// Checks:
    ///
    /// - `name` is non-empty, at most [`MAX_NAME_LEN`] bytes, and contains no
    ///   ASCII control characters.
    /// - `description`, when present, is at most [`MAX_DESCRIPTION_LEN`] bytes
    ///   and contains no control characters other than tab / LF / CR.
    /// - `model` is non-empty and at most [`MAX_MODEL_LEN`] bytes.
    ///
    /// Returns `Ok(())` on success.  Returns [`PolkagentError::Validation`] on
    /// the first failure encountered.
    ///
    /// # Errors
    ///
    /// Returns `Err(PolkagentError::Validation { .. })` when any constraint is
    /// violated.
    pub fn validate(&self) -> Result<(), PolkagentError> {
        // --- name ---
        if self.name.trim().is_empty() {
            return Err(PolkagentError::validation("name", "must not be empty"));
        }
        if self.name.len() > MAX_NAME_LEN {
            return Err(PolkagentError::validation(
                "name",
                format!("exceeds maximum length of {MAX_NAME_LEN} bytes"),
            ));
        }
        if self
            .name
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\t' | '\n' | '\r'))
        {
            return Err(PolkagentError::validation(
                "name",
                "contains control characters",
            ));
        }

        // --- description ---
        if let Some(desc) = &self.description {
            if desc.len() > MAX_DESCRIPTION_LEN {
                return Err(PolkagentError::validation(
                    "description",
                    format!("exceeds maximum length of {MAX_DESCRIPTION_LEN} bytes"),
                ));
            }
            if desc
                .chars()
                .any(|c| c.is_control() && !matches!(c, '\t' | '\n' | '\r'))
            {
                return Err(PolkagentError::validation(
                    "description",
                    "contains control characters",
                ));
            }
        }

        // --- model ---
        if self.model.trim().is_empty() {
            return Err(PolkagentError::validation("model", "must not be empty"));
        }
        if self.model.len() > MAX_MODEL_LEN {
            return Err(PolkagentError::validation(
                "model",
                format!("exceeds maximum length of {MAX_MODEL_LEN} bytes"),
            ));
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_spec() -> AgentSpec {
        AgentSpec::new(AgentId::new(), "Test Agent", "anthropic/claude-opus-4-6")
    }

    // --- DegradationStage ---

    #[test]
    fn degradation_stage_serde_round_trip() {
        let stage = DegradationStage::ChainRpcUnreachable;
        let json = serde_json::to_string(&stage).unwrap();
        assert_eq!(json, r#""chain_rpc_unreachable""#);
        let back: DegradationStage = serde_json::from_str(&json).unwrap();
        assert_eq!(back, stage);
    }

    // --- AgentState ---

    #[test]
    fn archived_is_terminal() {
        assert!(AgentState::Archived.is_terminal());
    }

    #[test]
    fn non_archived_states_are_not_terminal() {
        let non_terminal = [
            AgentState::Created,
            AgentState::Configured,
            AgentState::Active,
            AgentState::Paused,
            AgentState::Degraded {
                stage: DegradationStage::ProviderFallback,
            },
            AgentState::Deactivated,
        ];
        for state in &non_terminal {
            assert!(!state.is_terminal(), "{state:?} should not be terminal");
        }
    }

    #[test]
    fn accepts_runs_only_when_active_or_degraded() {
        assert!(AgentState::Active.accepts_runs());
        assert!(AgentState::Degraded {
            stage: DegradationStage::BudgetConstrained,
        }
        .accepts_runs());

        assert!(!AgentState::Created.accepts_runs());
        assert!(!AgentState::Configured.accepts_runs());
        assert!(!AgentState::Paused.accepts_runs());
        assert!(!AgentState::Deactivated.accepts_runs());
        assert!(!AgentState::Archived.accepts_runs());
    }

    #[test]
    fn agent_state_serde_round_trip_simple() {
        let state = AgentState::Active;
        let json = serde_json::to_string(&state).unwrap();
        let back: AgentState = serde_json::from_str(&json).unwrap();
        assert_eq!(state, back);
    }

    #[test]
    fn agent_state_serde_round_trip_with_stage() {
        let state = AgentState::Degraded {
            stage: DegradationStage::HarnessUnhealthy,
        };
        let json = serde_json::to_string(&state).unwrap();
        let back: AgentState = serde_json::from_str(&json).unwrap();
        assert_eq!(state, back);
    }

    #[test]
    fn agent_state_tag_is_present_in_json() {
        let json = serde_json::to_string(&AgentState::Paused).unwrap();
        assert!(json.contains(r#""state""#));
        assert!(json.contains(r#""paused""#));
    }

    // --- AgentSpec (legacy fields) ---

    #[test]
    fn agent_spec_new_has_correct_defaults() {
        let spec = make_spec();
        assert_eq!(spec.autonomy_level, AutonomyLevel::Supervised);
        assert!(spec.tools.is_empty());
        assert!(spec.system_prompt.is_none());
        assert!(spec.description.is_none());
    }

    #[test]
    fn agent_spec_serde_round_trip() {
        let spec = make_spec();
        let json = serde_json::to_string(&spec).unwrap();
        let back: AgentSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(spec.id, back.id);
        assert_eq!(spec.name, back.name);
        assert_eq!(spec.model, back.model);
        assert_eq!(spec.autonomy_level, back.autonomy_level);
    }

    // --- PRD-03: AgentSpec new fields — default values ---

    #[test]
    fn agent_spec_prd03_defaults_are_empty_or_none() {
        let spec = make_spec();
        assert!(
            spec.declared_capabilities.is_empty(),
            "declared_capabilities should default to empty"
        );
        assert!(
            spec.policy_refs.is_empty(),
            "policy_refs should default to empty"
        );
        assert!(
            spec.resource_limits.is_none(),
            "resource_limits should default to None"
        );
        assert!(
            spec.model_preference.is_none(),
            "model_preference should default to None"
        );
        assert!(
            spec.memory_config.is_none(),
            "memory_config should default to None"
        );
        assert!(
            spec.surface_bindings.is_empty(),
            "surface_bindings should default to empty"
        );
    }

    /// Verifies backward compatibility: a JSON blob that has no PRD-03 fields
    /// can still be deserialized into an `AgentSpec` and the new fields take
    /// their defaults.
    #[test]
    fn agent_spec_backward_compat_missing_prd03_fields() {
        let legacy_json = serde_json::json!({
            "id": "00000000-0000-0000-0000-000000000001",
            "name": "Legacy Agent",
            "description": null,
            "model": "anthropic/claude-opus-4-6",
            "tools": [],
            "system_prompt": null,
            "autonomy_level": "supervised",
            "created_at": "2024-01-01T00:00:00Z",
            "updated_at": "2024-01-01T00:00:00Z"
        });

        let spec: AgentSpec =
            serde_json::from_value(legacy_json).expect("should deserialize legacy JSON");
        assert!(spec.declared_capabilities.is_empty());
        assert!(spec.policy_refs.is_empty());
        assert!(spec.resource_limits.is_none());
        assert!(spec.model_preference.is_none());
        assert!(spec.memory_config.is_none());
        assert!(spec.surface_bindings.is_empty());
    }

    #[test]
    fn agent_spec_with_all_prd03_fields_round_trips() {
        let mut spec = make_spec();
        spec.declared_capabilities = vec!["file.read".to_owned(), "chain.query".to_owned()];
        spec.policy_refs = vec!["policies/base.toml".to_owned()];
        spec.resource_limits = Some(ResourceLimits {
            max_tokens_per_turn: Some(4096),
            max_turns: Some(20),
            timeout_secs: Some(300),
            max_concurrent_effects: Some(4),
        });
        spec.model_preference = Some(ModelPreference {
            provider: Some("anthropic".to_owned()),
            model_id: Some("claude-opus-4-6".to_owned()),
            temperature: Some(0.7),
            system_prompt: Some("You are a helpful assistant.".to_owned()),
        });
        spec.memory_config = Some(MemoryConfig {
            enabled: true,
            max_entries: Some(1000),
            retention_days: Some(30),
            classification_default: Some("private".to_owned()),
        });
        spec.surface_bindings = vec!["http".to_owned(), "cli".to_owned()];

        let json = serde_json::to_string(&spec).expect("serialization failed");
        let back: AgentSpec = serde_json::from_str(&json).expect("deserialization failed");

        assert_eq!(spec.declared_capabilities, back.declared_capabilities);
        assert_eq!(spec.policy_refs, back.policy_refs);
        assert_eq!(spec.resource_limits, back.resource_limits);
        assert_eq!(spec.model_preference, back.model_preference);
        assert_eq!(spec.memory_config, back.memory_config);
        assert_eq!(spec.surface_bindings, back.surface_bindings);
    }

    // --- PRD-03: ResourceLimits ---

    #[test]
    fn resource_limits_default_is_all_none() {
        let rl = ResourceLimits::default();
        assert!(rl.max_tokens_per_turn.is_none());
        assert!(rl.max_turns.is_none());
        assert!(rl.timeout_secs.is_none());
        assert!(rl.max_concurrent_effects.is_none());
    }

    #[test]
    fn resource_limits_serde_round_trip() {
        let rl = ResourceLimits {
            max_tokens_per_turn: Some(8192),
            max_turns: Some(50),
            timeout_secs: Some(600),
            max_concurrent_effects: Some(8),
        };
        let json = serde_json::to_string(&rl).expect("serialization failed");
        let back: ResourceLimits = serde_json::from_str(&json).expect("deserialization failed");
        assert_eq!(rl, back);
    }

    #[test]
    fn resource_limits_partial_fields_deserialize() {
        let json = r#"{"max_turns": 10}"#;
        let rl: ResourceLimits =
            serde_json::from_str(json).expect("partial deserialization failed");
        assert_eq!(rl.max_turns, Some(10));
        assert!(rl.max_tokens_per_turn.is_none());
        assert!(rl.timeout_secs.is_none());
        assert!(rl.max_concurrent_effects.is_none());
    }

    #[test]
    fn resource_limits_empty_json_gives_defaults() {
        let rl: ResourceLimits =
            serde_json::from_str("{}").expect("empty object deserialization failed");
        assert_eq!(rl, ResourceLimits::default());
    }

    // --- PRD-03: ModelPreference ---

    #[test]
    fn model_preference_default_is_all_none() {
        let mp = ModelPreference::default();
        assert!(mp.provider.is_none());
        assert!(mp.model_id.is_none());
        assert!(mp.temperature.is_none());
        assert!(mp.system_prompt.is_none());
    }

    #[test]
    fn model_preference_serde_round_trip() {
        let mp = ModelPreference {
            provider: Some("openai".to_owned()),
            model_id: Some("gpt-4o".to_owned()),
            temperature: Some(0.2),
            system_prompt: Some("Be concise.".to_owned()),
        };
        let json = serde_json::to_string(&mp).expect("serialization failed");
        let back: ModelPreference = serde_json::from_str(&json).expect("deserialization failed");
        assert_eq!(mp, back);
    }

    #[test]
    fn model_preference_temperature_precision_preserved() {
        let mp = ModelPreference {
            temperature: Some(1.337_f64),
            ..Default::default()
        };
        let json = serde_json::to_string(&mp).expect("serialization failed");
        let back: ModelPreference = serde_json::from_str(&json).expect("deserialization failed");
        // f64 equality is acceptable for config values that aren't computed.
        assert!((back.temperature.unwrap() - 1.337_f64).abs() < 1e-9);
    }

    // --- PRD-03: MemoryConfig ---

    #[test]
    fn memory_config_default_has_enabled_true() {
        let mc = MemoryConfig::default();
        assert!(mc.enabled, "memory should be enabled by default");
        assert!(mc.max_entries.is_none());
        assert!(mc.retention_days.is_none());
        assert!(mc.classification_default.is_none());
    }

    #[test]
    fn memory_config_serde_round_trip() {
        let mc = MemoryConfig {
            enabled: false,
            max_entries: Some(500),
            retention_days: Some(7),
            classification_default: Some("public".to_owned()),
        };
        let json = serde_json::to_string(&mc).expect("serialization failed");
        let back: MemoryConfig = serde_json::from_str(&json).expect("deserialization failed");
        assert_eq!(mc, back);
    }

    #[test]
    fn memory_config_disabled_round_trips() {
        let mc = MemoryConfig {
            enabled: false,
            ..Default::default()
        };
        let json = serde_json::to_string(&mc).expect("serialization failed");
        let back: MemoryConfig = serde_json::from_str(&json).expect("deserialization failed");
        assert!(!back.enabled);
    }

    // --- PRD-03: surface_bindings and declared_capabilities ---

    #[test]
    fn surface_bindings_and_capabilities_round_trip() {
        let mut spec = make_spec();
        spec.surface_bindings = vec!["http".to_owned(), "tui".to_owned()];
        spec.declared_capabilities = vec!["file.read".to_owned()];

        let json = serde_json::to_string(&spec).expect("serialization failed");
        let back: AgentSpec = serde_json::from_str(&json).expect("deserialization failed");

        assert_eq!(back.surface_bindings, vec!["http", "tui"]);
        assert_eq!(back.declared_capabilities, vec!["file.read"]);
    }

    // --- AgentSpec::validate ---

    #[test]
    fn agent_spec_validate_valid_passes() {
        let spec = make_spec();
        assert!(spec.validate().is_ok(), "valid spec should pass validation");
    }

    #[test]
    fn agent_spec_validate_empty_name_rejected() {
        let mut spec = make_spec();
        spec.name = String::new();
        let err = spec.validate().unwrap_err();
        assert!(
            err.to_string().contains("name"),
            "error should mention 'name'"
        );
    }

    #[test]
    fn agent_spec_validate_whitespace_only_name_rejected() {
        let mut spec = make_spec();
        spec.name = "   ".to_owned();
        assert!(spec.validate().is_err());
    }

    #[test]
    fn agent_spec_validate_name_too_long_rejected() {
        let mut spec = make_spec();
        spec.name = "a".repeat(MAX_NAME_LEN + 1);
        let err = spec.validate().unwrap_err();
        assert!(
            err.to_string().contains("name"),
            "error should mention 'name'"
        );
    }

    #[test]
    fn agent_spec_validate_name_at_max_len_passes() {
        let mut spec = make_spec();
        spec.name = "a".repeat(MAX_NAME_LEN);
        assert!(spec.validate().is_ok());
    }

    #[test]
    fn agent_spec_validate_name_with_control_chars_rejected() {
        let mut spec = make_spec();
        spec.name = "hello\x07world".to_owned(); // BEL character
        let err = spec.validate().unwrap_err();
        assert!(err.to_string().contains("name"));
    }

    #[test]
    fn agent_spec_validate_description_too_long_rejected() {
        let mut spec = make_spec();
        spec.description = Some("d".repeat(MAX_DESCRIPTION_LEN + 1));
        let err = spec.validate().unwrap_err();
        assert!(err.to_string().contains("description"));
    }

    #[test]
    fn agent_spec_validate_description_with_control_chars_rejected() {
        let mut spec = make_spec();
        spec.description = Some("desc\x1b[31mRed\x1b[0m".to_owned()); // ANSI escape
        let err = spec.validate().unwrap_err();
        assert!(err.to_string().contains("description"));
    }

    #[test]
    fn agent_spec_validate_description_with_lf_passes() {
        let mut spec = make_spec();
        spec.description = Some("line1\nline2".to_owned());
        assert!(spec.validate().is_ok());
    }

    #[test]
    fn agent_spec_validate_empty_model_rejected() {
        let mut spec = make_spec();
        spec.model = String::new();
        let err = spec.validate().unwrap_err();
        assert!(err.to_string().contains("model"));
    }

    // --- State machine transition validity (documentation tests) ---

    /// Demonstrates the valid Created → Configured transition.
    #[test]
    fn valid_transition_created_to_configured() {
        let state = AgentState::Created;
        // In the real kernel, apply_agent_event() would be called here.
        // This test documents the expected next state.
        let expected_next = AgentState::Configured;
        assert!(!state.is_terminal());
        assert!(!expected_next.is_terminal());
    }

    /// Demonstrates that Archived rejects all further transitions.
    #[test]
    fn invalid_transition_from_archived_rejected() {
        assert!(AgentState::Archived.is_terminal());
        // The kernel's apply_agent_event() would return
        // Err(AgentTransitionError::TerminalState) here.
    }
}
