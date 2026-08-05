//! Interaction targets, configuration, requests, and durable handles.

use std::collections::BTreeSet;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use polkagent_core::config::AutonomyLevel;
use polkagent_core::ids::{AgentId, ArtifactId, ConversationId, RunId};
use polkagent_core::usage::Budget;
use polkagent_group::GroupId;
use serde::{Deserialize, Serialize};

use crate::error::InteractionError;
use crate::ids::InteractionTurnId;

/// The durable execution target selected for an interaction.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum InteractionTarget {
    /// Route prompts to one configured agent.
    Agent(AgentId),
    /// Route prompts through one configured orchestration group.
    Group(GroupId),
    /// Let the runtime choose an eligible target.
    Auto,
}

/// Session-scoped execution configuration shared by every surface.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InteractionConfig {
    /// Current agent, group, or automatic target.
    pub target: InteractionTarget,
    /// Optional model override.
    pub model: Option<String>,
    /// Optional provider override.
    pub provider: Option<String>,
    /// Optional downstream harness override; `None` means executor-only or
    /// runtime default.
    pub harness: Option<String>,
    /// Requested autonomy level. Policy and grants may narrow this value.
    pub autonomy: AutonomyLevel,
    /// Maximum executor turns for one human interaction turn.
    pub max_turns: Option<u32>,
    /// Optional interaction budget limits.
    pub budget: Option<Budget>,
}

impl InteractionConfig {
    /// Create a supervised configuration for the supplied target.
    pub fn new(target: InteractionTarget) -> Self {
        Self {
            target,
            model: None,
            provider: None,
            harness: None,
            autonomy: AutonomyLevel::default(),
            max_turns: None,
            budget: None,
        }
    }

    /// Validate surface-provided configuration before runtime resolution.
    pub fn validate(&self) -> Result<(), InteractionError> {
        validate_optional_label("model", self.model.as_deref())?;
        validate_optional_label("provider", self.provider.as_deref())?;
        validate_optional_label("harness", self.harness.as_deref())?;
        if self.max_turns == Some(0) {
            return Err(InteractionError::invalid_config(
                "max_turns must be greater than zero",
            ));
        }
        if let Some(budget) = &self.budget {
            validate_budget(budget)?;
        }
        Ok(())
    }
}

fn validate_optional_label(field: &str, value: Option<&str>) -> Result<(), InteractionError> {
    if value.is_some_and(|value| value.trim().is_empty()) {
        return Err(InteractionError::invalid_config(format!(
            "{field} cannot be empty when set"
        )));
    }
    Ok(())
}

fn validate_budget(budget: &Budget) -> Result<(), InteractionError> {
    let costs = [
        budget.max_cost_usd,
        budget
            .per_run
            .as_ref()
            .and_then(|limits| limits.max_cost_usd),
        budget
            .per_turn
            .as_ref()
            .and_then(|limits| limits.max_cost_usd),
    ];
    if costs
        .into_iter()
        .flatten()
        .any(|value| !value.is_finite() || value < 0.0)
    {
        return Err(InteractionError::invalid_config(
            "budget costs must be finite and non-negative",
        ));
    }
    Ok(())
}

/// A tri-state override that distinguishes inheritance from clearing a value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(tag = "action", content = "value", rename_all = "snake_case")]
pub enum OverrideValue<T> {
    /// Retain the interaction's current value.
    #[default]
    Inherit,
    /// Replace the current value.
    Set(T),
    /// Explicitly clear the current optional value.
    Clear,
}

impl<T: Clone> OverrideValue<T> {
    fn apply(&self, current: &mut Option<T>) {
        match self {
            Self::Inherit => {}
            Self::Set(value) => *current = Some(value.clone()),
            Self::Clear => *current = None,
        }
    }
}

/// Per-prompt configuration changes that do not mutate session defaults.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct InteractionOverrides {
    /// Optional target replacement.
    pub target: Option<InteractionTarget>,
    /// Model override, inheritance, or explicit clearing.
    pub model: OverrideValue<String>,
    /// Provider override, inheritance, or explicit clearing.
    pub provider: OverrideValue<String>,
    /// Harness override, inheritance, or explicit clearing.
    pub harness: OverrideValue<String>,
    /// Optional autonomy replacement.
    pub autonomy: Option<AutonomyLevel>,
    /// Maximum-turn override, inheritance, or clearing.
    pub max_turns: OverrideValue<u32>,
    /// Budget override, inheritance, or clearing.
    pub budget: OverrideValue<Budget>,
}

impl InteractionOverrides {
    /// Apply these overrides to session configuration and validate the result.
    pub fn apply_to(
        &self,
        base: &InteractionConfig,
    ) -> Result<InteractionConfig, InteractionError> {
        let mut resolved = base.clone();
        if let Some(target) = &self.target {
            resolved.target = target.clone();
        }
        self.model.apply(&mut resolved.model);
        self.provider.apply(&mut resolved.provider);
        self.harness.apply(&mut resolved.harness);
        if let Some(autonomy) = self.autonomy {
            resolved.autonomy = autonomy;
        }
        self.max_turns.apply(&mut resolved.max_turns);
        self.budget.apply(&mut resolved.budget);
        resolved.validate()?;
        Ok(resolved)
    }
}

/// One capability negotiated with the initiating client or surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientCapability {
    /// The client can render structured tool calls.
    StructuredTools,
    /// The client can answer approval requests interactively.
    Approvals,
    /// The client can render plan updates.
    Plans,
    /// The client can resolve linked resources.
    Resources,
}

/// Extensible capability set negotiated with the initiating client or surface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ClientCapabilities {
    /// Capabilities supported by the client.
    pub supported: BTreeSet<ClientCapability>,
}

impl ClientCapabilities {
    /// Whether the client supports a capability.
    pub fn supports(&self, capability: ClientCapability) -> bool {
        self.supported.contains(&capability)
    }

    /// Add a supported capability.
    #[must_use]
    pub fn with(mut self, capability: ClientCapability) -> Self {
        self.supported.insert(capability);
        self
    }
}

/// Safe context supplied by the initiating surface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientContext {
    /// Working directory authoritative for relative project operations.
    pub working_directory: PathBuf,
    /// Optional stable client name such as `terminal`, `tui`, or an editor.
    pub client_name: Option<String>,
    /// Optional client-local session correlation token.
    pub client_session_id: Option<String>,
    /// Negotiated rendering and interaction capabilities.
    pub capabilities: ClientCapabilities,
}

impl ClientContext {
    /// Construct context rooted at an absolute working directory.
    pub fn new(working_directory: PathBuf) -> Result<Self, InteractionError> {
        if !working_directory.is_absolute() {
            return Err(InteractionError::invalid_request(
                "working_directory must be absolute",
            ));
        }
        Ok(Self {
            working_directory,
            client_name: None,
            client_session_id: None,
            capabilities: ClientCapabilities::default(),
        })
    }
}

/// One surface-neutral prompt content block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InteractionContent {
    /// Plain user-authored text.
    Text {
        /// Text content.
        text: String,
    },
    /// A link to a client-visible resource. The runtime must validate access.
    ResourceLink {
        /// Stable URI supplied by the client.
        uri: String,
        /// Optional presentation name.
        name: Option<String>,
        /// Optional media type.
        mime_type: Option<String>,
    },
    /// A reference to an existing durable Polkagent artifact.
    Artifact {
        /// Referenced artifact identity.
        artifact_id: ArtifactId,
    },
}

impl InteractionContent {
    fn validate(&self) -> Result<(), InteractionError> {
        match self {
            Self::Text { text } if text.trim().is_empty() => Err(
                InteractionError::invalid_request("text content cannot be empty"),
            ),
            Self::ResourceLink { uri, .. } if uri.trim().is_empty() => Err(
                InteractionError::invalid_request("resource URI cannot be empty"),
            ),
            _ => Ok(()),
        }
    }
}

/// A request to append a user turn and start linked execution.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromptRequest {
    /// Durable conversation/interaction identity.
    pub conversation_id: ConversationId,
    /// Ordered prompt content blocks.
    pub content: Vec<InteractionContent>,
    /// Per-turn changes layered over session configuration.
    pub config_overrides: InteractionOverrides,
    /// Initiating client context.
    pub client_context: ClientContext,
}

impl PromptRequest {
    /// Validate the request without resolving runtime-dependent readiness.
    pub fn validate(&self) -> Result<(), InteractionError> {
        if self.content.is_empty() {
            return Err(InteractionError::invalid_request(
                "prompt must contain at least one content block",
            ));
        }
        for content in &self.content {
            content.validate()?;
        }
        Ok(())
    }
}

/// Stable handle returned immediately after a turn is durably created.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnHandle {
    /// Durable interaction turn identity.
    pub turn_id: InteractionTurnId,
    /// Parent conversation identity.
    pub conversation_id: ConversationId,
    /// Runs linked at creation time, in deterministic target order.
    pub run_ids: Vec<RunId>,
    /// First durable interaction event sequence for this turn.
    pub first_event_sequence: u64,
}

/// Durable lifecycle of an interaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InteractionState {
    /// The interaction accepts prompts.
    Active,
    /// The interaction is retained but no longer accepts prompts.
    Archived,
}

/// Durable lifecycle of one human interaction turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnState {
    /// User input is durable but linked execution has not started.
    Pending,
    /// One or more linked runs are executing.
    Running,
    /// Execution is paused for a real approval request.
    AwaitingApproval,
    /// All linked work completed successfully.
    Completed,
    /// The turn failed.
    Failed,
    /// The turn was cancelled.
    Cancelled,
    /// The turn exceeded its deadline.
    TimedOut,
}

impl TurnState {
    /// Whether this state permits no further execution transitions.
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Cancelled | Self::TimedOut
        )
    }
}

/// List/load projection for a durable interaction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InteractionSummary {
    /// Conversation identity used as the durable interaction identity.
    pub conversation_id: ConversationId,
    /// Optional human-readable title.
    pub title: Option<String>,
    /// Current session configuration.
    pub config: InteractionConfig,
    /// Current lifecycle state.
    pub state: InteractionState,
    /// Number of durable turns.
    pub turn_count: u32,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Most recent durable update timestamp.
    pub updated_at: DateTime<Utc>,
}

/// Projection of one durable interaction turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnSummary {
    /// Stable handle and run correlation.
    pub handle: TurnHandle,
    /// Monotonic ordinal within the interaction.
    pub ordinal: u32,
    /// Current lifecycle state.
    pub state: TurnState,
    /// Start timestamp.
    pub started_at: DateTime<Utc>,
    /// Terminal timestamp, when known.
    pub completed_at: Option<DateTime<Utc>>,
}

/// Request to create a durable interaction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateInteractionRequest {
    /// Optional presentation title.
    pub title: Option<String>,
    /// Initial session configuration.
    pub config: InteractionConfig,
    /// Initiating client context.
    pub client_context: ClientContext,
}

/// Pagination and lifecycle filters for interaction listing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListInteractionsRequest {
    /// Maximum number of summaries to return.
    pub limit: u32,
    /// Number of summaries to skip.
    pub offset: u32,
    /// Optional lifecycle-state filter.
    pub state: Option<InteractionState>,
}

impl Default for ListInteractionsRequest {
    fn default() -> Self {
        Self {
            limit: 50,
            offset: 0,
            state: None,
        }
    }
}

/// A configurable session property shared with command and ACP adapters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigOption {
    /// Interaction target.
    Target,
    /// Model selection.
    Model,
    /// Provider selection.
    Provider,
    /// Harness selection.
    Harness,
    /// Autonomy level.
    Autonomy,
    /// Per-turn executor-turn ceiling.
    MaxTurns,
    /// Interaction budget.
    Budget,
}

/// Typed value for a session configuration update.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum ConfigOptionValue {
    /// Agent, group, or automatic target.
    Target(InteractionTarget),
    /// Optional model; `None` clears the override.
    Model(Option<String>),
    /// Optional provider; `None` clears the override.
    Provider(Option<String>),
    /// Optional harness; `None` selects executor-only/runtime default.
    Harness(Option<String>),
    /// Autonomy setting.
    Autonomy(AutonomyLevel),
    /// Optional turn ceiling; `None` clears it.
    MaxTurns(Option<u32>),
    /// Optional budget; `None` clears it.
    Budget(Option<Budget>),
}

/// One atomic session configuration change.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfigUpdate {
    /// Property being updated.
    pub option: ConfigOption,
    /// Typed new value.
    pub value: ConfigOptionValue,
}

impl ConfigUpdate {
    /// Validate that the option and value variants agree.
    pub fn validate(&self) -> Result<(), InteractionError> {
        let matches = matches!(
            (self.option, &self.value),
            (ConfigOption::Target, ConfigOptionValue::Target(_))
                | (ConfigOption::Model, ConfigOptionValue::Model(_))
                | (ConfigOption::Provider, ConfigOptionValue::Provider(_))
                | (ConfigOption::Harness, ConfigOptionValue::Harness(_))
                | (ConfigOption::Autonomy, ConfigOptionValue::Autonomy(_))
                | (ConfigOption::MaxTurns, ConfigOptionValue::MaxTurns(_))
                | (ConfigOption::Budget, ConfigOptionValue::Budget(_))
        );
        if !matches {
            return Err(InteractionError::invalid_request(
                "configuration option and value type do not match",
            ));
        }
        match &self.value {
            ConfigOptionValue::Model(value)
            | ConfigOptionValue::Provider(value)
            | ConfigOptionValue::Harness(value) => {
                validate_optional_label("configuration value", value.as_deref())?;
            }
            ConfigOptionValue::MaxTurns(Some(0)) => {
                return Err(InteractionError::invalid_config(
                    "max_turns must be greater than zero",
                ));
            }
            ConfigOptionValue::Budget(Some(budget)) => validate_budget(budget)?,
            _ => {}
        }
        Ok(())
    }
}

/// Operator decision for a real pending approval.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum ApprovalDecision {
    /// Approve the pending effect once.
    Approve,
    /// Deny the pending effect.
    Deny {
        /// Optional safe operator rationale.
        reason: Option<String>,
    },
}

/// Replay-aware request for live interaction events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubscriptionRequest {
    /// Interaction to observe.
    pub conversation_id: ConversationId,
    /// Optional turn filter.
    pub turn_id: Option<InteractionTurnId>,
    /// Last durable sequence already observed; replay starts after it.
    pub after_sequence: Option<u64>,
    /// Requested bounded live-channel capacity.
    pub capacity: usize,
}

impl SubscriptionRequest {
    /// Validate the requested channel shape.
    pub fn validate(&self) -> Result<(), InteractionError> {
        if self.capacity == 0 {
            return Err(InteractionError::invalid_request(
                "subscription capacity must be greater than zero",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use polkagent_core::usage::BudgetLimits;

    use super::*;

    fn context() -> ClientContext {
        ClientContext::new(PathBuf::from("/tmp/project")).expect("absolute context")
    }

    #[test]
    fn default_config_is_supervised_and_valid() {
        let config = InteractionConfig::new(InteractionTarget::Auto);
        assert_eq!(config.autonomy, AutonomyLevel::Supervised);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn config_rejects_empty_labels_and_zero_turns() {
        let mut config = InteractionConfig::new(InteractionTarget::Auto);
        config.model = Some("  ".to_owned());
        assert!(config.validate().is_err());
        config.model = None;
        config.max_turns = Some(0);
        assert!(config.validate().is_err());
    }

    #[test]
    fn config_rejects_negative_and_non_finite_costs() {
        for value in [-1.0, f64::INFINITY, f64::NAN] {
            let mut config = InteractionConfig::new(InteractionTarget::Auto);
            config.budget = Some(Budget {
                max_cost_usd: Some(value),
                ..Budget::default()
            });
            assert!(config.validate().is_err());
        }
    }

    #[test]
    fn nested_budget_cost_is_validated() {
        let mut config = InteractionConfig::new(InteractionTarget::Auto);
        config.budget = Some(Budget {
            per_turn: Some(BudgetLimits {
                max_cost_usd: Some(-0.01),
                ..BudgetLimits::default()
            }),
            ..Budget::default()
        });
        assert!(config.validate().is_err());
    }

    #[test]
    fn overrides_distinguish_inherit_set_and_clear() {
        let mut base = InteractionConfig::new(InteractionTarget::Auto);
        base.model = Some("old".to_owned());
        base.provider = Some("provider".to_owned());
        let overrides = InteractionOverrides {
            model: OverrideValue::Set("new".to_owned()),
            provider: OverrideValue::Clear,
            ..InteractionOverrides::default()
        };
        let resolved = overrides.apply_to(&base).expect("valid overrides");
        assert_eq!(resolved.model.as_deref(), Some("new"));
        assert_eq!(resolved.provider, None);
    }

    #[test]
    fn client_context_requires_absolute_working_directory() {
        assert!(ClientContext::new(PathBuf::from("relative/path")).is_err());
        assert!(ClientContext::new(PathBuf::from("/absolute/path")).is_ok());
    }

    #[test]
    fn prompt_requires_nonempty_valid_content() {
        let mut request = PromptRequest {
            conversation_id: ConversationId::new(),
            content: Vec::new(),
            config_overrides: InteractionOverrides::default(),
            client_context: context(),
        };
        assert!(request.validate().is_err());
        request.content.push(InteractionContent::Text {
            text: "  ".to_owned(),
        });
        assert!(request.validate().is_err());
        request.content[0] = InteractionContent::Text {
            text: "hello".to_owned(),
        };
        assert!(request.validate().is_ok());
    }

    #[test]
    fn turn_terminal_states_are_exact() {
        assert!(!TurnState::Pending.is_terminal());
        assert!(!TurnState::Running.is_terminal());
        assert!(!TurnState::AwaitingApproval.is_terminal());
        assert!(TurnState::Completed.is_terminal());
        assert!(TurnState::Failed.is_terminal());
        assert!(TurnState::Cancelled.is_terminal());
        assert!(TurnState::TimedOut.is_terminal());
    }

    #[test]
    fn config_update_rejects_mismatched_types() {
        let update = ConfigUpdate {
            option: ConfigOption::Model,
            value: ConfigOptionValue::Target(InteractionTarget::Auto),
        };
        assert!(update.validate().is_err());
    }

    #[test]
    fn subscription_requires_bounded_nonzero_capacity() {
        let request = SubscriptionRequest {
            conversation_id: ConversationId::new(),
            turn_id: None,
            after_sequence: None,
            capacity: 0,
        };
        assert!(request.validate().is_err());
    }
}
