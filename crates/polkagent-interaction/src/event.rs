//! Structured, stable events projected to interactive surfaces.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use polkagent_core::ids::{ApprovalId, ConversationId, EffectId, RunId};
use polkagent_core::run::RunState;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::InteractionError;
use crate::ids::{InteractionEventId, InteractionTurnId, PlanEntryId, ToolCallId};
use crate::model::{ApprovalDecision, InteractionTarget};

/// The role a linked run plays within an interaction turn.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunRole {
    /// The only or primary run responsible for the user response.
    Primary,
    /// A coordinator run that owns child orchestration.
    Coordinator,
    /// A named child role in a group or delegated plan.
    Child(String),
}

/// Stable semantic category for a user-visible tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCallKind {
    /// Read-only data access.
    Read,
    /// File or state mutation.
    Write,
    /// Local command or program execution.
    Execute,
    /// External network access.
    Network,
    /// Chain query, signing, or submission.
    Chain,
    /// A tool that does not fit another stable category.
    Other,
}

/// User-visible lifecycle of a stable tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCallStatus {
    /// The call is known but has not started.
    Pending,
    /// Execution is in progress.
    InProgress,
    /// Execution is paused for approval.
    AwaitingApproval,
    /// Execution completed successfully.
    Succeeded,
    /// Execution failed.
    Failed,
    /// Execution was cancelled.
    Cancelled,
    /// The external outcome is genuinely indeterminate.
    Unknown,
}

impl ToolCallStatus {
    /// Whether the tool call has reached a terminal state.
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed | Self::Cancelled | Self::Unknown
        )
    }
}

/// Optional source location associated with tool input or output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolLocation {
    /// Workspace-relative or absolute path, according to client policy.
    pub path: PathBuf,
    /// Optional one-based start line.
    pub line: Option<u32>,
    /// Optional one-based start column.
    pub column: Option<u32>,
}

/// Durable projection of one tool call.
///
/// `arguments` and `output` must already be redacted and safe for the target
/// surface. Secret-bearing provider payloads must never be copied here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCallView {
    /// Stable identity retained across every update.
    pub call_id: ToolCallId,
    /// Linked execution run.
    pub run_id: RunId,
    /// Canonical tool name.
    pub name: String,
    /// Short user-facing title.
    pub title: String,
    /// Semantic category.
    pub kind: ToolCallKind,
    /// Current lifecycle status.
    pub status: ToolCallStatus,
    /// Redacted structured arguments, when safe to expose.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<Value>,
    /// Safe summary when raw arguments are unavailable or inappropriate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Redacted structured output, when available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<Value>,
    /// Optional affected source locations.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub locations: Vec<ToolLocation>,
    /// Optional safe unified diff or change summary.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff: Option<String>,
    /// Safe failure explanation for failed or unknown outcomes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Lifecycle of one stable plan entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanEntryStatus {
    /// Work has not started.
    Pending,
    /// Work is active.
    InProgress,
    /// Work completed successfully.
    Completed,
    /// Work failed.
    Failed,
    /// Work was cancelled.
    Cancelled,
}

/// One stable entry in a user-visible orchestration plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanEntryView {
    /// Stable entry identity.
    pub entry_id: PlanEntryId,
    /// Short task title.
    pub title: String,
    /// Optional longer description.
    pub description: Option<String>,
    /// Current lifecycle state.
    pub status: PlanEntryStatus,
    /// Runs currently attributed to this entry.
    pub run_ids: Vec<RunId>,
}

/// Lifecycle of a real pending approval request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalStatus {
    /// Awaiting an authorized decision.
    Pending,
    /// Approved for one execution.
    Approved,
    /// Denied.
    Denied,
    /// Expired before a decision.
    Expired,
    /// Cancelled with the parent turn.
    Cancelled,
}

/// User-visible approval request carrying real domain identities.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalView {
    /// Stable approval-request identity.
    pub approval_id: ApprovalId,
    /// Real effect intent awaiting authorization.
    pub effect_id: EffectId,
    /// Parent run.
    pub run_id: RunId,
    /// Optional linked tool call.
    pub tool_call_id: Option<ToolCallId>,
    /// Short exact-operation title.
    pub title: String,
    /// Safe operation/target explanation.
    pub description: String,
    /// Current lifecycle state.
    pub status: ApprovalStatus,
    /// Policy or grant explanation, when available.
    pub policy_reason: Option<String>,
    /// Decision deadline, when present.
    pub expires_at: Option<DateTime<Utc>>,
}

/// Accumulated interaction-turn usage suitable for incremental projection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct UsageView {
    /// Input tokens consumed so far.
    pub input_tokens: u64,
    /// Output tokens consumed so far.
    pub output_tokens: u64,
    /// Prompt-cache read tokens.
    pub cache_read_tokens: u64,
    /// Prompt-cache write tokens.
    pub cache_write_tokens: u64,
    /// Accumulated USD cost, when pricing is known.
    pub cost_usd: Option<f64>,
    /// Remaining turn token budget, when bounded.
    pub remaining_tokens: Option<u64>,
    /// Remaining turn cost budget, when bounded.
    pub remaining_cost_usd: Option<f64>,
}

impl UsageView {
    /// Saturating total input plus output tokens.
    pub const fn total_tokens(&self) -> u64 {
        self.input_tokens.saturating_add(self.output_tokens)
    }
}

/// Durable result of a completed interaction turn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TurnResult {
    /// Accumulated assistant text.
    pub text: String,
    /// Linked runs in deterministic target order.
    pub run_ids: Vec<RunId>,
    /// Final usage totals.
    pub usage: UsageView,
}

/// Stable event vocabulary consumed by every interactive surface.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InteractionEvent {
    /// A durable turn and its run correlations now exist.
    TurnStarted {
        /// Resolved target for this turn.
        target: InteractionTarget,
        /// Linked runs and their roles.
        runs: Vec<(RunId, RunRole)>,
    },
    /// Incremental assistant-visible text.
    AgentMessageDelta {
        /// Producing run.
        run_id: RunId,
        /// Text delta.
        text: String,
    },
    /// Incremental reasoning text, only when policy permits exposure.
    ThoughtDelta {
        /// Producing run.
        run_id: RunId,
        /// Safe thought delta.
        text: String,
    },
    /// First complete projection of a tool call.
    ToolCallStarted {
        /// Tool-call projection.
        call: ToolCallView,
    },
    /// Replacement projection for an existing tool call.
    ToolCallUpdated {
        /// Tool-call projection with the same stable identity.
        call: ToolCallView,
    },
    /// Replacement projection of the current plan.
    PlanUpdated {
        /// Ordered plan entries.
        entries: Vec<PlanEntryView>,
    },
    /// A real effect is awaiting approval.
    ApprovalRequested {
        /// Approval projection.
        request: ApprovalView,
    },
    /// A pending approval reached a terminal decision.
    ApprovalResolved {
        /// Stable approval identity.
        approval_id: ApprovalId,
        /// Recorded decision.
        decision: ApprovalDecision,
    },
    /// Replacement projection of accumulated usage.
    UsageUpdated {
        /// Current totals.
        usage: UsageView,
    },
    /// A linked run changed durable lifecycle state.
    RunStateChanged {
        /// Linked run.
        run_id: RunId,
        /// New durable state.
        state: RunState,
    },
    /// Exactly-once successful terminal event.
    TurnCompleted {
        /// Durable turn result.
        result: TurnResult,
    },
    /// Exactly-once failed terminal event.
    TurnFailed {
        /// Typed safe error.
        error: InteractionError,
    },
    /// Exactly-once cancelled terminal event.
    TurnCancelled {
        /// Safe cancellation reason, when present.
        reason: Option<String>,
    },
    /// Exactly-once timeout terminal event.
    TurnTimedOut,
}

impl InteractionEvent {
    /// Whether this event terminates the interaction turn.
    pub const fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::TurnCompleted { .. }
                | Self::TurnFailed { .. }
                | Self::TurnCancelled { .. }
                | Self::TurnTimedOut
        )
    }
}

/// Durable event envelope with replay and correlation fields.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InteractionEventEnvelope {
    /// Stable event identity.
    pub event_id: InteractionEventId,
    /// Parent interaction.
    pub conversation_id: ConversationId,
    /// Parent interaction turn.
    pub turn_id: InteractionTurnId,
    /// Strictly increasing durable sequence within the interaction.
    pub sequence: u64,
    /// Durable event timestamp.
    pub timestamp: DateTime<Utc>,
    /// Structured event payload.
    pub event: InteractionEvent,
}

impl InteractionEventEnvelope {
    /// Construct an envelope using a new event ID and the current time.
    pub fn new(
        conversation_id: ConversationId,
        turn_id: InteractionTurnId,
        sequence: u64,
        event: InteractionEvent,
    ) -> Result<Self, InteractionError> {
        if sequence == 0 {
            return Err(InteractionError::invalid_request(
                "interaction event sequence must start at one",
            ));
        }
        Ok(Self {
            event_id: InteractionEventId::new(),
            conversation_id,
            turn_id,
            sequence,
            timestamp: Utc::now(),
            event,
        })
    }
}

#[cfg(test)]
mod tests {
    use polkagent_core::ids::{AgentId, EffectId};

    use super::*;

    #[test]
    fn terminal_classification_is_exact() {
        let running = InteractionEvent::UsageUpdated {
            usage: UsageView::default(),
        };
        let completed = InteractionEvent::TurnCompleted {
            result: TurnResult {
                text: "done".to_owned(),
                run_ids: Vec::new(),
                usage: UsageView::default(),
            },
        };
        assert!(!running.is_terminal());
        assert!(completed.is_terminal());
        assert!(InteractionEvent::TurnTimedOut.is_terminal());
    }

    #[test]
    fn envelope_rejects_zero_sequence() {
        let result = InteractionEventEnvelope::new(
            ConversationId::new(),
            InteractionTurnId::new(),
            0,
            InteractionEvent::TurnTimedOut,
        );
        assert!(result.is_err());
    }

    #[test]
    fn tool_updates_keep_stable_identity() {
        let id = ToolCallId::new();
        let run_id = RunId::new();
        let started = ToolCallView {
            call_id: id,
            run_id,
            name: "workspace.read".to_owned(),
            title: "Read config".to_owned(),
            kind: ToolCallKind::Read,
            status: ToolCallStatus::InProgress,
            arguments: None,
            summary: Some("config.toml".to_owned()),
            output: None,
            locations: Vec::new(),
            diff: None,
            error: None,
        };
        let mut updated = started.clone();
        updated.status = ToolCallStatus::Succeeded;
        assert_eq!(started.call_id, updated.call_id);
        assert!(!started.status.is_terminal());
        assert!(updated.status.is_terminal());
    }

    #[test]
    fn approval_carries_real_effect_and_request_ids() {
        let approval_id = ApprovalId::new();
        let effect_id = EffectId::new();
        let view = ApprovalView {
            approval_id,
            effect_id,
            run_id: RunId::new(),
            tool_call_id: Some(ToolCallId::new()),
            title: "Submit referendum vote".to_owned(),
            description: "Vote aye on referendum 42".to_owned(),
            status: ApprovalStatus::Pending,
            policy_reason: Some("chain writes require approval".to_owned()),
            expires_at: None,
        };
        assert_eq!(view.approval_id, approval_id);
        assert_eq!(view.effect_id, effect_id);
    }

    #[test]
    fn event_json_round_trip_keeps_tag_and_ids() {
        let event = InteractionEventEnvelope::new(
            ConversationId::new(),
            InteractionTurnId::new(),
            1,
            InteractionEvent::TurnStarted {
                target: InteractionTarget::Agent(AgentId::new()),
                runs: vec![(RunId::new(), RunRole::Primary)],
            },
        )
        .expect("valid envelope");
        let value = serde_json::to_value(&event).expect("serialize event");
        assert_eq!(value["event"]["type"], "turn_started");
        let decoded: InteractionEventEnvelope =
            serde_json::from_value(value).expect("deserialize event");
        assert_eq!(decoded, event);
    }

    #[test]
    fn usage_total_saturates() {
        let usage = UsageView {
            input_tokens: u64::MAX,
            output_tokens: 1,
            ..UsageView::default()
        };
        assert_eq!(usage.total_tokens(), u64::MAX);
    }
}
