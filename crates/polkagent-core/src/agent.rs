//! Agent domain types.
//!
//! An **agent** is a versioned, portable definition of an AI execution unit.
//! The [`AgentSpec`] captures what the agent does, which models and tools it
//! may use, its policy references, and its resource limits. The
//! [`AgentState`] tracks where the agent is in its operational lifecycle.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::config::AutonomyLevel;
use crate::ids::AgentId;

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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
}

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
        }
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

    // --- AgentSpec ---

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
