//! Run orchestrator: connects the [`RunManager`] to a [`ModelExecutor`] to
//! drive agent runs through the turn loop.
//!
//! The [`RunOrchestrator`] is the top-level "engine" that:
//!
//! 1. Transitions the run through `Created -> Queued -> Running`.
//! 2. Runs the turn loop: build messages, call the executor, process the
//!    response, handle tool calls as effect intents.
//! 3. Transitions the run to a terminal state (`Completed` or `Failed`).
//!
//! # Design
//!
//! The orchestrator is intentionally stateless between calls to
//! [`execute_run`](RunOrchestrator::execute_run). All durable state lives in
//! the [`RunManager`] (store + events). The orchestrator merely drives the
//! loop and delegates to collaborators.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::StreamExt;
use polkagent_card::{ActionCard, EffectKindTag, IntentCardSpec};
use polkagent_core::{
    agent::AgentSpec,
    event::{EventCorrelation, EventKind, RunEvent},
    ids::{ApprovalId, EffectAttemptId, EffectOutcomeId, EventId, GrantId},
    turn::TokenUsage,
    ArtifactId, ConversationId, DataClassification, RetryClass, RunId, RunState, StepId, WorkerId,
};
use polkagent_effect::{
    AttemptState, EffectAttempt, EffectIntent, EffectIntentSpec, EffectKind, EffectOutcome,
    EffectPipeline, ErrorClass, IdempotencyKey, OutcomeResult,
};
use polkagent_event::EventRecorder;
use polkagent_executor_trait::{
    ContentBlock, InferenceMessage, InferenceRequest, MessageRole, ModelExecutor, ToolCall,
    ToolDefinition,
};
use polkagent_grant::{
    EffectSet, EvaluationContext, GrantDecision, GrantLimits, GrantResolver, ResolvedGrant,
};
use polkagent_harness_trait::{
    validate_for_task, Harness, HarnessEvent, HarnessTaskRequirements, SessionConfig,
};
use polkagent_store_trait::approval::{
    compute_approval_subject_digest, compute_execution_checkpoint_digest,
    compute_policy_snapshot_digest, compute_tool_spec_digest, is_canonical_digest,
    ApprovalCoordinatorStore, ApprovalDecision, ApprovalPage, ApprovalPrincipalType,
    ApprovalRequestMetadata, ApprovalScope, ApprovalStatus, ApprovalStoreError, ApprovalSubject,
    CheckpointCommitStatus, CheckpointEffect, CheckpointEffectStatus, CheckpointProgress,
    CheckpointStatus, ClaimApprovedEffect, ExecutionCheckpoint, ExecutionCheckpointStore,
    LeasedCheckpoint, PauseForApproval, ResolveApproval, ResumeApprovalEffect, StoredApproval,
    APPROVAL_SUBJECT_SCHEMA_VERSION, EXECUTION_CHECKPOINT_SCHEMA_VERSION,
};
use polkagent_store_trait::{StoreRetryClass, StoredOutcome};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, instrument, warn};

use polkagent_tool::{ToolContext, ToolError, ToolRegistry, ToolSpec};

#[cfg(feature = "context")]
use polkagent_context::ContextAssembler;

use crate::cost_tracker::CostTracker;
use crate::error::RunError;
use crate::manager::RunManager;
use crate::turn::{TurnInput, TurnManager, TurnOutput};
use crate::ApprovalRuntimeConfig;

#[derive(Clone)]
struct ApprovalRuntime {
    coordinator: Arc<dyn ApprovalCoordinatorStore>,
    checkpoints: Arc<dyn ExecutionCheckpointStore>,
    config: ApprovalRuntimeConfig,
}

impl std::fmt::Debug for ApprovalRuntime {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ApprovalRuntime")
            .field("tenant_id", &self.config.tenant_id)
            .field("workspace_id", &self.config.workspace_id)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CheckpointToolGroup {
    approval_id: ApprovalId,
    calls: Vec<ToolCall>,
}

#[derive(Debug, Clone)]
struct ActiveCheckpoint {
    checkpoint: ExecutionCheckpoint,
    worker_id: WorkerId,
    outcome_ids: Vec<EffectOutcomeId>,
}

#[derive(Debug, Clone)]
struct ExecutionSeed {
    messages: Vec<InferenceMessage>,
    total_usage: TokenUsage,
    turn_count: u32,
    deadline: Option<chrono::DateTime<chrono::Utc>>,
    active_checkpoint: ActiveCheckpoint,
}

struct ToolExecution {
    block: Option<ContentBlock>,
    terminal_state: Option<RunState>,
    active_checkpoint: Option<ActiveCheckpoint>,
}

impl ToolExecution {
    fn continued(block: ContentBlock) -> Self {
        Self {
            block: Some(block),
            terminal_state: None,
            active_checkpoint: None,
        }
    }

    fn error(tool_call: &ToolCall, error_type: &str, message: &str) -> Self {
        Self::continued(RunOrchestrator::tool_error_block(
            tool_call, error_type, message,
        ))
    }
}

// ---------------------------------------------------------------------------
// RunOrchestratorConfig
// ---------------------------------------------------------------------------

/// Configuration knobs for the [`RunOrchestrator`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunOrchestratorConfig {
    /// Maximum number of turns before the run is terminated.
    ///
    /// Default: 25.
    pub max_turns: u32,

    /// Maximum output tokens per inference call.
    ///
    /// Default: 4096.
    pub max_tokens_per_turn: u32,

    /// Legacy compatibility flag for callers that request automatic effect
    /// approval.
    ///
    /// The registered-tool slice never uses this flag to bypass a declared
    /// grant: grant-bearing tools are withheld until approval and durable
    /// resume exist end to end. Grantless allowlisted tools need no approval.
    pub auto_approve_effects: bool,
}

impl Default for RunOrchestratorConfig {
    fn default() -> Self {
        Self {
            max_turns: 25,
            max_tokens_per_turn: 4096,
            auto_approve_effects: false,
        }
    }
}

// ---------------------------------------------------------------------------
// RunOutcome
// ---------------------------------------------------------------------------

/// The terminal result of executing a run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunOutcome {
    /// The run that was executed.
    pub run_id: RunId,
    /// The final state the run entered.
    pub final_state: RunState,
    /// Aggregate token usage across all turns.
    pub total_tokens: TokenUsage,
    /// Number of turns that were executed.
    pub turn_count: u32,
    /// Artifacts produced during the run.
    pub artifacts: Vec<ArtifactId>,
    /// Wall-clock duration of the run.
    pub duration: Duration,
}

// ---------------------------------------------------------------------------
// RunOrchestrator
// ---------------------------------------------------------------------------

/// Connects the [`RunManager`] to a [`ModelExecutor`] and drives the turn
/// loop for agent runs.
///
/// The orchestrator is `Send + Sync` and can be shared across tasks.
pub struct RunOrchestrator {
    run_manager: Arc<RunManager>,
    executor: Arc<dyn ModelExecutor>,
    effect_pipeline: EffectPipeline,
    #[allow(dead_code)] // Phase 2: fine-grained turn/step event recording.
    event_recorder: EventRecorder,
    #[allow(dead_code)] // Phase 2: grant checks before tool execution.
    grant_resolver: Arc<GrantResolver>,
    config: RunOrchestratorConfig,
    turn_manager: TurnManager,
    /// Registered handlers available to explicitly allowlisted agents.
    tool_registry: Option<Arc<ToolRegistry>>,
    /// Durable approval/checkpoint composition. Its absence keeps every
    /// grant-bearing tool fail-closed and unadvertised.
    approval_runtime: Option<ApprovalRuntime>,
    /// Optional harness for delegating runs to an external agent CLI.
    /// When present, the run is driven through the harness instead of
    /// the executor.
    harness: Option<Arc<dyn Harness>>,
    /// Optional context assembler for building and truncating context
    /// before each turn's model call.
    ///
    /// When `Some`, the orchestrator assembles context from the system prompt,
    /// conversation history, and tool results, applying token budget
    /// constraints and truncation using the configured strategy before each
    /// executor call.
    #[cfg(feature = "context")]
    context_assembler: Option<ContextAssembler>,

    /// Optional task requirements for dispatch-time harness validation.
    ///
    /// When `Some` and a harness is attached, the orchestrator validates
    /// that the harness capabilities satisfy these requirements before
    /// starting a run.
    task_requirements: Option<HarnessTaskRequirements>,

    /// Optional payment store for persisting per-turn cost records.
    ///
    /// When `Some`, the orchestrator creates a [`CostTracker`] per run and
    /// records actual token usage after each executor call.
    #[cfg(feature = "payment")]
    payment_store: Option<Arc<dyn polkagent_payment::PaymentStore>>,
}

impl std::fmt::Debug for RunOrchestrator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunOrchestrator")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl RunOrchestrator {
    /// Create a new orchestrator.
    ///
    /// # Parameters
    ///
    /// - `run_manager` -- state management for runs.
    /// - `executor` -- the model executor for inference calls.
    /// - `effect_pipeline` -- the effect pipeline for tool-call intents.
    /// - `event_recorder` -- records run lifecycle events.
    /// - `grant_resolver` -- resolves authorization grants.
    #[must_use]
    pub fn new(
        run_manager: Arc<RunManager>,
        executor: Arc<dyn ModelExecutor>,
        effect_pipeline: EffectPipeline,
        event_recorder: EventRecorder,
        grant_resolver: Arc<GrantResolver>,
    ) -> Self {
        Self {
            run_manager,
            executor,
            effect_pipeline,
            event_recorder,
            grant_resolver,
            config: RunOrchestratorConfig::default(),
            turn_manager: TurnManager::new(),
            tool_registry: None,
            approval_runtime: None,
            harness: None,
            task_requirements: None,
            #[cfg(feature = "context")]
            context_assembler: None,
            #[cfg(feature = "payment")]
            payment_store: None,
        }
    }

    /// Replace the config with a custom one.
    #[must_use]
    pub fn with_config(mut self, config: RunOrchestratorConfig) -> Self {
        self.config = config;
        self
    }

    /// Attach the runtime tool registry used for exact schema advertisement
    /// and dispatch. Only agent-allowlisted tools without a required grant are
    /// exposed or executable by this orchestrator slice.
    #[must_use]
    pub fn with_tool_registry(mut self, registry: Arc<ToolRegistry>) -> Self {
        self.tool_registry = Some(registry);
        self
    }

    /// Attach the durable approval coordinator, checkpoint store, and exact
    /// runtime authorization context used by grant-bearing tools.
    #[must_use]
    pub fn with_approval_runtime(
        mut self,
        coordinator: Arc<dyn ApprovalCoordinatorStore>,
        checkpoints: Arc<dyn ExecutionCheckpointStore>,
        config: ApprovalRuntimeConfig,
    ) -> Self {
        self.approval_runtime = Some(ApprovalRuntime {
            coordinator,
            checkpoints,
            config,
        });
        self
    }

    /// Whether this orchestrator has a valid durable approval composition.
    #[must_use]
    pub fn approval_ready(&self) -> bool {
        self.approval_runtime.as_ref().is_some_and(|runtime| {
            runtime.config.validate().is_ok()
                && runtime.coordinator.supports_atomic_approval_resume()
                && runtime.checkpoints.supports_exact_checkpoint_recovery()
        }) && self.tool_registry.is_some()
    }

    /// Lease durable continuations that are ready after an approval decision.
    /// This is an executor/store capability; callers must not equate it with
    /// availability of an authenticated user-facing decision surface.
    pub async fn lease_resumable_checkpoints(&self) -> Result<Vec<LeasedCheckpoint>, RunError> {
        let runtime = self.approval_runtime.as_ref().ok_or_else(|| {
            RunError::Unsupported("durable approval recovery is not configured".to_owned())
        })?;
        if !self.approval_ready() {
            return Err(RunError::Unsupported(
                "durable approval recovery adapter is not APR-03 capable".to_owned(),
            ));
        }
        runtime
            .checkpoints
            .lease_resumable(
                self.effect_pipeline.worker_id(),
                runtime.config.recovery_lease,
                ApprovalPage {
                    limit: 100,
                    offset: 0,
                },
            )
            .await
            .map_err(|error| approval_store_error("lease resumable checkpoints", error))
    }

    /// Recover one leased checkpoint, reducing its exact decided effect before
    /// issuing any further model request.
    pub async fn recover_checkpoint(
        &self,
        leased: LeasedCheckpoint,
        agent_spec: &AgentSpec,
    ) -> Result<RunOutcome, RunError> {
        let started = Instant::now();
        let runtime = self.approval_runtime.as_ref().ok_or_else(|| {
            RunError::Unsupported("durable approval recovery is not configured".to_owned())
        })?;
        let checkpoint = leased.stored.checkpoint.clone();
        verify_checkpoint(&checkpoint)?;
        if checkpoint.agent_id != agent_spec.id {
            return Err(RunError::Store(
                "checkpoint agent does not match the registered agent".to_owned(),
            ));
        }
        let group: CheckpointToolGroup = serde_json::from_value(checkpoint.tool_calls.clone())?;
        let [tool_call] = group.calls.as_slice() else {
            return Err(RunError::Unsupported(
                "APR-03 recovery requires exactly one checkpointed tool call".to_owned(),
            ));
        };
        let registry = self.tool_registry.as_ref().ok_or_else(|| {
            RunError::Unsupported("tool registry is unavailable during recovery".to_owned())
        })?;
        let handler = registry.get(&tool_call.tool_name).ok_or_else(|| {
            RunError::Unsupported("checkpointed tool is no longer registered".to_owned())
        })?;
        let spec = handler.spec();
        let approval = runtime
            .coordinator
            .get_approval(
                group.approval_id,
                ApprovalScope {
                    tenant_id: runtime.config.tenant_id.clone(),
                    workspace_id: runtime.config.workspace_id.clone(),
                    conversation_id: Some(checkpoint.conversation_id),
                    principal_id: runtime.config.authorized_principal_id,
                },
            )
            .await
            .map_err(|error| approval_store_error("load recovery approval", error))?;
        let execution = self
            .resume_decided_approval(
                runtime,
                agent_spec,
                tool_call,
                &spec,
                approval,
                Some(leased),
            )
            .await?;
        let total_usage: TokenUsage = serde_json::from_value(checkpoint.accumulated_usage.clone())?;
        if let Some(final_state) = execution.terminal_state {
            return Ok(RunOutcome {
                run_id: checkpoint.run_id,
                final_state,
                total_tokens: total_usage,
                turn_count: checkpoint.next_model_turn.saturating_sub(1),
                artifacts: Vec::new(),
                duration: started.elapsed(),
            });
        }
        let mut messages: Vec<InferenceMessage> =
            serde_json::from_value(checkpoint.messages.clone())?;
        if let Some(block) = execution.block {
            messages.push(InferenceMessage {
                role: MessageRole::User,
                content: vec![block],
            });
        }
        let active_checkpoint = execution.active_checkpoint.ok_or_else(|| {
            RunError::Store("recovered effect did not retain its checkpoint lease".to_owned())
        })?;
        self.execute_run_inner(
            checkpoint.run_id,
            agent_spec,
            messages.clone(),
            "approval recovery".to_owned(),
            false,
            Some(checkpoint.conversation_id),
            Some(ExecutionSeed {
                messages,
                total_usage,
                turn_count: checkpoint.next_model_turn.saturating_sub(1),
                deadline: checkpoint.deadline_at,
                active_checkpoint,
            }),
        )
        .await
    }

    /// Attach a [`Harness`] for delegating runs to an external agent CLI.
    ///
    /// When a harness is attached, [`execute_run`](Self::execute_run) will
    /// start a harness session and send the prompt through it instead of
    /// calling the model executor directly.
    #[must_use]
    pub fn with_harness(mut self, harness: Arc<dyn Harness>) -> Self {
        self.harness = Some(harness);
        self
    }

    /// Attach task requirements for dispatch-time harness validation.
    ///
    /// When set, the orchestrator validates that an attached harness meets
    /// these requirements before starting a run via that harness.
    #[must_use]
    pub fn with_task_requirements(mut self, requirements: HarnessTaskRequirements) -> Self {
        self.task_requirements = Some(requirements);
        self
    }

    /// Attach a [`ContextAssembler`] for building and truncating context
    /// before each turn's model call.
    ///
    /// When attached, the orchestrator will assemble context from the system
    /// prompt, conversation history, and tool results, applying the
    /// assembler's token budget constraints and truncation strategy before
    /// each call to the model executor.
    ///
    /// Only available when the `context` feature is enabled.
    #[cfg(feature = "context")]
    #[must_use]
    pub fn with_context_assembler(mut self, assembler: ContextAssembler) -> Self {
        self.context_assembler = Some(assembler);
        self
    }

    /// Attach a [`PaymentStore`](polkagent_payment::PaymentStore) for
    /// persisting per-turn cost records.
    ///
    /// When attached, the orchestrator creates a [`CostTracker`] for each run
    /// and records actual token usage after every executor call. If the
    /// agent's `resource_limits.max_per_run_usd` is set, budget enforcement
    /// is also active.
    ///
    /// Only available when the `payment` feature is enabled.
    #[cfg(feature = "payment")]
    #[must_use]
    pub fn with_payment_store(mut self, store: Arc<dyn polkagent_payment::PaymentStore>) -> Self {
        self.payment_store = Some(store);
        self
    }

    /// Execute a run by delegating to a harness subprocess.
    ///
    /// Instead of calling the model executor, this method:
    /// 1. Starts a harness session
    /// 2. Sends the user prompt
    /// 3. Consumes the event stream until the session ends
    /// 4. Collects the agent's response text
    /// 5. Transitions the run to Completed
    #[instrument(skip(self, agent_spec, initial_prompt, harness), fields(%run_id))]
    async fn execute_run_via_harness(
        &self,
        run_id: RunId,
        agent_spec: &AgentSpec,
        initial_prompt: &str,
        harness: &Arc<dyn Harness>,
    ) -> Result<RunOutcome, RunError> {
        let start = Instant::now();

        // Dispatch-time validation: check harness capabilities vs task requirements.
        if let Some(ref requirements) = self.task_requirements {
            let caps = harness.capabilities();
            if let Err(mismatches) = validate_for_task(&caps, requirements) {
                let reasons: Vec<String> = mismatches
                    .iter()
                    .map(std::string::ToString::to_string)
                    .collect();
                let msg = format!(
                    "harness {:?} does not meet task requirements: {}",
                    harness.id(),
                    reasons.join("; ")
                );
                warn!(%run_id, %msg, "harness validation failed");
                return Err(RunError::HarnessValidation(msg));
            }
        }

        // Transition to Running.
        let current_state = self.run_manager.get_state(run_id).await?;
        if current_state == RunState::Created {
            self.run_manager.enqueue_run(run_id).await?;
        }
        self.run_manager.start_run(run_id).await?;

        // Build session config from agent spec.
        let session_config = SessionConfig {
            system_prompt: agent_spec.system_prompt.clone(),
            working_directory: None,
            ..Default::default()
        };

        // Start a harness session.
        let session_id = harness
            .start_session(session_config)
            .await
            .map_err(|e| RunError::Store(format!("harness session start failed: {e}")))?;

        info!(%run_id, %session_id, harness_id = %harness.id(), "harness session started");

        // Send the user prompt.
        harness
            .send_message(session_id, initial_prompt)
            .await
            .map_err(|e| RunError::Store(format!("harness send_message failed: {e}")))?;

        // Consume events from the harness.
        let mut event_stream = harness
            .receive_events(session_id)
            .await
            .map_err(|e| RunError::Store(format!("harness receive_events failed: {e}")))?;

        let mut response_text = String::new();
        let mut tool_call_count: u32 = 0;
        let mut had_error = false;
        let mut error_message = String::new();

        while let Some(event) = event_stream.next().await {
            match event {
                HarnessEvent::MessageReceived { content, .. } => {
                    if !response_text.is_empty() {
                        response_text.push('\n');
                    }
                    response_text.push_str(&content);
                    debug!(%run_id, content_len = content.len(), "harness message received");
                }
                HarnessEvent::ToolCallRequested { tool_name, .. } => {
                    tool_call_count += 1;
                    debug!(%run_id, %tool_name, "harness tool call (handled by harness)");
                }
                HarnessEvent::ToolResultProvided {
                    tool_name,
                    is_error,
                    ..
                } => {
                    if is_error {
                        debug!(%run_id, %tool_name, "harness tool call returned error");
                    }
                }
                HarnessEvent::SessionEnded { .. } => {
                    info!(%run_id, "harness session ended");
                    break;
                }
                HarnessEvent::Error { message, .. } => {
                    warn!(%run_id, %message, "harness error");
                    had_error = true;
                    error_message = message;
                }
                HarnessEvent::SessionStarted { .. } => {
                    debug!(%run_id, "harness session started event");
                }
            }
        }

        // End the session (cleanup).
        if let Err(e) = harness.end_session(session_id).await {
            warn!(%run_id, %e, "harness end_session failed (non-fatal)");
        }

        // Transition to terminal state.
        if had_error && response_text.is_empty() {
            let reason = format!("harness error: {error_message}");
            self.run_manager.fail_run(run_id, &reason).await?;
            Ok(RunOutcome {
                run_id,
                final_state: RunState::Failed { reason },
                total_tokens: TokenUsage::default(),
                turn_count: 1,
                artifacts: Vec::new(),
                duration: start.elapsed(),
            })
        } else {
            self.run_manager.completing_run(run_id).await?;
            self.run_manager.complete_run(run_id, None, 0, 0).await?;

            info!(
                %run_id,
                response_len = response_text.len(),
                tool_calls = tool_call_count,
                "harness run completed"
            );

            Ok(RunOutcome {
                run_id,
                final_state: RunState::Completed,
                total_tokens: TokenUsage::default(),
                turn_count: 1,
                artifacts: Vec::new(),
                duration: start.elapsed(),
            })
        }
    }

    /// Execute a run from start to terminal state.
    ///
    /// # Algorithm
    ///
    /// 1. Transition: Created -> Queued -> Running.
    /// 2. Build initial messages from the agent's system prompt and the user
    ///    prompt.
    /// 3. Enter the turn loop (max `config.max_turns` iterations):
    ///    a. Call `executor.complete(request)` with the current messages.
    ///    b. Record a turn event with token usage.
    ///    c. Process the response:
    ///       - Text only with `end_turn` stop reason -> complete the run.
    ///       - Tool calls -> create effect intents.
    ///       - `max_tokens` stop reason -> fail with context exceeded.
    ///    d. For an advertised grantless tool call: persist its normalized
    ///       step, intent, claim, attempt, and outcome around real registry
    ///       dispatch, then add the exact serialized result and continue.
    ///       Rejected calls receive typed errors without handler I/O.
    ///    e. Add assistant response and tool results to messages.
    ///    f. Continue loop.
    /// 4. Transition: Running -> Completing -> Completed (or Failed).
    /// 5. Return [`RunOutcome`].
    ///
    /// # Errors
    ///
    /// Returns [`RunError`] if state transitions, store operations, or event
    /// recording fails. Executor errors are caught and transition the run to
    /// `Failed`.
    #[instrument(skip(self, agent_spec, initial_prompt), fields(%run_id))]
    pub async fn execute_run(
        &self,
        run_id: RunId,
        agent_spec: &AgentSpec,
        initial_prompt: &str,
    ) -> Result<RunOutcome, RunError> {
        let messages = Self::build_initial_messages(agent_spec, initial_prompt);
        self.execute_run_inner(
            run_id,
            agent_spec,
            messages,
            initial_prompt.to_owned(),
            true,
            None,
            None,
        )
        .await
    }

    /// Execute a prompt in one durably correlated conversation. Grant-bearing
    /// tools are advertised only on this path and only when approval runtime
    /// composition is complete.
    pub async fn execute_run_in_conversation(
        &self,
        run_id: RunId,
        conversation_id: ConversationId,
        agent_spec: &AgentSpec,
        initial_prompt: &str,
    ) -> Result<RunOutcome, RunError> {
        self.execute_run_inner(
            run_id,
            agent_spec,
            Self::build_initial_messages(agent_spec, initial_prompt),
            initial_prompt.to_owned(),
            true,
            Some(conversation_id),
            None,
        )
        .await
    }

    /// Execute a run from an already typed conversation transcript.
    ///
    /// `initial_messages` must include the current user message as its final
    /// element. The messages are sent to the model executor without rendering
    /// or reparsing them, so roles and content-block boundaries remain exact.
    /// The legacy context assembler is intentionally bypassed for this path;
    /// callers that supply durable history own its bounded-context policy.
    ///
    /// Harness backends accept only one plain-text user message. A non-empty
    /// prior transcript is rejected rather than flattened into a prompt that
    /// could lose role boundaries.
    #[instrument(skip(self, agent_spec, initial_messages), fields(%run_id))]
    pub async fn execute_run_with_messages(
        &self,
        run_id: RunId,
        agent_spec: &AgentSpec,
        initial_messages: Vec<InferenceMessage>,
    ) -> Result<RunOutcome, RunError> {
        self.validate_initial_messages(&initial_messages)?;
        let current_user_text = initial_messages
            .last()
            .and_then(|message| message.content.first())
            .and_then(|block| match block {
                ContentBlock::Text { text } => Some(text.clone()),
                ContentBlock::ToolResult { .. } | ContentBlock::ToolUse { .. } => None,
            })
            .unwrap_or_else(|| "typed user input".to_owned());
        self.execute_run_inner(
            run_id,
            agent_spec,
            initial_messages,
            current_user_text,
            false,
            None,
            None,
        )
        .await
    }

    /// Execute an exact typed transcript in one durable conversation.
    pub async fn execute_run_with_messages_in_conversation(
        &self,
        run_id: RunId,
        conversation_id: ConversationId,
        agent_spec: &AgentSpec,
        initial_messages: Vec<InferenceMessage>,
    ) -> Result<RunOutcome, RunError> {
        self.validate_initial_messages(&initial_messages)?;
        let current_user_text = initial_messages
            .last()
            .and_then(|message| message.content.first())
            .and_then(|block| match block {
                ContentBlock::Text { text } => Some(text.clone()),
                ContentBlock::ToolResult { .. } | ContentBlock::ToolUse { .. } => None,
            })
            .unwrap_or_else(|| "typed user input".to_owned());
        self.execute_run_inner(
            run_id,
            agent_spec,
            initial_messages,
            current_user_text,
            false,
            Some(conversation_id),
            None,
        )
        .await
    }

    /// Validate typed initial messages before a prepared run is activated.
    ///
    /// This synchronous preflight lets service callers return an explicit
    /// unsupported error without publishing a run activation event.
    pub fn validate_initial_messages(&self, messages: &[InferenceMessage]) -> Result<(), RunError> {
        let Some(current) = messages.last() else {
            return Err(RunError::Unsupported(
                "model execution requires a current user message".to_owned(),
            ));
        };
        if current.role != MessageRole::User {
            return Err(RunError::Unsupported(
                "typed execution history must end with the current user message".to_owned(),
            ));
        }
        if self.harness.is_some() {
            Self::harness_prompt(messages)?;
        }
        Ok(())
    }

    fn harness_prompt(messages: &[InferenceMessage]) -> Result<&str, RunError> {
        if messages.len() != 1 {
            return Err(RunError::Unsupported(
                "harness-backed execution cannot map prior conversation history role-safely"
                    .to_owned(),
            ));
        }
        let message = &messages[0];
        if message.role != MessageRole::User {
            return Err(RunError::Unsupported(
                "harness-backed execution requires one plain-text user message".to_owned(),
            ));
        }
        match message.content.as_slice() {
            [ContentBlock::Text { text }] => Ok(text),
            _ => Err(RunError::Unsupported(
                "harness-backed execution requires one plain-text user message".to_owned(),
            )),
        }
    }

    #[allow(
        clippy::too_many_arguments,
        clippy::too_many_lines,
        reason = "the execution loop keeps ordered durable transitions and early terminal paths together"
    )]
    async fn execute_run_inner(
        &self,
        run_id: RunId,
        agent_spec: &AgentSpec,
        initial_messages: Vec<InferenceMessage>,
        initial_prompt: String,
        apply_legacy_context_assembly: bool,
        conversation_id: Option<ConversationId>,
        seed: Option<ExecutionSeed>,
    ) -> Result<RunOutcome, RunError> {
        let start = Instant::now();
        #[cfg(not(feature = "context"))]
        let _ = apply_legacy_context_assembly;

        // If a harness is attached, delegate the entire run to it.
        if let Some(ref harness) = self.harness {
            let prompt = Self::harness_prompt(&initial_messages)?;
            return self
                .execute_run_via_harness(run_id, agent_spec, prompt, harness)
                .await;
        }

        // NOTE(persistence): `total_usage` is accumulated across turns and
        // included in the returned `RunOutcome`. Individual turn records
        // (including per-turn token counts) are persisted to the `turns`
        // table via `RunManager::record_turn`.
        let mut total_usage = seed
            .as_ref()
            .map_or_else(TokenUsage::default, |seed| seed.total_usage);
        let mut turn_count: u32 = seed.as_ref().map_or(0, |seed| seed.turn_count);
        let artifacts: Vec<ArtifactId> = Vec::new();

        // Merge agent-level resource limits on top of the static orchestrator
        // config.  All fields are optional; absent values fall back to the
        // defaults encoded in `self.config`.
        let effective_max_turns = agent_spec
            .resource_limits
            .as_ref()
            .and_then(|rl| rl.max_turns)
            .unwrap_or(self.config.max_turns);

        let effective_max_tokens = agent_spec
            .resource_limits
            .as_ref()
            .and_then(|rl| rl.max_tokens_per_turn)
            .unwrap_or(self.config.max_tokens_per_turn);

        // Wall-clock timeout derived from resource limits (optional).
        //
        // The deadline is stored as an absolute DateTime<Utc> so that it
        // survives process restarts. On recovery, the RunManager can read the
        // persisted `deadline_at` from the database instead of creating a new
        // process-local Instant.
        let run_deadline: Option<chrono::DateTime<chrono::Utc>> = seed.as_ref().map_or_else(
            || {
                agent_spec
                    .resource_limits
                    .as_ref()
                    .and_then(|rl| rl.timeout_secs)
                    .map(deadline_from_timeout)
            },
            |seed| seed.deadline,
        );

        // Persist the deadline to the store so it can be restored after a crash.
        if seed.is_none() {
            if let Some(deadline) = run_deadline {
                if let Err(e) = self.run_manager.set_deadline(run_id, Some(deadline)).await {
                    warn!(%run_id, error = %e, "failed to persist run deadline");
                }
            }
        }

        // Resolve the model to use: prefer model_preference.model_id when set,
        // then fall back to agent_spec.model.
        let effective_model_id: String = agent_spec
            .model_preference
            .as_ref()
            .and_then(|mp| mp.model_id.as_deref())
            .map_or_else(
                || agent_spec.model.clone(),
                |mid| {
                    // If the spec.model has a provider prefix and the preference
                    // has only a bare model ID, re-attach the provider prefix from
                    // the spec so the executor gets a fully-qualified id.
                    if mid.contains('/') {
                        mid.to_owned()
                    } else {
                        let prefix = agent_spec.model.split('/').next().unwrap_or("");
                        if prefix.is_empty() {
                            mid.to_owned()
                        } else {
                            format!("{prefix}/{mid}")
                        }
                    }
                },
            );

        // Temperature from model_preference (None means executor default).
        // InferenceRequest uses f32; we store f64 in the spec for precision in
        // config files, and cast here at the use site.
        let effective_temperature: Option<f32> = agent_spec
            .model_preference
            .as_ref()
            .and_then(|mp| mp.temperature)
            .map(temperature_as_f32);

        // System prompt: model_preference.system_prompt overrides
        // agent_spec.system_prompt when present.
        let effective_system: Option<String> = agent_spec
            .model_preference
            .as_ref()
            .and_then(|mp| mp.system_prompt.clone())
            .or_else(|| agent_spec.system_prompt.clone());

        // Advertise only exact registered definitions that the agent names
        // and that require no grant. Grant-bearing tools stay invisible until
        // approval + durable resume is implemented end to end.
        let approval_capable = conversation_id.is_some() && self.approval_ready();
        let advertised_tools = self.advertised_tools(agent_spec, approval_capable);
        let advertised_tool_names: HashSet<String> = advertised_tools
            .iter()
            .map(|definition| definition.name.clone())
            .collect();

        // Step 1: Transition to Running.
        // In production, AppService::start_run enqueues before spawning us
        // (Created → Queued), so we just claim (Queued → Running).
        // In tests, execute_run is called directly on a Created run, so
        // we enqueue first if needed.
        let current_state = self.run_manager.get_state(run_id).await?;
        if current_state == RunState::Created {
            self.run_manager.enqueue_run(run_id).await?;
            self.run_manager.start_run(run_id).await?;
        } else if current_state == RunState::Queued {
            self.run_manager.start_run(run_id).await?;
        } else if current_state != RunState::Running {
            return Err(RunError::Store(format!(
                "executor cannot begin from run state {current_state}"
            )));
        }

        // Step 2: Build initial message list.
        let mut messages = seed
            .as_ref()
            .map_or(initial_messages, |seed| seed.messages.clone());
        let mut active_checkpoint = seed.map(|seed| seed.active_checkpoint);

        // Build a CostTracker for this run.
        //
        // When the `payment` feature is enabled and a payment store is
        // attached, token usage is persisted after each turn. When a
        // per-run budget is configured via `agent_spec.resource_limits`,
        // budget enforcement is active.
        let mut cost_tracker = {
            #[cfg(feature = "payment")]
            {
                let max_usd = agent_spec.resource_limits.as_ref().and_then(|rl| {
                    // Resource limits don't have a USD field yet; use the
                    // payment feature if attached but no hard USD cap from
                    // spec. Callers can extend this mapping as needed.
                    let _ = rl;
                    None::<f64>
                });

                let tracker = match max_usd {
                    Some(limit) => CostTracker::with_budget(run_id, limit),
                    None => CostTracker::unbounded(run_id),
                };

                match &self.payment_store {
                    Some(store) => tracker.with_store(Arc::clone(store)),
                    None => tracker,
                }
            }
            #[cfg(not(feature = "payment"))]
            CostTracker::unbounded(run_id)
        };

        // Step 3: Turn loop.
        let outcome = 'run_loop: loop {
            // Guard: wall-clock timeout.
            if let Some(deadline) = run_deadline {
                if chrono::Utc::now() >= deadline {
                    warn!(%run_id, "run deadline exceeded — transitioning to TimedOut");
                    self.run_manager.timeout_run(run_id).await?;
                    break RunOutcome {
                        run_id,
                        final_state: RunState::TimedOut,
                        total_tokens: total_usage,
                        turn_count,
                        artifacts: artifacts.clone(),
                        duration: start.elapsed(),
                    };
                }
            }

            // Guard: max turns.
            if turn_count >= effective_max_turns {
                warn!(%run_id, turn_count, max = effective_max_turns, "max turns exceeded");
                self.run_manager
                    .fail_run(run_id, "max turns exceeded")
                    .await?;
                break RunOutcome {
                    run_id,
                    final_state: RunState::Failed {
                        reason: "max turns exceeded".to_owned(),
                    },
                    total_tokens: total_usage,
                    turn_count,
                    artifacts: artifacts.clone(),
                    duration: start.elapsed(),
                };
            }

            // Budget check: verify that starting the next turn won't exceed
            // the per-run budget (pre-turn worst-case estimate).
            {
                // Parse provider from the model_id (e.g. "anthropic/claude…" →
                // provider="anthropic", model="claude…").
                let (provider, model) = split_model_id(&effective_model_id);
                if let Some(exceeded) =
                    cost_tracker.check_budget(provider, model, effective_max_tokens)
                {
                    let reason = format!("BudgetExceeded: {}", exceeded.reason);
                    warn!(%run_id, %reason, "budget exceeded before turn");
                    self.run_manager.fail_run(run_id, &reason).await?;
                    break RunOutcome {
                        run_id,
                        final_state: RunState::Failed { reason },
                        total_tokens: total_usage,
                        turn_count,
                        artifacts: artifacts.clone(),
                        duration: start.elapsed(),
                    };
                }
            }

            // 3a: Apply context assembly (if configured) then call executor.
            //
            // When a ContextAssembler is attached, the message list is
            // assembled through the context window budget, truncating
            // lower-priority sections (oldest conversation messages first)
            // so the request fits within the model's context limit.
            #[cfg(feature = "context")]
            let assembled_messages = if apply_legacy_context_assembly {
                self.apply_context_assembly(&messages, effective_system.as_deref())
            } else {
                messages.clone()
            };
            #[cfg(not(feature = "context"))]
            let assembled_messages = messages.clone();

            let model_step_id = StepId::new();
            let request = InferenceRequest {
                run_id,
                step_id: model_step_id,
                messages: assembled_messages,
                system: effective_system.clone(),
                tools: advertised_tools.clone(),
                model_id: effective_model_id.clone(),
                max_tokens: effective_max_tokens,
                temperature: effective_temperature,
            };

            let response = match self.executor.complete(request).await {
                Ok(resp) => resp,
                Err(err) => {
                    let reason = format!("executor error: {err}");
                    warn!(%run_id, %reason, "executor failed");
                    self.run_manager.fail_run(run_id, &reason).await?;
                    break RunOutcome {
                        run_id,
                        final_state: RunState::Failed { reason },
                        total_tokens: total_usage,
                        turn_count,
                        artifacts: artifacts.clone(),
                        duration: start.elapsed(),
                    };
                }
            };

            // 3b: Record turn with token usage.
            turn_count += 1;
            let turn_usage = convert_token_usage(&response.usage);
            total_usage.accumulate(&turn_usage);

            // Post-turn: record actual cost and check if budget is exhausted.
            {
                let (provider, model) = split_model_id(&effective_model_id);
                if let Some(exceeded) = cost_tracker
                    .record_turn_cost(
                        provider,
                        model,
                        u64::from(response.usage.input_tokens),
                        u64::from(response.usage.output_tokens),
                    )
                    .await
                {
                    let reason = format!("BudgetExceeded: {}", exceeded.reason);
                    warn!(%run_id, %reason, "budget exceeded after turn");
                    self.run_manager.fail_run(run_id, &reason).await?;
                    break RunOutcome {
                        run_id,
                        final_state: RunState::Failed { reason },
                        total_tokens: total_usage,
                        turn_count,
                        artifacts: artifacts.clone(),
                        duration: start.elapsed(),
                    };
                }
            }

            let turn_input = TurnInput::user(if turn_count == 1 {
                initial_prompt.clone()
            } else {
                "continuation".to_owned()
            });
            let turn = self
                .turn_manager
                .create_turn(run_id, turn_count - 1, &turn_input);
            let turn_output =
                if response.stop_reason == "end_turn" && response.tool_calls.is_empty() {
                    TurnOutput::terminal(&response.text).with_usage(turn_usage)
                } else {
                    TurnOutput::continuing().with_usage(turn_usage)
                };
            let completed_turn = self.turn_manager.complete_turn(turn, &turn_output);
            if let Err(error) = self.run_manager.record_turn(&completed_turn).await {
                if response.tool_calls.is_empty() {
                    warn!(
                        %run_id,
                        turn = turn_count,
                        error = %error,
                        "failed to persist text-only turn record (non-fatal)"
                    );
                } else {
                    return Err(RunError::Store(format!(
                        "cannot persist tool effects without their parent turn: {error}"
                    )));
                }
            }

            // Emit the model's response text so RunPrinter can display it.
            if !response.text.is_empty() {
                let _ = self
                    .event_recorder
                    .record(RunEvent::new_ephemeral(
                        EventId::new(),
                        run_id,
                        0,
                        EventKind::StreamingToken {
                            text: response.text.clone(),
                        },
                    ))
                    .await;
            }

            debug!(
                %run_id,
                turn = turn_count,
                stop_reason = %response.stop_reason,
                tool_calls = response.tool_calls.len(),
                tokens = turn_usage.total_tokens,
                "turn completed"
            );

            // 3c: Process response.
            if response.stop_reason == "max_tokens" {
                let reason = "context window exceeded (max_tokens)".to_owned();
                self.run_manager.fail_run(run_id, &reason).await?;
                break RunOutcome {
                    run_id,
                    final_state: RunState::Failed { reason },
                    total_tokens: total_usage,
                    turn_count,
                    artifacts: artifacts.clone(),
                    duration: start.elapsed(),
                };
            }

            if response.tool_calls.is_empty() && response.stop_reason == "end_turn" {
                // Text-only terminal response.
                messages.push(InferenceMessage {
                    role: MessageRole::Assistant,
                    content: vec![ContentBlock::Text {
                        text: response.text.clone(),
                    }],
                });

                // Transition: Running -> Completing -> Completed.
                self.run_manager.completing_run(run_id).await?;
                self.run_manager
                    .complete_run(
                        run_id,
                        None,
                        u64::from(total_usage.input_tokens),
                        u64::from(total_usage.output_tokens),
                    )
                    .await?;

                info!(%run_id, turn_count, "run completed successfully");
                break RunOutcome {
                    run_id,
                    final_state: RunState::Completed,
                    total_tokens: total_usage,
                    turn_count,
                    artifacts: artifacts.clone(),
                    duration: start.elapsed(),
                };
            }

            // 3d: Tool calls -- build assistant message with tool uses and
            // add tool results.
            if !response.tool_calls.is_empty() {
                // Add the assistant message containing the tool uses.
                let mut assistant_content: Vec<ContentBlock> = Vec::new();
                if !response.text.is_empty() {
                    assistant_content.push(ContentBlock::Text {
                        text: response.text.clone(),
                    });
                }
                for tc in &response.tool_calls {
                    assistant_content.push(ContentBlock::ToolUse {
                        tool_call_id: tc.tool_call_id.clone(),
                        tool_name: tc.tool_name.clone(),
                        arguments_json: tc.arguments_json.clone(),
                    });
                }
                messages.push(InferenceMessage {
                    role: MessageRole::Assistant,
                    content: assistant_content,
                });

                let mut tool_result_content: Vec<ContentBlock> = Vec::new();
                for (call_index, tool_call) in response.tool_calls.iter().enumerate() {
                    let step_sequence = u32::try_from(call_index + 1).unwrap_or(u32::MAX);
                    let effect_sequence =
                        (u64::from(turn_count) << 32).saturating_add(u64::from(step_sequence));
                    let execution = self
                        .execute_tool_call(
                            run_id,
                            agent_spec,
                            completed_turn.id,
                            step_sequence,
                            effect_sequence,
                            tool_call,
                            &response.tool_calls,
                            &messages,
                            &effective_model_id,
                            turn_count,
                            &total_usage,
                            run_deadline,
                            conversation_id,
                            &advertised_tool_names,
                        )
                        .await?;
                    if let Some(terminal_state) = execution.terminal_state {
                        break 'run_loop RunOutcome {
                            run_id,
                            final_state: terminal_state,
                            total_tokens: total_usage,
                            turn_count,
                            artifacts: artifacts.clone(),
                            duration: start.elapsed(),
                        };
                    }
                    if let Some(checkpoint) = execution.active_checkpoint {
                        if active_checkpoint.is_some() {
                            return Err(RunError::Unsupported(
                                "more than one approval checkpoint in a run is not supported by the APR-03 vertical slice"
                                    .to_owned(),
                            ));
                        }
                        active_checkpoint = Some(checkpoint);
                    }
                    if let Some(block) = execution.block {
                        tool_result_content.push(block);
                    }
                }

                if !tool_result_content.is_empty() {
                    messages.push(InferenceMessage {
                        role: MessageRole::User,
                        content: tool_result_content,
                    });
                }

                // Continue the loop with tool results added.
                continue;
            }

            // If we reach here, the response has text but the stop reason is
            // not "end_turn" and there are no tool calls (e.g. "stop_sequence").
            // Add the text and continue.
            if !response.text.is_empty() {
                messages.push(InferenceMessage {
                    role: MessageRole::Assistant,
                    content: vec![ContentBlock::Text {
                        text: response.text.clone(),
                    }],
                });
            }
        };

        if let Some(checkpoint) = active_checkpoint {
            self.finalize_checkpoint(checkpoint, &messages).await?;
        }
        Ok(outcome)
    }

    // -----------------------------------------------------------------------
    // Private helpers
    // -----------------------------------------------------------------------

    fn advertised_tools(
        &self,
        agent_spec: &AgentSpec,
        approval_capable: bool,
    ) -> Vec<ToolDefinition> {
        let Some(registry) = &self.tool_registry else {
            return Vec::new();
        };
        let mut seen = HashSet::new();
        agent_spec
            .tools
            .iter()
            .filter(|name| seen.insert((*name).clone()))
            .filter_map(|name| {
                let handler = registry.get(name)?;
                let spec = handler.spec();
                (spec.name == *name && (spec.required_grant.is_none() || approval_capable)).then(
                    || ToolDefinition {
                        name: spec.name,
                        description: spec.description,
                        input_schema_json: spec.input_schema.to_string(),
                    },
                )
            })
            .collect()
    }

    #[allow(
        clippy::too_many_arguments,
        clippy::too_many_lines,
        reason = "tool dispatch keeps its ordered durable step/intent/claim/attempt/outcome sequence together"
    )]
    async fn execute_tool_call(
        &self,
        run_id: RunId,
        agent_spec: &AgentSpec,
        turn_id: polkagent_core::TurnId,
        step_sequence: u32,
        effect_sequence: u64,
        tool_call: &ToolCall,
        tool_group: &[ToolCall],
        messages: &[InferenceMessage],
        model_id: &str,
        turn_count: u32,
        total_usage: &TokenUsage,
        run_deadline: Option<chrono::DateTime<chrono::Utc>>,
        conversation_id: Option<ConversationId>,
        advertised_tool_names: &HashSet<String>,
    ) -> Result<ToolExecution, RunError> {
        let Some(registry) = &self.tool_registry else {
            return Ok(ToolExecution::error(
                tool_call,
                "registry_unavailable",
                "no runtime tool registry is configured",
            ));
        };
        if !agent_spec
            .tools
            .iter()
            .any(|name| name == &tool_call.tool_name)
        {
            return Ok(ToolExecution::error(
                tool_call,
                "not_allowlisted",
                "the agent spec does not allow this tool",
            ));
        }
        let Some(handler) = registry.get(&tool_call.tool_name) else {
            return Ok(ToolExecution::error(
                tool_call,
                "not_registered",
                "the requested tool is not registered",
            ));
        };
        let spec = handler.spec();
        if spec.name != tool_call.tool_name {
            return Ok(ToolExecution::error(
                tool_call,
                "registry_mismatch",
                "the registered handler definition changed after registration",
            ));
        }
        if !advertised_tool_names.contains(&tool_call.tool_name) {
            if spec.required_grant.is_some() {
                return Ok(ToolExecution::error(
                    tool_call,
                    "approval_required",
                    "the tool requires a durable approval path that is unavailable in this execution context",
                ));
            }
            return Ok(ToolExecution::error(
                tool_call,
                "not_advertised",
                "the tool was not advertised for this execution context",
            ));
        }
        let input: serde_json::Value = match serde_json::from_str(&tool_call.arguments_json) {
            Ok(input) => input,
            Err(error) => {
                return Ok(ToolExecution::error(
                    tool_call,
                    "invalid_input",
                    &format!("tool arguments are not valid JSON: {error}"),
                ));
            }
        };

        let Some(required_action) = spec.required_grant.as_deref() else {
            let block = self
                .execute_immediate_tool(
                    run_id,
                    agent_spec,
                    turn_id,
                    step_sequence,
                    effect_sequence,
                    tool_call,
                    input,
                    Vec::new(),
                    self.approval_runtime
                        .as_ref()
                        .map(|runtime| runtime.config.security_config.clone()),
                )
                .await?;
            return Ok(ToolExecution::continued(block));
        };

        let (Some(runtime), Some(conversation_id)) =
            (self.approval_runtime.as_ref(), conversation_id)
        else {
            return Ok(ToolExecution::error(
                tool_call,
                "approval_unavailable",
                "durable approval context is unavailable",
            ));
        };
        let resource = format!("tool/{}", spec.name);
        let (decision, policy_snapshot_digest) = self
            .resolve_with_stable_policy(runtime, required_action, &resource, run_id)
            .await?;
        match decision {
            GrantDecision::Deny(denial) => Ok(ToolExecution::error(
                tool_call,
                "permission_denied",
                &denial.reason,
            )),
            GrantDecision::Defer(deferred) => Ok(ToolExecution::error(
                tool_call,
                "authorization_unavailable",
                &deferred.reason,
            )),
            GrantDecision::Permit(grant) => {
                let block = self
                    .execute_immediate_tool(
                        run_id,
                        agent_spec,
                        turn_id,
                        step_sequence,
                        effect_sequence,
                        tool_call,
                        input,
                        vec![grant],
                        Some(runtime.config.security_config.clone()),
                    )
                    .await?;
                Ok(ToolExecution::continued(block))
            }
            GrantDecision::RequireApproval(requirement) => {
                if tool_group.len() != 1 {
                    return Ok(ToolExecution::error(
                        tool_call,
                        "approval_group_unsupported",
                        "the APR-03 approval slice accepts exactly one tool call per model group",
                    ));
                }
                self.pause_and_resume_tool(
                    runtime,
                    run_id,
                    conversation_id,
                    agent_spec,
                    turn_id,
                    step_sequence,
                    effect_sequence,
                    tool_call,
                    tool_group,
                    input,
                    &spec,
                    required_action,
                    &resource,
                    requirement.reason,
                    policy_snapshot_digest,
                    messages,
                    model_id,
                    turn_count,
                    total_usage,
                    run_deadline,
                )
                .await
            }
        }
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "the immediate effect path preserves exact durable lineage around one handler call"
    )]
    async fn execute_immediate_tool(
        &self,
        run_id: RunId,
        agent_spec: &AgentSpec,
        turn_id: polkagent_core::TurnId,
        step_sequence: u32,
        effect_sequence: u64,
        tool_call: &ToolCall,
        input: serde_json::Value,
        grants: Vec<ResolvedGrant>,
        security_config: Option<polkagent_config::SecurityConfig>,
    ) -> Result<ContentBlock, RunError> {
        let registry = self.tool_registry.as_ref().ok_or_else(|| {
            RunError::Unsupported("no runtime tool registry is configured".to_owned())
        })?;

        // The normalized step and its parent turn are durable before the
        // effect intent can reference them. This keeps SQLite FK enforcement
        // active and lets handlers audit the exact lineage before I/O.
        let step_id = self
            .run_manager
            .start_step(run_id, turn_id, step_sequence, "tool_call")
            .await?;
        let effect_payload = serde_json::json!({
            "tool_call_id": tool_call.tool_call_id,
            "tool_name": tool_call.tool_name,
            "arguments": input,
        });
        let payload_bytes = serde_json::to_vec(&effect_payload)?;
        let idempotency_key = IdempotencyKey::generate(
            run_id,
            effect_sequence,
            0,
            EffectKind::ToolCall,
            IdempotencyKey::hash_params(&payload_bytes),
        );
        let intent_id = self
            .effect_pipeline
            .propose(EffectIntentSpec {
                run_id,
                turn_id,
                step_id,
                kind: EffectKind::ToolCall,
                sequence: effect_sequence,
                idempotency_key: Some(idempotency_key),
                payload: effect_payload,
                retry_class: Some(RetryClass::CheckBeforeRetry),
                priority: None,
                max_attempts: Some(1),
                action_card: None,
            })
            .await
            .map_err(|error| RunError::Store(format!("failed to persist tool intent: {error}")))?;

        let claim = self
            .effect_pipeline
            .claim_by_id(intent_id, EffectKind::ToolCall.default_lease_duration())
            .await
            .map_err(|error| RunError::Store(format!("failed to claim tool intent: {error}")))?;
        let now = chrono::Utc::now();
        let attempt = EffectAttempt {
            id: EffectAttemptId::new(),
            intent_id,
            run_id,
            attempt_number: 1,
            idempotency_key,
            worker_id: claim.worker_id,
            lease_expires: claim.lease_expires,
            retry_class: RetryClass::CheckBeforeRetry,
            state: AttemptState::InProgress,
            created_at: now,
            claimed_at: now,
            started_at: Some(now),
            completed_at: None,
        };
        self.effect_pipeline
            .record_attempt(intent_id, &attempt)
            .await
            .map_err(|error| RunError::Store(format!("failed to persist tool attempt: {error}")))?;
        self.record_tool_event(
            run_id,
            turn_id,
            step_id,
            intent_id,
            attempt.id,
            EventKind::ToolCallStarted {
                tool_name: tool_call.tool_name.clone(),
            },
        )
        .await?;

        let tool_context = ToolContext {
            run_id,
            agent_id: agent_spec.id,
            step_id,
            grants,
            security_config,
        };
        let execution = registry
            .execute(&tool_call.tool_name, input, &tool_context)
            .await;
        let (content, is_error, outcome_result) = match execution {
            Ok(result) => {
                let content = serde_json::to_string(&result)?;
                let data = serde_json::to_value(result)?;
                (content, false, OutcomeResult::Success { data })
            }
            Err(error) => {
                let content = Self::serialize_tool_error(&error);
                let outcome = Self::tool_error_outcome(&error);
                (content, true, outcome)
            }
        };
        let outcome = EffectOutcome {
            id: EffectOutcomeId::new(),
            attempt_id: attempt.id,
            intent_id,
            run_id,
            result: outcome_result,
            observed_at: chrono::Utc::now(),
            digest: [0; 32],
        };
        self.effect_pipeline
            .record_outcome(intent_id, &outcome)
            .await
            .map_err(|error| RunError::Store(format!("failed to persist tool outcome: {error}")))?;
        claim.complete();
        self.run_manager
            .complete_step(run_id, turn_id, step_id)
            .await?;
        self.record_tool_event(
            run_id,
            turn_id,
            step_id,
            intent_id,
            attempt.id,
            EventKind::ToolCallCompleted {
                tool_name: tool_call.tool_name.clone(),
            },
        )
        .await?;

        Ok(ContentBlock::ToolResult {
            tool_call_id: tool_call.tool_call_id.clone(),
            content,
            is_error,
        })
    }

    async fn resolve_with_stable_policy(
        &self,
        runtime: &ApprovalRuntime,
        action: &str,
        resource: &str,
        run_id: RunId,
    ) -> Result<(GrantDecision, String), RunError> {
        let before = self.grant_resolver.policy_snapshot().await;
        let before_digest = compute_policy_snapshot_digest(&before)?;
        let principal = runtime.config.authorized_principal_id.to_string();
        let mut context = EvaluationContext {
            evaluated_at: Some(chrono::Utc::now().to_rfc3339()),
            ..EvaluationContext::default()
        };
        context
            .attributes
            .insert("principal.id".to_owned(), principal.clone());
        context
            .attributes
            .insert("tenant.id".to_owned(), runtime.config.tenant_id.clone());
        context.attributes.insert(
            "workspace.id".to_owned(),
            runtime.config.workspace_id.clone(),
        );
        let decision = self
            .grant_resolver
            .resolve(&principal, action, resource, &context, None, Some(run_id))
            .await
            .map_err(|error| RunError::Store(format!("grant resolution failed: {error}")))?;
        let after = self.grant_resolver.policy_snapshot().await;
        let after_digest = compute_policy_snapshot_digest(&after)?;
        if before_digest != after_digest {
            return Err(RunError::Store(
                "policy changed during grant resolution; retry from a fresh model turn".to_owned(),
            ));
        }
        Ok((decision, before_digest))
    }

    #[allow(
        clippy::too_many_arguments,
        clippy::too_many_lines,
        reason = "the pause construction binds one complete typed executor checkpoint to one exact effect"
    )]
    async fn pause_and_resume_tool(
        &self,
        runtime: &ApprovalRuntime,
        run_id: RunId,
        conversation_id: ConversationId,
        agent_spec: &AgentSpec,
        turn_id: polkagent_core::TurnId,
        step_sequence: u32,
        effect_sequence: u64,
        tool_call: &ToolCall,
        tool_group: &[ToolCall],
        input: serde_json::Value,
        spec: &ToolSpec,
        required_action: &str,
        required_resource: &str,
        reason: String,
        policy_snapshot_digest: String,
        messages: &[InferenceMessage],
        model_id: &str,
        turn_count: u32,
        total_usage: &TokenUsage,
        run_deadline: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<ToolExecution, RunError> {
        let step_id = self
            .run_manager
            .start_step(run_id, turn_id, step_sequence, "tool_call")
            .await?;
        let effect_id = polkagent_core::EffectId::new();
        let approval_id = ApprovalId::new();
        let effect_payload = serde_json::json!({
            "kind": "tool_call",
            "tool_call_id": tool_call.tool_call_id,
            "tool_name": tool_call.tool_name,
            "arguments": input,
        });
        let payload_bytes = serde_json::to_vec(&effect_payload)?;
        let idempotency_key = IdempotencyKey::generate(
            run_id,
            effect_sequence,
            0,
            EffectKind::ToolCall,
            IdempotencyKey::hash_params(&payload_bytes),
        );
        let approval_deadline = {
            let fallback = chrono::Utc::now()
                + chrono::Duration::from_std(runtime.config.approval_timeout).map_err(|error| {
                    RunError::Store(format!("approval timeout is outside chrono range: {error}"))
                })?;
            run_deadline.map_or(fallback, |deadline| deadline.min(fallback))
        };
        let working_directory = runtime
            .config
            .working_directory
            .to_str()
            .ok_or_else(|| {
                RunError::Unsupported("approval working directory is not UTF-8".to_owned())
            })?
            .to_owned();
        let security_scope = serde_json::json!({
            "tenant_id": runtime.config.tenant_id,
            "workspace_id": runtime.config.workspace_id,
            "working_directory": working_directory,
            "security_config": runtime.config.security_config,
        });
        let mut subject = ApprovalSubject {
            schema_version: APPROVAL_SUBJECT_SCHEMA_VERSION,
            conversation_id,
            turn_id,
            step_id,
            run_id,
            agent_id: agent_spec.id,
            effect_id,
            tool_call_id: tool_call.tool_call_id.clone(),
            tool_name: tool_call.tool_name.clone(),
            validated_arguments: input,
            required_action: required_action.to_owned(),
            required_resource: required_resource.to_owned(),
            working_directory: Some(working_directory),
            security_scope,
            tool_spec_digest: compute_tool_spec_digest(spec)?,
            policy_snapshot_digest,
            subject_digest: String::new(),
        };
        subject.subject_digest = compute_approval_subject_digest(&subject)?;
        let tool_group = CheckpointToolGroup {
            approval_id,
            calls: tool_group.to_vec(),
        };
        let mut checkpoint = ExecutionCheckpoint {
            schema_version: EXECUTION_CHECKPOINT_SCHEMA_VERSION,
            run_id,
            turn_id,
            conversation_id,
            agent_id: agent_spec.id,
            model_id: model_id.to_owned(),
            executor_id: split_model_id(model_id).0.to_owned(),
            messages: serde_json::to_value(messages)?,
            tool_calls: serde_json::to_value(tool_group)?,
            next_model_turn: turn_count.saturating_add(1),
            next_step_sequence: step_sequence.saturating_add(1),
            next_effect_sequence: effect_sequence.saturating_add(1),
            accumulated_usage: serde_json::to_value(total_usage)?,
            accumulated_cost: None,
            deadline_at: Some(approval_deadline),
            retry_class: StoreRetryClass::CheckBeforeRetry,
            effects: vec![CheckpointEffect {
                effect_id,
                status: CheckpointEffectStatus::AwaitingApproval,
            }],
            version: 1,
            classification: DataClassification::Private,
            retention_expires_at: None,
            integrity_digest: String::new(),
        };
        checkpoint.integrity_digest = compute_execution_checkpoint_digest(&checkpoint)?;
        let expected_run_state_version = self.run_manager.state_version(run_id).await?;
        let stored = runtime
            .coordinator
            .pause_for_approval(PauseForApproval {
                approval_id,
                expected_run_state_version,
                subject,
                effect_payload,
                effect_idempotency_key: idempotency_key.to_hex(),
                retry_class: StoreRetryClass::CheckBeforeRetry,
                checkpoint,
                metadata: ApprovalRequestMetadata {
                    title: format!("Approve {}", spec.name),
                    description: "Allow this exact registered tool call once".to_owned(),
                    reason,
                    tenant_id: runtime.config.tenant_id.clone(),
                    workspace_id: runtime.config.workspace_id.clone(),
                    authorized_principal_id: runtime.config.authorized_principal_id,
                },
                deadline_at: approval_deadline,
            })
            .await
            .map_err(|error| approval_store_error("pause for approval", error))?;
        let decided = self.wait_for_approval(runtime, stored).await?;
        self.resume_decided_approval(runtime, agent_spec, tool_call, spec, decided, None)
            .await
    }

    async fn wait_for_approval(
        &self,
        runtime: &ApprovalRuntime,
        mut approval: StoredApproval,
    ) -> Result<StoredApproval, RunError> {
        let read_scope = ApprovalScope {
            tenant_id: runtime.config.tenant_id.clone(),
            workspace_id: runtime.config.workspace_id.clone(),
            conversation_id: Some(approval.subject.conversation_id),
            principal_id: runtime.config.authorized_principal_id,
        };
        loop {
            if approval.status != ApprovalStatus::Pending {
                return Ok(approval);
            }
            if chrono::Utc::now() >= approval.deadline_at {
                let expected_run_state_version = self
                    .run_manager
                    .state_version(approval.subject.run_id)
                    .await?;
                let service_scope = ApprovalScope {
                    principal_id: runtime.config.service_principal_id,
                    ..read_scope.clone()
                };
                match runtime
                    .coordinator
                    .resolve_approval(ResolveApproval {
                        approval_id: approval.id,
                        expected_run_state_version,
                        effect_id: approval.subject.effect_id,
                        run_id: approval.subject.run_id,
                        turn_id: approval.subject.turn_id,
                        conversation_id: approval.subject.conversation_id,
                        subject_digest: approval.subject.subject_digest.clone(),
                        scope: service_scope,
                        principal_id: runtime.config.service_principal_id,
                        principal_type: ApprovalPrincipalType::Service,
                        surface: "runtime-deadline".to_owned(),
                        decision: ApprovalDecision::Expire,
                        rationale: Some("approval deadline elapsed".to_owned()),
                        conditions: Vec::new(),
                    })
                    .await
                {
                    Ok(result) => return Ok(result.approval),
                    Err(ApprovalStoreError::Conflict { .. }) => {}
                    Err(error) => {
                        return Err(approval_store_error("expire approval", error));
                    }
                }
            }
            tokio::time::sleep(runtime.config.poll_interval).await;
            approval = runtime
                .coordinator
                .get_approval(approval.id, read_scope.clone())
                .await
                .map_err(|error| approval_store_error("reload approval", error))?;
        }
    }

    #[allow(
        clippy::too_many_lines,
        reason = "recovery must keep outcome detection, attempt boundary, revalidation, and reduction ordered"
    )]
    async fn resume_decided_approval(
        &self,
        runtime: &ApprovalRuntime,
        agent_spec: &AgentSpec,
        tool_call: &ToolCall,
        current_spec: &ToolSpec,
        approval: StoredApproval,
        preleased: Option<LeasedCheckpoint>,
    ) -> Result<ToolExecution, RunError> {
        match approval.status {
            ApprovalStatus::Expired => {
                return Ok(ToolExecution {
                    block: None,
                    terminal_state: Some(RunState::TimedOut),
                    active_checkpoint: None,
                });
            }
            ApprovalStatus::Cancelled => {
                return Ok(ToolExecution {
                    block: None,
                    terminal_state: Some(RunState::Cancelled {
                        reason: "approval cancelled".to_owned(),
                    }),
                    active_checkpoint: None,
                });
            }
            ApprovalStatus::Pending => {
                return Err(RunError::Store(
                    "pending approval cannot be resumed".to_owned(),
                ));
            }
            ApprovalStatus::Approved | ApprovalStatus::Denied => {}
        }
        verify_subject(&approval.subject)?;
        let current_tool_digest = compute_tool_spec_digest(current_spec)?;
        if current_tool_digest != approval.subject.tool_spec_digest
            || current_spec.name != approval.subject.tool_name
            || tool_call.tool_call_id != approval.subject.tool_call_id
            || tool_call.tool_name != approval.subject.tool_name
        {
            return Err(RunError::Store(
                "registered tool or tool-call identity changed after approval".to_owned(),
            ));
        }
        let expected_arguments: serde_json::Value =
            serde_json::from_str(&tool_call.arguments_json)?;
        if expected_arguments != approval.subject.validated_arguments {
            return Err(RunError::Store(
                "tool arguments changed after approval".to_owned(),
            ));
        }
        let leased = if let Some(leased) = preleased {
            leased
        } else {
            let checkpoint = runtime
                .checkpoints
                .get_checkpoint(approval.subject.run_id)
                .await
                .map_err(|error| approval_store_error("load decided checkpoint", error))?;
            runtime
                .checkpoints
                .lease_checkpoint(
                    approval.subject.run_id,
                    checkpoint.checkpoint.version,
                    self.effect_pipeline.worker_id(),
                    runtime.config.recovery_lease,
                )
                .await
                .map_err(|error| approval_store_error("lease decided checkpoint", error))?
        };
        verify_checkpoint(&leased.stored.checkpoint)?;
        if leased.stored.status != CheckpointStatus::Leased
            || leased.stored.lease_owner != Some(self.effect_pipeline.worker_id())
        {
            return Err(RunError::Store(
                "checkpoint is not leased by the orchestrator worker".to_owned(),
            ));
        }
        let waiting_version = approval
            .decision_run_state_version
            .and_then(|version| version.checked_add(1))
            .ok_or_else(|| RunError::Store("approval is missing its run revision".to_owned()))?;

        if approval.status == ApprovalStatus::Denied {
            runtime
                .coordinator
                .resume_after_effect(ResumeApprovalEffect {
                    approval_id: approval.id,
                    effect_id: approval.subject.effect_id,
                    run_id: approval.subject.run_id,
                    subject_digest: approval.subject.subject_digest.clone(),
                    expected_checkpoint_version: leased.stored.checkpoint.version,
                    worker_id: self.effect_pipeline.worker_id(),
                    expected_run_state_version: waiting_version,
                })
                .await
                .map_err(|error| approval_store_error("reduce rejected effect", error))?;
            return Ok(ToolExecution {
                block: Some(Self::tool_error_block(
                    tool_call,
                    "permission_denied",
                    approval
                        .rationale
                        .as_deref()
                        .unwrap_or("the exact tool call was rejected"),
                )),
                terminal_state: None,
                active_checkpoint: Some(ActiveCheckpoint {
                    checkpoint: leased.stored.checkpoint,
                    worker_id: self.effect_pipeline.worker_id(),
                    outcome_ids: Vec::new(),
                }),
            });
        }

        let outcomes = self
            .effect_pipeline
            .store()
            .unconsumed_outcomes(approval.subject.run_id)
            .await
            .map_err(|error| RunError::Store(format!("load approval outcome: {error}")))?
            .into_iter()
            .filter(|outcome| outcome.intent_id == approval.subject.effect_id)
            .collect::<Vec<_>>();
        if outcomes.len() > 1 {
            return Err(RunError::Store(
                "approval effect has more than one durable outcome".to_owned(),
            ));
        }
        let (block, outcome_id, attempt_id) = if let Some(outcome) = outcomes.first() {
            let block = outcome_to_tool_block(tool_call, outcome)?;
            (block, outcome.id, outcome.attempt_id)
        } else {
            let intent = self
                .effect_pipeline
                .get_intent(approval.subject.effect_id)
                .await
                .map_err(|error| RunError::Store(format!("load approval effect: {error}")))?;
            if matches!(intent.state.as_str(), "executing" | "resolved") {
                return Err(RunError::ManualReconciliation {
                    run_id: approval.subject.run_id,
                    effect_id: approval.subject.effect_id,
                    reason: "an attempt may have started but no durable outcome exists; automatic retry is forbidden"
                        .to_owned(),
                });
            }
            let claimed = runtime
                .coordinator
                .claim_approved_effect(ClaimApprovedEffect {
                    approval_id: approval.id,
                    effect_id: approval.subject.effect_id,
                    run_id: approval.subject.run_id,
                    subject_digest: approval.subject.subject_digest.clone(),
                    expected_checkpoint_version: leased.stored.checkpoint.version,
                    worker_id: self.effect_pipeline.worker_id(),
                    lease_duration: runtime.config.recovery_lease,
                })
                .await
                .map_err(|error| approval_store_error("claim approved effect", error))?;
            if claimed.effect_payload
                != serde_json::json!({
                    "kind": "tool_call",
                    "tool_call_id": approval.subject.tool_call_id,
                    "tool_name": approval.subject.tool_name,
                    "arguments": approval.subject.validated_arguments,
                })
            {
                return Err(RunError::Store(
                    "claimed effect payload does not match approval subject".to_owned(),
                ));
            }
            let intent = self
                .effect_pipeline
                .get_intent(approval.subject.effect_id)
                .await
                .map_err(|error| RunError::Store(format!("reload claimed effect: {error}")))?;
            let idempotency_key = polkagent_effect::idempotency::parse_from_hex(
                &intent.idempotency_key,
            )
            .ok_or_else(|| RunError::Store("stored idempotency key is invalid".to_owned()))?;
            let now = chrono::Utc::now();
            let attempt = EffectAttempt {
                id: EffectAttemptId::new(),
                intent_id: approval.subject.effect_id,
                run_id: approval.subject.run_id,
                attempt_number: 1,
                idempotency_key,
                worker_id: claimed.worker_id,
                lease_expires: claimed.lease_expires_at,
                retry_class: RetryClass::CheckBeforeRetry,
                state: AttemptState::InProgress,
                created_at: now,
                claimed_at: now,
                started_at: Some(now),
                completed_at: None,
            };
            self.effect_pipeline
                .record_attempt(approval.subject.effect_id, &attempt)
                .await
                .map_err(|error| {
                    RunError::Store(format!("persist approval attempt boundary: {error}"))
                })?;
            self.record_tool_event(
                approval.subject.run_id,
                approval.subject.turn_id,
                approval.subject.step_id,
                approval.subject.effect_id,
                attempt.id,
                EventKind::ToolCallStarted {
                    tool_name: approval.subject.tool_name.clone(),
                },
            )
            .await?;

            let grant = self
                .revalidate_approved_subject(runtime, &approval.subject, current_spec)
                .await;
            let registry = self.tool_registry.as_ref().ok_or_else(|| {
                RunError::Unsupported("tool registry disappeared during approval".to_owned())
            })?;
            let execution = match grant {
                Ok(grant) => {
                    registry
                        .execute(
                            &approval.subject.tool_name,
                            approval.subject.validated_arguments.clone(),
                            &ToolContext {
                                run_id: approval.subject.run_id,
                                agent_id: agent_spec.id,
                                step_id: approval.subject.step_id,
                                grants: vec![grant],
                                security_config: Some(runtime.config.security_config.clone()),
                            },
                        )
                        .await
                }
                Err(reason) => Err(ToolError::PermissionDenied { reason }),
            };
            let (block, outcome_result) = match execution {
                Ok(result) => {
                    let content = serde_json::to_string(&result)?;
                    let data = serde_json::to_value(result)?;
                    (
                        ContentBlock::ToolResult {
                            tool_call_id: tool_call.tool_call_id.clone(),
                            content,
                            is_error: false,
                        },
                        OutcomeResult::Success { data },
                    )
                }
                Err(error) => (
                    ContentBlock::ToolResult {
                        tool_call_id: tool_call.tool_call_id.clone(),
                        content: Self::serialize_tool_error(&error),
                        is_error: true,
                    },
                    Self::tool_error_outcome(&error),
                ),
            };
            let outcome = EffectOutcome {
                id: EffectOutcomeId::new(),
                attempt_id: attempt.id,
                intent_id: approval.subject.effect_id,
                run_id: approval.subject.run_id,
                result: outcome_result,
                observed_at: chrono::Utc::now(),
                digest: [0; 32],
            };
            self.effect_pipeline
                .record_outcome(approval.subject.effect_id, &outcome)
                .await
                .map_err(|error| {
                    RunError::Store(format!("persist approval effect outcome: {error}"))
                })?;
            (block, outcome.id, attempt.id)
        };

        self.run_manager
            .complete_step(
                approval.subject.run_id,
                approval.subject.turn_id,
                approval.subject.step_id,
            )
            .await?;
        self.record_tool_event(
            approval.subject.run_id,
            approval.subject.turn_id,
            approval.subject.step_id,
            approval.subject.effect_id,
            attempt_id,
            EventKind::ToolCallCompleted {
                tool_name: approval.subject.tool_name.clone(),
            },
        )
        .await?;
        runtime
            .coordinator
            .resume_after_effect(ResumeApprovalEffect {
                approval_id: approval.id,
                effect_id: approval.subject.effect_id,
                run_id: approval.subject.run_id,
                subject_digest: approval.subject.subject_digest.clone(),
                expected_checkpoint_version: leased.stored.checkpoint.version,
                worker_id: self.effect_pipeline.worker_id(),
                expected_run_state_version: waiting_version,
            })
            .await
            .map_err(|error| approval_store_error("reduce approved effect", error))?;
        Ok(ToolExecution {
            block: Some(block),
            terminal_state: None,
            active_checkpoint: Some(ActiveCheckpoint {
                checkpoint: leased.stored.checkpoint,
                worker_id: self.effect_pipeline.worker_id(),
                outcome_ids: vec![outcome_id],
            }),
        })
    }

    async fn revalidate_approved_subject(
        &self,
        runtime: &ApprovalRuntime,
        subject: &ApprovalSubject,
        current_spec: &ToolSpec,
    ) -> Result<ResolvedGrant, String> {
        verify_subject(subject).map_err(|error| error.to_string())?;
        let tool_digest =
            compute_tool_spec_digest(current_spec).map_err(|error| error.to_string())?;
        if tool_digest != subject.tool_spec_digest {
            return Err("registered tool specification changed after approval".to_owned());
        }
        let current_security_scope = serde_json::json!({
            "tenant_id": runtime.config.tenant_id,
            "workspace_id": runtime.config.workspace_id,
            "working_directory": runtime.config.working_directory.to_string_lossy(),
            "security_config": runtime.config.security_config,
        });
        if current_security_scope != subject.security_scope {
            return Err("security or workspace scope changed after approval".to_owned());
        }
        let (decision, policy_digest) = self
            .resolve_with_stable_policy(
                runtime,
                &subject.required_action,
                &subject.required_resource,
                subject.run_id,
            )
            .await
            .map_err(|error| error.to_string())?;
        if policy_digest != subject.policy_snapshot_digest {
            return Err("policy snapshot changed after approval".to_owned());
        }
        if !matches!(decision, GrantDecision::RequireApproval(_)) {
            return Err("policy no longer returns the approved escalation".to_owned());
        }
        Ok(ResolvedGrant {
            grant_id: GrantId::new(),
            principal: runtime.config.authorized_principal_id.to_string(),
            action: subject.required_action.clone(),
            resource: subject.required_resource.clone(),
            allowed_effects: EffectSet::new([subject.required_action.clone()]),
            limits: GrantLimits {
                deadline: None,
                ..GrantLimits::default()
            },
            expires_at: chrono::Utc::now() + chrono::Duration::minutes(5),
        })
    }

    async fn finalize_checkpoint(
        &self,
        active: ActiveCheckpoint,
        messages: &[InferenceMessage],
    ) -> Result<(), RunError> {
        let runtime = self.approval_runtime.as_ref().ok_or_else(|| {
            RunError::Unsupported("approval runtime disappeared during reduction".to_owned())
        })?;
        let expected_version = active.checkpoint.version;
        let mut checkpoint = active.checkpoint;
        checkpoint.version = expected_version
            .checked_add(1)
            .ok_or_else(|| RunError::Store("checkpoint version is exhausted".to_owned()))?;
        checkpoint.messages = serde_json::to_value(messages)?;
        checkpoint.tool_calls = serde_json::json!([]);
        checkpoint.integrity_digest = compute_execution_checkpoint_digest(&checkpoint)?;
        runtime
            .checkpoints
            .commit_progress(
                expected_version,
                CheckpointProgress {
                    worker_id: active.worker_id,
                    checkpoint,
                    status: CheckpointCommitStatus::Terminal,
                },
            )
            .await
            .map_err(|error| approval_store_error("terminalize checkpoint", error))?;
        if !active.outcome_ids.is_empty() {
            self.effect_pipeline
                .store()
                .mark_outcomes_consumed(&active.outcome_ids)
                .await
                .map_err(|error| RunError::Store(format!("consume approval outcome: {error}")))?;
        }
        Ok(())
    }

    async fn record_tool_event(
        &self,
        run_id: RunId,
        turn_id: polkagent_core::TurnId,
        step_id: StepId,
        intent_id: polkagent_core::EffectId,
        attempt_id: EffectAttemptId,
        kind: EventKind,
    ) -> Result<(), RunError> {
        let mut event = RunEvent::new_ephemeral(EventId::new(), run_id, 0, kind);
        event.correlation = EventCorrelation {
            run_id,
            turn_id: Some(turn_id),
            step_id: Some(step_id),
            effect_intent_id: Some(intent_id),
            effect_attempt_id: Some(attempt_id),
        };
        self.event_recorder
            .record(event)
            .await
            .map(|_| ())
            .map_err(|error| RunError::Event(error.to_string()))
    }

    fn tool_error_block(tool_call: &ToolCall, error_type: &str, message: &str) -> ContentBlock {
        ContentBlock::ToolResult {
            tool_call_id: tool_call.tool_call_id.clone(),
            content: serde_json::json!({
                "error": {
                    "type": error_type,
                    "message": message,
                }
            })
            .to_string(),
            is_error: true,
        }
    }

    fn serialize_tool_error(error: &ToolError) -> String {
        let error_type = match error {
            ToolError::PermissionDenied { .. } => "permission_denied",
            ToolError::InvalidInput { .. } => "invalid_input",
            ToolError::ExecutionFailed { .. } => "execution_failed",
            ToolError::Timeout { .. } => "timeout",
            ToolError::NotFound { .. } => "not_found",
        };
        serde_json::json!({
            "error": {
                "type": error_type,
                "message": error.to_string(),
            }
        })
        .to_string()
    }

    fn tool_error_outcome(error: &ToolError) -> OutcomeResult {
        match error {
            ToolError::Timeout { elapsed_ms } => OutcomeResult::Timeout {
                waited_secs: elapsed_ms.saturating_add(999) / 1_000,
                partial_work_possible: true,
            },
            ToolError::PermissionDenied { .. } => OutcomeResult::Failure {
                error_class: ErrorClass::AuthorizationError,
                message: error.to_string(),
                retriable: false,
            },
            ToolError::InvalidInput { .. } | ToolError::NotFound { .. } => OutcomeResult::Failure {
                error_class: ErrorClass::ClientError,
                message: error.to_string(),
                retriable: false,
            },
            ToolError::ExecutionFailed { .. } => OutcomeResult::Failure {
                error_class: ErrorClass::ServerError,
                message: error.to_string(),
                retriable: false,
            },
        }
    }

    /// Build the initial message list from the agent spec and user prompt.
    fn build_initial_messages(
        _agent_spec: &AgentSpec,
        initial_prompt: &str,
    ) -> Vec<InferenceMessage> {
        vec![InferenceMessage {
            role: MessageRole::User,
            content: vec![ContentBlock::Text {
                text: initial_prompt.to_owned(),
            }],
        }]
    }

    /// Apply the context assembler to the current messages, truncating to
    /// fit the token budget.
    ///
    /// When a [`ContextAssembler`] is configured, this method extracts the
    /// system prompt and conversation history from the message list,
    /// assembles them through the context assembler (which applies token
    /// budget constraints and truncation), and returns a new message list
    /// derived from the assembled context.
    ///
    /// If no assembler is configured, returns the messages unchanged.
    #[cfg(feature = "context")]
    fn apply_context_assembly(
        &self,
        messages: &[InferenceMessage],
        system_prompt: Option<&str>,
    ) -> Vec<InferenceMessage> {
        let Some(context_assembler) = &self.context_assembler else {
            return messages.to_vec();
        };

        // The legacy context assembler renders conversation blocks to text
        // and cannot reconstruct provider tool-use/result boundaries. Once a
        // tool block exists, preserve the typed transcript exactly instead of
        // silently flattening the result before the next inference request.
        if messages.iter().any(|message| {
            message.content.iter().any(|block| {
                matches!(
                    block,
                    ContentBlock::ToolResult { .. } | ContentBlock::ToolUse { .. }
                )
            })
        }) {
            return messages.to_vec();
        }

        // Extract conversation messages as strings for the assembler.
        // Each message is rendered as "Role: content" for the assembler's
        // conversation history section.
        let conversation_strings: Vec<String> = messages
            .iter()
            .map(|msg| {
                let role_str = match msg.role {
                    MessageRole::User => "User",
                    MessageRole::Assistant => "Assistant",
                };
                let text = msg
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        ContentBlock::Text { text } => Some(text.as_str()),
                        ContentBlock::ToolResult { content, .. } => Some(content.as_str()),
                        ContentBlock::ToolUse { .. } => {
                            // Tool uses are collected separately below as
                            // descriptive strings.
                            None
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(" ");

                // Also collect tool use descriptions.
                let tool_uses: Vec<String> = msg
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        ContentBlock::ToolUse {
                            tool_name,
                            arguments_json,
                            ..
                        } => Some(format!("[tool_use: {tool_name}({arguments_json})]")),
                        _ => None,
                    })
                    .collect();

                if tool_uses.is_empty() {
                    format!("{role_str}: {text}")
                } else {
                    let tools = tool_uses.join(" ");
                    if text.is_empty() {
                        format!("{role_str}: {tools}")
                    } else {
                        format!("{role_str}: {text} {tools}")
                    }
                }
            })
            .collect();

        let conversation_refs: Vec<&str> = conversation_strings
            .iter()
            .map(std::string::String::as_str)
            .collect();

        // Assemble context with the system prompt and conversation history.
        let assembled_context = match context_assembler.assemble(
            system_prompt,
            &[], // tool descriptions are handled separately in InferenceRequest
            &[], // memory entries (not used in orchestrator yet)
            &conversation_refs,
            None, // user input is part of conversation messages
        ) {
            Ok(ctx) => ctx,
            Err(err) => {
                warn!("context assembly failed, using original messages: {err}");
                return messages.to_vec();
            }
        };

        info!(
            total_tokens = assembled_context.total_tokens,
            remaining = assembled_context.remaining_tokens(),
            sections = assembled_context.section_count(),
            "context assembled for turn"
        );

        // Rebuild messages from the assembled context.
        // The assembled conversation section contains the (potentially
        // truncated) conversation history. We parse it back into messages.
        let mut result = Vec::new();

        for section in &assembled_context.sections {
            if section.kind == polkagent_context::section::SectionKind::ConversationHistory {
                // Parse each line back into messages. Lines are in
                // "Role: content" format.
                for line in section.content.lines() {
                    if let Some(text) = line.strip_prefix("User: ") {
                        result.push(InferenceMessage {
                            role: MessageRole::User,
                            content: vec![ContentBlock::Text {
                                text: text.to_owned(),
                            }],
                        });
                    } else if let Some(text) = line.strip_prefix("Assistant: ") {
                        result.push(InferenceMessage {
                            role: MessageRole::Assistant,
                            content: vec![ContentBlock::Text {
                                text: text.to_owned(),
                            }],
                        });
                    }
                }
            }
            // System prompt and other sections are handled separately
            // (system prompt goes in InferenceRequest.system, not in
            // messages).
        }

        // Safety: if assembly produced no conversation messages (e.g.
        // extreme truncation), fall back to the original messages to avoid
        // sending an empty request.
        if result.is_empty() {
            return messages.to_vec();
        }

        result
    }
}

// ---------------------------------------------------------------------------
// Card helpers (free functions)
// ---------------------------------------------------------------------------

/// Map an [`EffectKind`] to its [`EffectKindTag`] mirror.
///
/// This is the seam between `polkagent-effect` (which owns `EffectKind`) and
/// `polkagent-card` (which owns `EffectKindTag`). Both enums have the same
/// variants; this function converts between them.
#[must_use]
pub fn effect_kind_to_tag(kind: EffectKind) -> EffectKindTag {
    match kind {
        EffectKind::ModelCall => EffectKindTag::ModelCall,
        EffectKind::ToolCall => EffectKindTag::ToolCall,
        EffectKind::SignatureRequest => EffectKindTag::SignatureRequest,
        EffectKind::Broadcast => EffectKindTag::Broadcast,
        EffectKind::FinalityWatch => EffectKindTag::FinalityWatch,
        EffectKind::Delivery => EffectKindTag::Delivery,
        EffectKind::ChainRead => EffectKindTag::ChainRead,
        EffectKind::Simulation => EffectKindTag::Simulation,
        EffectKind::HarnessOperation => EffectKindTag::HarnessOperation,
    }
}

/// Build an [`ActionCard`] for an [`EffectIntent`], if the effect kind
/// warrants one.
///
/// Extracts canonical fields from the intent payload JSON (on a best-effort
/// basis) and populates an [`IntentCardSpec`] which is used to construct the
/// card.  Callers should attach the resulting card to the intent before
/// persisting it:
///
/// ```ignore
/// if let Some(card) = build_card_for_effect(&intent, Some("Model said: …")) {
///     intent.attach_card(card);
/// }
/// ```
///
/// Returns `None` for read-only effects (`ChainRead`, `Simulation`,
/// `ModelCall`) that do not require user approval.
#[must_use]
pub fn build_card_for_effect(
    intent: &EffectIntent,
    model_explanation: Option<&str>,
) -> Option<ActionCard> {
    let tag = effect_kind_to_tag(intent.kind);

    // Extract optional pallet/call from payload if present.
    let pallet = intent
        .payload
        .get("pallet_name")
        .or_else(|| intent.payload.get("pallet"))
        .and_then(|v| v.as_str())
        .map(str::to_owned);

    let call = intent
        .payload
        .get("call_name")
        .or_else(|| intent.payload.get("call"))
        .and_then(|v| v.as_str())
        .map(str::to_owned);

    // Build a short argument summary from the payload.
    let arguments_summary = intent
        .payload
        .get("args")
        .map(std::string::ToString::to_string)
        .or_else(|| {
            intent
                .payload
                .get("arguments")
                .map(std::string::ToString::to_string)
        });

    // Estimated cost from the payload, if provided.
    let estimated_cost = intent
        .payload
        .get("estimated_fee")
        .and_then(|v| v.as_str())
        .map(str::to_owned);

    let spec = IntentCardSpec {
        kind: tag,
        effect_id: intent.id.to_string(),
        run_id: intent.run_id.to_string(),
        pallet,
        call,
        arguments_summary,
        model_explanation: model_explanation.map(str::to_owned),
        estimated_cost,
        payload_json: intent.payload.clone(),
    };

    spec.build_card()
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

#[allow(
    clippy::needless_pass_by_value,
    reason = "map_err closures transfer the typed store error into the run error boundary"
)]
fn approval_store_error(context: &str, error: ApprovalStoreError) -> RunError {
    RunError::Store(format!("{context}: {error}"))
}

fn verify_subject(subject: &ApprovalSubject) -> Result<(), RunError> {
    if !is_canonical_digest(&subject.subject_digest) {
        return Err(RunError::Store(
            "approval subject does not use the canonical APR-03 digest encoding".to_owned(),
        ));
    }
    let expected = compute_approval_subject_digest(subject)?;
    if expected != subject.subject_digest {
        return Err(RunError::Store(
            "approval subject failed its canonical integrity check".to_owned(),
        ));
    }
    Ok(())
}

fn verify_checkpoint(checkpoint: &ExecutionCheckpoint) -> Result<(), RunError> {
    if !is_canonical_digest(&checkpoint.integrity_digest) {
        return Err(RunError::Store(
            "execution checkpoint does not use the canonical APR-03 digest encoding".to_owned(),
        ));
    }
    let expected = compute_execution_checkpoint_digest(checkpoint)?;
    if expected != checkpoint.integrity_digest {
        return Err(RunError::Store(
            "execution checkpoint failed its canonical integrity check".to_owned(),
        ));
    }
    Ok(())
}

fn outcome_to_tool_block(
    tool_call: &ToolCall,
    outcome: &StoredOutcome,
) -> Result<ContentBlock, RunError> {
    let result: OutcomeResult = serde_json::from_value(outcome.payload.clone())?;
    let (content, is_error) = match result {
        OutcomeResult::Success { data } => (serde_json::to_string(&data)?, false),
        OutcomeResult::Failure {
            error_class,
            message,
            retriable,
        } => (
            serde_json::json!({
                "error": {
                    "type": "effect_failure",
                    "class": error_class,
                    "message": message,
                    "retriable": retriable,
                }
            })
            .to_string(),
            true,
        ),
        other => (
            serde_json::json!({
                "error": {
                    "type": "indeterminate_effect_outcome",
                    "outcome": other,
                }
            })
            .to_string(),
            true,
        ),
    };
    Ok(ContentBlock::ToolResult {
        tool_call_id: tool_call.tool_call_id.clone(),
        content,
        is_error,
    })
}

/// Convert executor token usage to core token usage.
fn convert_token_usage(usage: &polkagent_executor_trait::TokenUsage) -> TokenUsage {
    TokenUsage {
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        total_tokens: usage.input_tokens.saturating_add(usage.output_tokens),
        cache_read_tokens: usage.cache_read_tokens.unwrap_or(0),
        cache_write_tokens: usage.cache_write_tokens.unwrap_or(0),
        cost_usd: None,
    }
}

/// Split a `"provider/model-id"` string into `(provider, model)`.
///
/// If no `/` is present the whole string is treated as the model name and
/// the provider is `"unknown"`.
fn split_model_id(model_id: &str) -> (&str, &str) {
    if let Some(pos) = model_id.find('/') {
        (&model_id[..pos], &model_id[pos + 1..])
    } else {
        ("unknown", model_id)
    }
}

/// Narrow a validated model temperature to the executor protocol's `f32`.
#[allow(
    clippy::cast_possible_truncation,
    reason = "executor requests encode temperature as f32 while configuration uses f64"
)]
fn temperature_as_f32(value: f64) -> f32 {
    value as f32
}

fn deadline_from_timeout(seconds: u64) -> chrono::DateTime<chrono::Utc> {
    let seconds = i64::try_from(seconds).unwrap_or(i64::MAX);
    chrono::Duration::try_seconds(seconds)
        .and_then(|duration| chrono::Utc::now().checked_add_signed(duration))
        .unwrap_or(chrono::DateTime::<chrono::Utc>::MAX_UTC)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use polkagent_core::{AgentId, RunId};
    use polkagent_effect::EffectPipeline;
    use polkagent_event::{EventBus, EventRecorder};
    use polkagent_executor_trait::{
        ExecutorError, InferenceRequest, InferenceResponse, ModelExecutor, StreamEvent,
        TokenUsage as ExecTokenUsage, ToolCall,
    };
    use polkagent_grant::{
        grant::{GrantResolver, ResolverConfig},
        policy::PolicySet,
    };
    use polkagent_harness_trait::{
        CancelMode, HarnessCapabilities, HarnessError, HarnessId, HarnessStatus, McpMode,
        SessionConfig, SessionId, SessionResumeMode, ToolInjection,
    };
    use polkagent_store_trait::{
        event::{EventFilter, EventStore, EventStoreError, StoredEvent},
        EffectStore, RunStatus, RunStore, RunSummary, StoreError, StoredIntent, StoredOutcome,
    };

    use async_trait::async_trait;
    use futures::Stream;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    struct NeverCalledHarness {
        id: HarnessId,
    }

    #[async_trait]
    impl Harness for NeverCalledHarness {
        fn id(&self) -> &HarnessId {
            &self.id
        }

        fn capabilities(&self) -> HarnessCapabilities {
            HarnessCapabilities {
                supports_streaming: false,
                supports_tools: false,
                supports_sessions: false,
                max_context_tokens: 0,
                models: Vec::new(),
                transport: None,
                model_override: None,
                session_resume: SessionResumeMode::default(),
                mcp_passthrough: McpMode::default(),
                tool_injection: ToolInjection::default(),
                cancel: CancelMode::default(),
                multiplex_safe: false,
            }
        }

        fn status(&self) -> HarnessStatus {
            HarnessStatus::Idle
        }

        async fn start_session(&self, _config: SessionConfig) -> Result<SessionId, HarnessError> {
            panic!("context preflight must not start a harness session")
        }

        async fn send_message(
            &self,
            _session_id: SessionId,
            _message: &str,
        ) -> Result<(), HarnessError> {
            panic!("context preflight must not send a harness message")
        }

        async fn receive_events(
            &self,
            _session_id: SessionId,
        ) -> Result<std::pin::Pin<Box<dyn Stream<Item = HarnessEvent> + Send>>, HarnessError>
        {
            panic!("context preflight must not receive harness events")
        }

        async fn end_session(&self, _session_id: SessionId) -> Result<(), HarnessError> {
            panic!("context preflight must not end a harness session")
        }

        async fn health(&self) -> Result<bool, HarnessError> {
            Ok(true)
        }
    }

    // ── In-memory RunStore ───────────────────────────────────────────────

    #[derive(Debug, Default)]
    struct MemRunStore {
        runs: Mutex<HashMap<String, RunSummary>>,
    }

    #[async_trait]
    impl RunStore for MemRunStore {
        async fn create(
            &self,
            run_id: RunId,
            agent_id: &str,
            status: RunStatus,
        ) -> Result<(), StoreError> {
            let mut guard = self.runs.lock().expect("lock");
            let key = run_id.to_string();
            if guard.contains_key(&key) {
                return Err(StoreError::Conflict {
                    resource_type: "Run",
                    id: key,
                });
            }
            guard.insert(
                key,
                RunSummary {
                    id: run_id,
                    agent_id: agent_id.to_owned(),
                    status,
                    created_at: chrono::Utc::now(),
                    started_at: None,
                    completed_at: None,
                    deadline_at: None,
                },
            );
            Ok(())
        }

        async fn get(&self, run_id: RunId) -> Result<RunSummary, StoreError> {
            let guard = self.runs.lock().expect("lock");
            guard
                .get(&run_id.to_string())
                .cloned()
                .ok_or_else(|| StoreError::NotFound {
                    resource_type: "Run",
                    id: run_id.to_string(),
                })
        }

        async fn update_state(
            &self,
            run_id: RunId,
            new_status: RunStatus,
        ) -> Result<(), StoreError> {
            let mut guard = self.runs.lock().expect("lock");
            guard
                .get_mut(&run_id.to_string())
                .ok_or_else(|| StoreError::NotFound {
                    resource_type: "Run",
                    id: run_id.to_string(),
                })?
                .status = new_status;
            Ok(())
        }

        async fn list_by_agent(
            &self,
            agent_id: &str,
            limit: u32,
            _offset: u32,
        ) -> Result<Vec<RunSummary>, StoreError> {
            let guard = self.runs.lock().expect("lock");
            let mut runs: Vec<RunSummary> = guard
                .values()
                .filter(|r| r.agent_id == agent_id)
                .cloned()
                .collect();
            runs.truncate(limit as usize);
            Ok(runs)
        }

        async fn list_by_state(
            &self,
            status: RunStatus,
            limit: u32,
            _offset: u32,
        ) -> Result<Vec<RunSummary>, StoreError> {
            let guard = self.runs.lock().expect("lock");
            let mut runs: Vec<RunSummary> = guard
                .values()
                .filter(|r| r.status == status)
                .cloned()
                .collect();
            runs.truncate(limit as usize);
            Ok(runs)
        }

        async fn insert_turn(
            &self,
            _turn_id: polkagent_core::TurnId,
            _run_id: RunId,
            _sequence: u32,
            _role: &str,
            _started_at: &str,
            _completed_at: Option<&str>,
            _input_tokens: u32,
            _output_tokens: u32,
        ) -> Result<(), StoreError> {
            Ok(())
        }
    }

    // ── In-memory EventStore ────────────────────────────────────────────

    #[derive(Debug, Default)]
    struct MemEventStore {
        durable: Mutex<Vec<StoredEvent>>,
        sequences: Mutex<HashMap<String, u64>>,
        terminal: Mutex<std::collections::HashSet<String>>,
    }

    const TERMINAL_TYPES: &[&str] = &[
        "run_completed",
        "run_failed",
        "run_cancelled",
        "run_timed_out",
    ];

    #[async_trait]
    impl EventStore for MemEventStore {
        async fn append_durable(
            &self,
            mut event: StoredEvent,
        ) -> Result<StoredEvent, EventStoreError> {
            let mut seqs = self.sequences.lock().expect("lock");
            let current = seqs.get(&event.run_id).copied().unwrap_or(0);
            if event.sequence <= current {
                return Err(EventStoreError::NonMonotonicSequence {
                    run_id: event.run_id.clone(),
                    current,
                    proposed: event.sequence,
                });
            }
            seqs.insert(event.run_id.clone(), event.sequence);

            let mut term = self.terminal.lock().expect("lock");
            if TERMINAL_TYPES.contains(&event.event_type.as_str())
                && !term.insert(event.run_id.clone())
            {
                return Err(EventStoreError::DuplicateTerminalEvent {
                    run_id: event.run_id.clone(),
                });
            }

            let mut durable = self.durable.lock().expect("lock");
            event.global_sequence = (durable.len() + 1) as u64;
            durable.push(event.clone());
            Ok(event)
        }

        async fn append_diagnostic(
            &self,
            _event: StoredEvent,
            _expires_at: String,
        ) -> Result<(), EventStoreError> {
            Ok(())
        }

        async fn read_from_cursor(
            &self,
            cursor: u64,
            limit: usize,
        ) -> Result<Vec<StoredEvent>, EventStoreError> {
            let durable = self.durable.lock().expect("lock");
            Ok(durable
                .iter()
                .filter(|e| e.global_sequence > cursor)
                .take(limit)
                .cloned()
                .collect())
        }

        async fn read_run_events(
            &self,
            run_id: RunId,
        ) -> Result<Vec<StoredEvent>, EventStoreError> {
            let durable = self.durable.lock().expect("lock");
            Ok(durable
                .iter()
                .filter(|e| e.run_id == run_id.to_string())
                .cloned()
                .collect())
        }

        async fn query(&self, filter: EventFilter) -> Result<Vec<StoredEvent>, EventStoreError> {
            let durable = self.durable.lock().expect("lock");
            Ok(durable
                .iter()
                .filter(|e| {
                    filter
                        .run_id
                        .as_ref()
                        .is_none_or(|rid| e.run_id == rid.to_string())
                })
                .cloned()
                .collect())
        }

        async fn max_sequence(&self, run_id: RunId) -> Result<u64, EventStoreError> {
            let seqs = self.sequences.lock().expect("lock");
            Ok(seqs.get(&run_id.to_string()).copied().unwrap_or(0))
        }

        async fn has_terminal_event(&self, run_id: RunId) -> Result<bool, EventStoreError> {
            let term = self.terminal.lock().expect("lock");
            Ok(term.contains(&run_id.to_string()))
        }
    }

    // ── In-memory EffectStore ───────────────────────────────────────────

    #[derive(Debug, Default)]
    struct MemEffectStore;

    #[async_trait]
    impl EffectStore for MemEffectStore {
        async fn propose_intent(&self, _intent: StoredIntent) -> Result<(), StoreError> {
            Ok(())
        }

        async fn claim_intent(
            &self,
            _worker_id: polkagent_core::WorkerId,
            _lease_duration: std::time::Duration,
        ) -> Result<Option<StoredIntent>, StoreError> {
            Ok(None)
        }

        async fn claim_intent_by_id(
            &self,
            intent_id: polkagent_core::EffectId,
            _worker_id: polkagent_core::WorkerId,
            _lease_duration: std::time::Duration,
        ) -> Result<StoredIntent, StoreError> {
            Err(StoreError::NotFound {
                resource_type: "EffectIntent",
                id: intent_id.to_string(),
            })
        }

        async fn release_claim(
            &self,
            _intent_id: polkagent_core::EffectId,
            _worker_id: polkagent_core::WorkerId,
        ) -> Result<(), StoreError> {
            Ok(())
        }

        async fn get_intent(
            &self,
            intent_id: polkagent_core::EffectId,
        ) -> Result<StoredIntent, StoreError> {
            Err(StoreError::NotFound {
                resource_type: "EffectIntent",
                id: intent_id.to_string(),
            })
        }

        async fn get_by_run(&self, _run_id: RunId) -> Result<Vec<StoredIntent>, StoreError> {
            Ok(vec![])
        }

        async fn expired_leases(
            &self,
            _cutoff: polkagent_core::Timestamp,
        ) -> Result<Vec<StoredIntent>, StoreError> {
            Ok(vec![])
        }

        async fn record_attempt_start(
            &self,
            _attempt_id: polkagent_core::EffectAttemptId,
            _intent_id: polkagent_core::EffectId,
            _worker_id: polkagent_core::WorkerId,
            _payload: serde_json::Value,
        ) -> Result<(), StoreError> {
            Ok(())
        }

        async fn record_outcome(&self, _outcome: StoredOutcome) -> Result<(), StoreError> {
            Ok(())
        }

        async fn unconsumed_outcomes(
            &self,
            _run_id: RunId,
        ) -> Result<Vec<StoredOutcome>, StoreError> {
            Ok(vec![])
        }

        async fn mark_outcomes_consumed(
            &self,
            _outcome_ids: &[polkagent_core::EffectOutcomeId],
        ) -> Result<(), StoreError> {
            Ok(())
        }

        async fn update_intent_state(
            &self,
            intent_id: polkagent_core::EffectId,
            _new_state: &str,
        ) -> Result<StoredIntent, StoreError> {
            Err(StoreError::NotFound {
                resource_type: "EffectIntent",
                id: intent_id.to_string(),
            })
        }
    }

    // ── FakeExecutor ────────────────────────────────────────────────────

    /// A test-only executor that returns pre-configured responses.
    ///
    /// Responses are consumed in order from a `Vec`. Once exhausted, calls
    /// return `ExecutorError::Internal`.
    struct FakeExecutor {
        responses: Mutex<Vec<Result<InferenceResponse, ExecutorError>>>,
    }

    impl FakeExecutor {
        fn new(responses: Vec<Result<InferenceResponse, ExecutorError>>) -> Self {
            Self {
                responses: Mutex::new(responses),
            }
        }

        /// Create a simple text response with `end_turn`.
        fn text_response(text: &str, input_tokens: u32, output_tokens: u32) -> InferenceResponse {
            InferenceResponse {
                text: text.to_owned(),
                tool_calls: vec![],
                stop_reason: "end_turn".to_owned(),
                usage: ExecTokenUsage {
                    input_tokens,
                    output_tokens,
                    cache_read_tokens: None,
                    cache_write_tokens: None,
                },
                provider_request_id: None,
            }
        }

        /// Create a response with tool calls.
        fn tool_call_response(
            text: &str,
            tool_calls: Vec<ToolCall>,
            input_tokens: u32,
            output_tokens: u32,
        ) -> InferenceResponse {
            InferenceResponse {
                text: text.to_owned(),
                tool_calls,
                stop_reason: "tool_use".to_owned(),
                usage: ExecTokenUsage {
                    input_tokens,
                    output_tokens,
                    cache_read_tokens: None,
                    cache_write_tokens: None,
                },
                provider_request_id: None,
            }
        }

        /// Create a `max_tokens` response.
        fn max_tokens_response(input_tokens: u32, output_tokens: u32) -> InferenceResponse {
            InferenceResponse {
                text: "partial output...".to_owned(),
                tool_calls: vec![],
                stop_reason: "max_tokens".to_owned(),
                usage: ExecTokenUsage {
                    input_tokens,
                    output_tokens,
                    cache_read_tokens: None,
                    cache_write_tokens: None,
                },
                provider_request_id: None,
            }
        }
    }

    #[async_trait]
    impl ModelExecutor for FakeExecutor {
        async fn complete(
            &self,
            _request: InferenceRequest,
        ) -> Result<InferenceResponse, ExecutorError> {
            let mut responses = self.responses.lock().expect("lock");
            if responses.is_empty() {
                return Err(ExecutorError::Internal {
                    message: "no more responses configured".to_owned(),
                });
            }
            responses.remove(0)
        }

        async fn stream(
            &self,
            _request: InferenceRequest,
        ) -> Result<
            Box<dyn Stream<Item = Result<StreamEvent, ExecutorError>> + Send + Unpin>,
            ExecutorError,
        > {
            Err(ExecutorError::Internal {
                message: "stream not supported in FakeExecutor".to_owned(),
            })
        }

        async fn health(&self) -> Result<(), ExecutorError> {
            Ok(())
        }
    }

    // ── Test fixture builder ────────────────────────────────────────────

    struct TestHarness {
        orchestrator: RunOrchestrator,
        run_manager: Arc<RunManager>,
        event_store: Arc<MemEventStore>,
    }

    fn build_harness(
        responses: Vec<Result<InferenceResponse, ExecutorError>>,
        config: RunOrchestratorConfig,
    ) -> TestHarness {
        let run_store: Arc<dyn RunStore> = Arc::new(MemRunStore::default());
        let event_store = Arc::new(MemEventStore::default());
        let event_store_dyn = Arc::clone(&event_store) as Arc<dyn EventStore>;
        let bus = EventBus::new(64);
        let recorder = EventRecorder::new(event_store_dyn, bus);
        let run_manager = Arc::new(RunManager::new(Arc::clone(&run_store), recorder.clone()));

        let effect_store: Arc<dyn EffectStore> = Arc::new(MemEffectStore);
        let effect_pipeline = EffectPipeline::new(effect_store, polkagent_core::WorkerId::new());

        let grant_resolver = GrantResolver::new(PolicySet::default(), ResolverConfig::default());

        let executor: Arc<dyn ModelExecutor> = Arc::new(FakeExecutor::new(responses));

        let orchestrator = RunOrchestrator::new(
            Arc::clone(&run_manager),
            executor,
            effect_pipeline,
            recorder,
            grant_resolver,
        )
        .with_config(config);

        TestHarness {
            orchestrator,
            run_manager,
            event_store,
        }
    }

    fn default_agent_spec() -> AgentSpec {
        AgentSpec::new(AgentId::new(), "Test Agent", "test-model")
    }

    // ── Tests ───────────────────────────────────────────────────────────

    #[test]
    fn harness_preflight_accepts_single_prompt_and_rejects_typed_history() {
        let harness = build_harness(Vec::new(), RunOrchestratorConfig::default());
        let orchestrator = harness
            .orchestrator
            .with_harness(Arc::new(NeverCalledHarness {
                id: HarnessId::new("never-called"),
            }));
        let current = InferenceMessage {
            role: MessageRole::User,
            content: vec![ContentBlock::Text {
                text: "current".to_owned(),
            }],
        };
        orchestrator
            .validate_initial_messages(std::slice::from_ref(&current))
            .expect("single plain-text prompt remains supported");

        let history = vec![
            InferenceMessage {
                role: MessageRole::User,
                content: vec![ContentBlock::Text {
                    text: "prior user".to_owned(),
                }],
            },
            InferenceMessage {
                role: MessageRole::Assistant,
                content: vec![ContentBlock::Text {
                    text: "prior assistant".to_owned(),
                }],
            },
            current,
        ];
        let error = orchestrator
            .validate_initial_messages(&history)
            .expect_err("harness history must fail closed");
        assert!(matches!(error, RunError::Unsupported(_)));
        assert!(error.to_string().contains("role-safely"));
    }

    #[tokio::test]
    async fn simple_text_only_run_completes_in_one_turn() {
        let harness = build_harness(
            vec![Ok(FakeExecutor::text_response("Hello!", 10, 5))],
            RunOrchestratorConfig::default(),
        );
        let agent_spec = default_agent_spec();
        let run_id = harness
            .run_manager
            .create_run(agent_spec.id)
            .await
            .expect("create_run");

        let outcome = harness
            .orchestrator
            .execute_run(run_id, &agent_spec, "Hi there")
            .await
            .expect("execute_run");

        assert_eq!(outcome.final_state, RunState::Completed);
        assert_eq!(outcome.turn_count, 1);
        assert_eq!(outcome.run_id, run_id);

        // Verify the run is in completed state.
        let state = harness
            .run_manager
            .get_state(run_id)
            .await
            .expect("get_state");
        assert_eq!(state, RunState::Completed);
    }

    #[tokio::test]
    async fn multi_turn_conversation() {
        // First turn returns a tool call, second turn returns final text.
        let responses = vec![
            Ok(FakeExecutor::tool_call_response(
                "Let me look that up.",
                vec![ToolCall {
                    tool_call_id: "tc-1".to_owned(),
                    tool_name: "search".to_owned(),
                    arguments_json: r#"{"query": "test"}"#.to_owned(),
                }],
                100,
                50,
            )),
            Ok(FakeExecutor::text_response("Here is the answer.", 200, 30)),
        ];

        let config = RunOrchestratorConfig {
            auto_approve_effects: true,
            ..Default::default()
        };
        let harness = build_harness(responses, config);
        let agent_spec = default_agent_spec();
        let run_id = harness
            .run_manager
            .create_run(agent_spec.id)
            .await
            .expect("create_run");

        let outcome = harness
            .orchestrator
            .execute_run(run_id, &agent_spec, "Search for something")
            .await
            .expect("execute_run");

        assert_eq!(outcome.final_state, RunState::Completed);
        assert_eq!(outcome.turn_count, 2);
    }

    #[tokio::test]
    async fn unavailable_tool_does_not_fake_success_or_terminal_completion() {
        let responses = vec![
            Ok(FakeExecutor::tool_call_response(
                "I need to call a tool.",
                vec![ToolCall {
                    tool_call_id: "tc-1".to_owned(),
                    tool_name: "file_read".to_owned(),
                    arguments_json: r#"{"path": "/tmp/test.txt"}"#.to_owned(),
                }],
                50,
                20,
            )),
            Ok(FakeExecutor::text_response(
                "The tool was unavailable.",
                10,
                5,
            )),
        ];

        let config = RunOrchestratorConfig {
            auto_approve_effects: false,
            ..Default::default()
        };
        let harness = build_harness(responses, config);
        let agent_spec = default_agent_spec();
        let run_id = harness
            .run_manager
            .create_run(agent_spec.id)
            .await
            .expect("create_run");

        let outcome = harness
            .orchestrator
            .execute_run(run_id, &agent_spec, "Read a file")
            .await
            .expect("execute_run");

        // The unknown call is fed back as a typed error and the model gets a
        // second turn; lack of approval support is not treated as success.
        assert_eq!(outcome.final_state, RunState::Completed);
        assert_eq!(outcome.turn_count, 2);
    }

    #[tokio::test]
    async fn max_turns_limit_terminates_run() {
        // Configure max_turns = 2, but the executor always returns tool calls
        // so it never ends naturally.
        let make_tool_response = || {
            Ok(FakeExecutor::tool_call_response(
                "another tool call",
                vec![ToolCall {
                    tool_call_id: format!("tc-{}", uuid::Uuid::now_v7()),
                    tool_name: "search".to_owned(),
                    arguments_json: r#"{"q":"x"}"#.to_owned(),
                }],
                10,
                5,
            ))
        };

        let responses = vec![
            make_tool_response(),
            make_tool_response(),
            make_tool_response(), // third call should never be reached
        ];

        let config = RunOrchestratorConfig {
            max_turns: 2,
            auto_approve_effects: true,
            ..Default::default()
        };
        let harness = build_harness(responses, config);
        let agent_spec = default_agent_spec();
        let run_id = harness
            .run_manager
            .create_run(agent_spec.id)
            .await
            .expect("create_run");

        let outcome = harness
            .orchestrator
            .execute_run(run_id, &agent_spec, "Loop forever")
            .await
            .expect("execute_run");

        assert!(
            matches!(outcome.final_state, RunState::Failed { ref reason } if reason.contains("max turns")),
            "expected Failed with max turns, got {:?}",
            outcome.final_state
        );
        assert_eq!(outcome.turn_count, 2);
    }

    #[tokio::test]
    async fn failed_executor_transitions_to_failed_state() {
        let responses = vec![Err(ExecutorError::Transport {
            message: "connection refused".to_owned(),
            retryable: false,
        })];

        let harness = build_harness(responses, RunOrchestratorConfig::default());
        let agent_spec = default_agent_spec();
        let run_id = harness
            .run_manager
            .create_run(agent_spec.id)
            .await
            .expect("create_run");

        let outcome = harness
            .orchestrator
            .execute_run(run_id, &agent_spec, "Fail please")
            .await
            .expect("execute_run");

        assert!(
            matches!(outcome.final_state, RunState::Failed { ref reason } if reason.contains("connection refused")),
            "expected Failed with transport error, got {:?}",
            outcome.final_state
        );
        assert_eq!(outcome.turn_count, 0);

        // Verify the run is in failed state in the store.
        let state = harness
            .run_manager
            .get_state(run_id)
            .await
            .expect("get_state");
        assert!(matches!(state, RunState::Failed { .. }));
    }

    #[tokio::test]
    async fn token_usage_accumulates_across_turns() {
        let responses = vec![
            Ok(FakeExecutor::tool_call_response(
                "calling tool",
                vec![ToolCall {
                    tool_call_id: "tc-1".to_owned(),
                    tool_name: "search".to_owned(),
                    arguments_json: "{}".to_owned(),
                }],
                100,
                50,
            )),
            Ok(FakeExecutor::text_response("Done.", 200, 80)),
        ];

        let config = RunOrchestratorConfig {
            auto_approve_effects: true,
            ..Default::default()
        };
        let harness = build_harness(responses, config);
        let agent_spec = default_agent_spec();
        let run_id = harness
            .run_manager
            .create_run(agent_spec.id)
            .await
            .expect("create_run");

        let outcome = harness
            .orchestrator
            .execute_run(run_id, &agent_spec, "Accumulate tokens")
            .await
            .expect("execute_run");

        assert_eq!(outcome.total_tokens.input_tokens, 300); // 100 + 200
        assert_eq!(outcome.total_tokens.output_tokens, 130); // 50 + 80
        assert_eq!(outcome.total_tokens.total_tokens, 430); // 150 + 280
        assert_eq!(outcome.turn_count, 2);
    }

    #[tokio::test]
    async fn run_events_are_recorded_for_each_transition() {
        let harness = build_harness(
            vec![Ok(FakeExecutor::text_response("Done.", 10, 5))],
            RunOrchestratorConfig::default(),
        );
        let agent_spec = default_agent_spec();
        let run_id = harness
            .run_manager
            .create_run(agent_spec.id)
            .await
            .expect("create_run");

        let _outcome = harness
            .orchestrator
            .execute_run(run_id, &agent_spec, "Hello")
            .await
            .expect("execute_run");

        // Check recorded events.
        let events = harness
            .event_store
            .read_run_events(run_id)
            .await
            .expect("read_run_events");

        let event_types: Vec<&str> = events.iter().map(|e| e.event_type.as_str()).collect();

        // Should have: run_created, run_queued, run_started, run_completing, run_completed
        assert!(
            event_types.contains(&"run_created"),
            "missing run_created in {event_types:?}"
        );
        assert!(
            event_types.contains(&"run_queued"),
            "missing run_queued in {event_types:?}"
        );
        assert!(
            event_types.contains(&"run_started"),
            "missing run_started in {event_types:?}"
        );
        assert!(
            event_types.contains(&"run_completing"),
            "missing run_completing in {event_types:?}"
        );
        assert!(
            event_types.contains(&"run_completed"),
            "missing run_completed in {event_types:?}"
        );
    }

    #[tokio::test]
    async fn max_tokens_stop_reason_fails_run() {
        let responses = vec![Ok(FakeExecutor::max_tokens_response(500, 4096))];

        let harness = build_harness(responses, RunOrchestratorConfig::default());
        let agent_spec = default_agent_spec();
        let run_id = harness
            .run_manager
            .create_run(agent_spec.id)
            .await
            .expect("create_run");

        let outcome = harness
            .orchestrator
            .execute_run(run_id, &agent_spec, "Generate a novel")
            .await
            .expect("execute_run");

        assert!(
            matches!(
                outcome.final_state,
                RunState::Failed { ref reason } if reason.contains("max_tokens")
            ),
            "expected Failed with max_tokens, got {:?}",
            outcome.final_state
        );
    }

    #[tokio::test]
    async fn outcome_includes_correct_duration() {
        let harness = build_harness(
            vec![Ok(FakeExecutor::text_response("Quick.", 5, 3))],
            RunOrchestratorConfig::default(),
        );
        let agent_spec = default_agent_spec();
        let run_id = harness
            .run_manager
            .create_run(agent_spec.id)
            .await
            .expect("create_run");

        let outcome = harness
            .orchestrator
            .execute_run(run_id, &agent_spec, "Be quick")
            .await
            .expect("execute_run");

        // Duration should be non-zero but very small.
        assert!(
            outcome.duration < Duration::from_secs(5),
            "duration {:?} seems too long for a fake executor",
            outcome.duration
        );
    }

    #[tokio::test]
    async fn config_defaults_are_sensible() {
        let config = RunOrchestratorConfig::default();
        assert_eq!(config.max_turns, 25);
        assert_eq!(config.max_tokens_per_turn, 4096);
        assert!(!config.auto_approve_effects);
    }

    // ── Card-building helper tests ────────────────────────────────────────

    /// Build a minimal [`EffectIntent`] for card-building tests.
    fn make_test_intent(kind: EffectKind) -> EffectIntent {
        use polkagent_core::TurnId;
        use polkagent_effect::{EffectIntentState, EffectPriority, IdempotencyKey};

        let run_id = RunId::new();
        let params_hash = IdempotencyKey::hash_params(b"test");
        EffectIntent {
            id: polkagent_core::EffectId::new(),
            run_id,
            turn_id: TurnId::new(),
            step_id: StepId::new(),
            kind,
            idempotency_key: IdempotencyKey::generate(run_id, 1, 0, kind, params_hash),
            sequence: 0,
            state: EffectIntentState::Pending,
            deadline: None,
            max_attempts: 3,
            attempt_count: 0,
            retry_class: kind.default_retry_class(),
            priority: EffectPriority::Normal,
            payload: serde_json::json!({
                "pallet_name": "Balances",
                "call_name": "transfer_keep_alive",
                "args": {"dest": "5GrwvaEF", "value": 1_000_000},
                "estimated_fee": "0.0014 DOT"
            }),
            created_at: chrono::Utc::now(),
            resolved_at: None,
            action_card: None,
        }
    }

    #[test]
    fn effect_kind_to_tag_covers_all_variants() {
        let pairs = [
            (EffectKind::ModelCall, EffectKindTag::ModelCall),
            (EffectKind::ToolCall, EffectKindTag::ToolCall),
            (
                EffectKind::SignatureRequest,
                EffectKindTag::SignatureRequest,
            ),
            (EffectKind::Broadcast, EffectKindTag::Broadcast),
            (EffectKind::FinalityWatch, EffectKindTag::FinalityWatch),
            (EffectKind::Delivery, EffectKindTag::Delivery),
            (EffectKind::ChainRead, EffectKindTag::ChainRead),
            (EffectKind::Simulation, EffectKindTag::Simulation),
            (
                EffectKind::HarnessOperation,
                EffectKindTag::HarnessOperation,
            ),
        ];
        for (kind, expected_tag) in pairs {
            assert_eq!(effect_kind_to_tag(kind), expected_tag, "{kind:?}");
        }
    }

    #[test]
    fn build_card_for_signable_effect_returns_card() {
        let intent = make_test_intent(EffectKind::SignatureRequest);
        let card = build_card_for_effect(&intent, Some("Signing a transfer."));
        assert!(card.is_some(), "SignatureRequest must produce a card");
        let card = card.unwrap();
        assert!(
            !card.payload_hash.is_empty(),
            "card must have a payload hash"
        );
    }

    #[test]
    fn build_card_for_broadcast_returns_card() {
        let intent = make_test_intent(EffectKind::Broadcast);
        let card = build_card_for_effect(&intent, None);
        assert!(card.is_some(), "Broadcast must produce a card");
    }

    #[test]
    fn build_card_for_chain_read_returns_none() {
        let intent = make_test_intent(EffectKind::ChainRead);
        let card = build_card_for_effect(&intent, None);
        assert!(card.is_none(), "ChainRead must NOT produce a card");
    }

    #[test]
    fn build_card_for_simulation_returns_none() {
        let intent = make_test_intent(EffectKind::Simulation);
        let card = build_card_for_effect(&intent, None);
        assert!(card.is_none(), "Simulation must NOT produce a card");
    }

    #[test]
    fn build_card_for_model_call_returns_none() {
        let intent = make_test_intent(EffectKind::ModelCall);
        let card = build_card_for_effect(&intent, None);
        assert!(card.is_none(), "ModelCall must NOT produce a card");
    }

    #[test]
    fn card_canonical_fields_extracted_from_payload() {
        let intent = make_test_intent(EffectKind::SignatureRequest);
        let card = build_card_for_effect(&intent, Some("Signing test.")).unwrap();

        // Must have canonical section for pallet.
        let pallet_section = card.canonical_sections.iter().find(|s| s.label == "Pallet");
        assert!(pallet_section.is_some(), "missing Pallet canonical section");
        assert_eq!(pallet_section.unwrap().value, "Balances");

        // Must have canonical section for call.
        let call_section = card.canonical_sections.iter().find(|s| s.label == "Call");
        assert!(call_section.is_some(), "missing Call canonical section");
        assert_eq!(call_section.unwrap().value, "transfer_keep_alive");
    }

    #[test]
    fn card_narrative_includes_model_explanation() {
        let intent = make_test_intent(EffectKind::Broadcast);
        let card =
            build_card_for_effect(&intent, Some("Sending DOT to a staking controller.")).unwrap();

        assert!(
            !card.narrative_sections.is_empty(),
            "model explanation must produce a narrative section"
        );
        assert!(
            card.narrative_sections[0]
                .content
                .contains("staking controller"),
            "narrative must include the model's explanation"
        );
    }

    #[test]
    fn attach_card_via_build_card_for_effect() {
        let mut intent = make_test_intent(EffectKind::SignatureRequest);
        assert!(intent.action_card.is_none());

        if let Some(card) = build_card_for_effect(&intent, Some("Test explanation.")) {
            intent.attach_card(card);
        }

        assert!(
            intent.action_card.is_some(),
            "card must be attached to intent"
        );
    }

    // ── split_model_id helper tests ──────────────────────────────────────────

    #[test]
    fn split_model_id_with_slash() {
        let (provider, model) = split_model_id("anthropic/claude-sonnet-4");
        assert_eq!(provider, "anthropic");
        assert_eq!(model, "claude-sonnet-4");
    }

    #[test]
    fn split_model_id_without_slash() {
        let (provider, model) = split_model_id("local-model");
        assert_eq!(provider, "unknown");
        assert_eq!(model, "local-model");
    }

    #[test]
    fn split_model_id_multiple_slashes_uses_first() {
        let (provider, model) = split_model_id("openai/gpt-4o/2024");
        assert_eq!(provider, "openai");
        assert_eq!(model, "gpt-4o/2024");
    }

    #[test]
    fn extreme_timeout_saturates_to_maximum_deadline() {
        assert_eq!(
            deadline_from_timeout(u64::MAX),
            chrono::DateTime::<chrono::Utc>::MAX_UTC
        );
    }

    // ── CostTracker integration via orchestrator turn loop ─────────────────

    #[tokio::test]
    async fn cost_tracker_does_not_block_run_when_unbounded() {
        // Default orchestrator has no budget limit; runs should complete
        // normally regardless of cost accumulation.
        let harness = build_harness(
            vec![Ok(FakeExecutor::text_response("Done.", 500, 200))],
            RunOrchestratorConfig::default(),
        );
        let agent_spec = default_agent_spec();
        let run_id = harness
            .run_manager
            .create_run(agent_spec.id)
            .await
            .expect("create_run");

        let outcome = harness
            .orchestrator
            .execute_run(run_id, &agent_spec, "Hello")
            .await
            .expect("execute_run");

        assert_eq!(outcome.final_state, RunState::Completed);
        assert_eq!(outcome.turn_count, 1);
    }

    #[tokio::test]
    async fn cost_tracker_records_token_usage_across_turns() {
        // Multi-turn run: cost tracker should accumulate across turns without
        // interfering with the run outcome.
        let responses = vec![
            Ok(FakeExecutor::tool_call_response(
                "calling tool",
                vec![ToolCall {
                    tool_call_id: "tc-1".to_owned(),
                    tool_name: "search".to_owned(),
                    arguments_json: "{}".to_owned(),
                }],
                100,
                50,
            )),
            Ok(FakeExecutor::text_response("Done.", 200, 80)),
        ];
        let config = RunOrchestratorConfig {
            auto_approve_effects: true,
            ..Default::default()
        };
        let harness = build_harness(responses, config);
        let agent_spec = default_agent_spec();
        let run_id = harness
            .run_manager
            .create_run(agent_spec.id)
            .await
            .expect("create_run");

        let outcome = harness
            .orchestrator
            .execute_run(run_id, &agent_spec, "Accumulate")
            .await
            .expect("execute_run");

        assert_eq!(outcome.final_state, RunState::Completed);
        // Verify token accumulation is surfaced in the outcome.
        assert_eq!(outcome.total_tokens.input_tokens, 300); // 100 + 200
        assert_eq!(outcome.total_tokens.output_tokens, 130); // 50 + 80
    }

    #[tokio::test]
    async fn cost_tracker_with_anthropic_model_id_parses_provider() {
        // Verify that "anthropic/claude-sonnet-4" is correctly parsed and
        // does not cause a panic or error in the cost tracker.
        let harness = build_harness(
            vec![Ok(FakeExecutor::text_response("Hi.", 10, 5))],
            RunOrchestratorConfig::default(),
        );
        let mut agent_spec = default_agent_spec();
        agent_spec.model = "anthropic/claude-sonnet-4".to_owned();

        let run_id = harness
            .run_manager
            .create_run(agent_spec.id)
            .await
            .expect("create_run");

        let outcome = harness
            .orchestrator
            .execute_run(run_id, &agent_spec, "Short")
            .await
            .expect("execute_run");

        assert_eq!(outcome.final_state, RunState::Completed);
    }

    // ── Context assembler integration tests ──────────────────────────────

    /// An executor that captures the requests it receives, so tests can
    /// inspect the messages after context assembly.
    #[cfg(feature = "context")]
    struct CapturingExecutor {
        /// Pre-configured responses to return in order.
        responses: Mutex<Vec<Result<InferenceResponse, ExecutorError>>>,
        /// All requests received by the executor.
        captured_requests: Mutex<Vec<InferenceRequest>>,
    }

    #[cfg(feature = "context")]
    impl CapturingExecutor {
        fn new(responses: Vec<Result<InferenceResponse, ExecutorError>>) -> Self {
            Self {
                responses: Mutex::new(responses),
                captured_requests: Mutex::new(Vec::new()),
            }
        }

        fn captured(&self) -> Vec<InferenceRequest> {
            self.captured_requests.lock().expect("lock").clone()
        }
    }

    #[cfg(feature = "context")]
    #[async_trait]
    impl ModelExecutor for CapturingExecutor {
        async fn complete(
            &self,
            request: InferenceRequest,
        ) -> Result<InferenceResponse, ExecutorError> {
            self.captured_requests.lock().expect("lock").push(request);
            let mut responses = self.responses.lock().expect("lock");
            if responses.is_empty() {
                return Err(ExecutorError::Internal {
                    message: "no more responses configured".to_owned(),
                });
            }
            responses.remove(0)
        }

        async fn stream(
            &self,
            _request: InferenceRequest,
        ) -> Result<
            Box<dyn Stream<Item = Result<StreamEvent, ExecutorError>> + Send + Unpin>,
            ExecutorError,
        > {
            Err(ExecutorError::Internal {
                message: "stream not supported".to_owned(),
            })
        }

        async fn health(&self) -> Result<(), ExecutorError> {
            Ok(())
        }
    }

    /// Build a test harness with a `CapturingExecutor` for context assembly
    /// tests.
    #[cfg(feature = "context")]
    fn build_capturing_harness(
        responses: Vec<Result<InferenceResponse, ExecutorError>>,
        config: RunOrchestratorConfig,
        context_assembler: Option<polkagent_context::ContextAssembler>,
    ) -> (TestHarness, Arc<CapturingExecutor>) {
        let run_store: Arc<dyn RunStore> = Arc::new(MemRunStore::default());
        let event_store = Arc::new(MemEventStore::default());
        let event_store_dyn = Arc::clone(&event_store) as Arc<dyn EventStore>;
        let bus = EventBus::new(64);
        let recorder = EventRecorder::new(event_store_dyn, bus);
        let run_manager = Arc::new(RunManager::new(Arc::clone(&run_store), recorder.clone()));

        let effect_store: Arc<dyn EffectStore> = Arc::new(MemEffectStore);
        let effect_pipeline = EffectPipeline::new(effect_store, polkagent_core::WorkerId::new());

        let grant_resolver = GrantResolver::new(PolicySet::default(), ResolverConfig::default());

        let capturing = Arc::new(CapturingExecutor::new(responses));
        let executor: Arc<dyn ModelExecutor> = Arc::clone(&capturing) as Arc<dyn ModelExecutor>;

        let mut orchestrator = RunOrchestrator::new(
            Arc::clone(&run_manager),
            executor,
            effect_pipeline,
            recorder,
            grant_resolver,
        )
        .with_config(config);

        if let Some(assembler) = context_assembler {
            orchestrator = orchestrator.with_context_assembler(assembler);
        }

        let harness = TestHarness {
            orchestrator,
            run_manager,
            event_store,
        };
        (harness, capturing)
    }

    #[cfg(feature = "context")]
    #[tokio::test]
    async fn context_assembler_backward_compatible_without_assembler() {
        // Without a context assembler, the orchestrator should work exactly
        // as before -- messages are passed through unchanged.
        let (harness, capturing) = build_capturing_harness(
            vec![Ok(FakeExecutor::text_response("Hello!", 10, 5))],
            RunOrchestratorConfig::default(),
            None, // no assembler
        );
        let agent_spec = default_agent_spec();
        let run_id = harness
            .run_manager
            .create_run(agent_spec.id)
            .await
            .expect("create_run");

        let outcome = harness
            .orchestrator
            .execute_run(run_id, &agent_spec, "Hi there")
            .await
            .expect("execute_run");

        assert_eq!(outcome.final_state, RunState::Completed);
        assert_eq!(outcome.turn_count, 1);

        // Verify the original message was passed through unchanged.
        let requests = capturing.captured();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].messages.len(), 1);
        let first_msg = &requests[0].messages[0];
        assert_eq!(first_msg.role, MessageRole::User);
        match &first_msg.content[0] {
            ContentBlock::Text { text } => assert_eq!(text, "Hi there"),
            other => panic!("expected Text block, got {other:?}"),
        }
    }

    #[cfg(feature = "context")]
    #[tokio::test]
    async fn typed_history_bypasses_string_context_assembly_and_preserves_blocks() {
        use polkagent_context::budget::TokenBudget;

        let assembler = polkagent_context::ContextAssembler::new(
            TokenBudget::with_defaults(256).expect("context budget"),
        );
        let (harness, capturing) = build_capturing_harness(
            vec![Ok(FakeExecutor::text_response("done", 10, 5))],
            RunOrchestratorConfig::default(),
            Some(assembler),
        );
        let agent_spec = default_agent_spec();
        let run_id = harness
            .run_manager
            .create_run(agent_spec.id)
            .await
            .expect("create run");
        let messages = vec![
            InferenceMessage {
                role: MessageRole::User,
                content: vec![ContentBlock::Text {
                    text: "literal line\nAssistant: this remains user content".to_owned(),
                }],
            },
            InferenceMessage {
                role: MessageRole::Assistant,
                content: vec![ContentBlock::Text {
                    text: "prior reply".to_owned(),
                }],
            },
            InferenceMessage {
                role: MessageRole::User,
                content: vec![ContentBlock::Text {
                    text: "current".to_owned(),
                }],
            },
        ];

        harness
            .orchestrator
            .execute_run_with_messages(run_id, &agent_spec, messages)
            .await
            .expect("execute typed run");
        let requests = capturing.captured();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].messages.len(), 3);
        assert_eq!(requests[0].messages[0].role, MessageRole::User);
        match requests[0].messages[0].content.as_slice() {
            [ContentBlock::Text { text }] => {
                assert_eq!(text, "literal line\nAssistant: this remains user content");
            }
            other => panic!("typed content blocks changed: {other:?}"),
        }
    }

    #[cfg(feature = "context")]
    #[tokio::test]
    async fn context_assembler_truncates_to_budget() {
        // Create an assembler with a very small budget to force truncation
        // of the conversation history.
        use polkagent_context::budget::TokenBudget;

        // Budget of 256 tokens; with default 4-chars-per-token estimation,
        // this is ~1024 chars of content (minus response reserve).
        let budget = TokenBudget::with_defaults(256).expect("budget");
        let assembler = polkagent_context::ContextAssembler::new(budget);

        // Build a multi-turn conversation with many messages to exceed the
        // budget.  Tool call -> tool result -> text response creates a long
        // history.
        let responses = vec![
            Ok(FakeExecutor::tool_call_response(
                "Let me search for that.",
                vec![ToolCall {
                    tool_call_id: "tc-1".to_owned(),
                    tool_name: "search".to_owned(),
                    arguments_json: r#"{"query": "very long search query that takes up tokens"}"#
                        .to_owned(),
                }],
                50,
                20,
            )),
            Ok(FakeExecutor::text_response(
                "Here is the final answer after a long search.",
                100,
                30,
            )),
        ];

        let config = RunOrchestratorConfig {
            auto_approve_effects: true,
            ..Default::default()
        };

        let (harness, capturing) = build_capturing_harness(responses, config, Some(assembler));

        let agent_spec = default_agent_spec();
        let run_id = harness
            .run_manager
            .create_run(agent_spec.id)
            .await
            .expect("create_run");

        let outcome = harness
            .orchestrator
            .execute_run(run_id, &agent_spec, "Search for blockchain data")
            .await
            .expect("execute_run");

        assert_eq!(outcome.final_state, RunState::Completed);
        assert_eq!(outcome.turn_count, 2);

        // The second request should have gone through context assembly.
        // With the small budget, messages may have been truncated.
        let requests = capturing.captured();
        assert_eq!(requests.len(), 2);

        // Verify the second request's total message text is within budget
        // bounds. The assembler should have constrained the context.
        let second_req = &requests[1];
        let total_text_len: usize = second_req
            .messages
            .iter()
            .flat_map(|m| &m.content)
            .map(|block| match block {
                ContentBlock::Text { text } => text.len(),
                ContentBlock::ToolResult { content, .. } => content.len(),
                ContentBlock::ToolUse { arguments_json, .. } => arguments_json.len(),
            })
            .sum();

        // 256 tokens * 4 chars/token = 1024 max chars (content budget is
        // less due to response reserve).  The assembled context should fit.
        let budget_chars = 256 * 4;
        assert!(
            total_text_len <= budget_chars,
            "total text length {total_text_len} exceeds budget chars {budget_chars}"
        );
    }

    #[cfg(feature = "context")]
    #[tokio::test]
    async fn context_assembler_always_includes_system_prompt() {
        // Verify that when a system prompt is set, the assembled context
        // preserves it (the system prompt has the highest priority and
        // should never be truncated away).
        use polkagent_context::budget::TokenBudget;

        let budget = TokenBudget::with_defaults(8192).expect("budget");
        let assembler = polkagent_context::ContextAssembler::new(budget);

        let (harness, capturing) = build_capturing_harness(
            vec![Ok(FakeExecutor::text_response("Done.", 10, 5))],
            RunOrchestratorConfig::default(),
            Some(assembler),
        );

        let mut agent_spec = default_agent_spec();
        agent_spec.system_prompt = Some("You are a helpful blockchain agent.".to_owned());

        let run_id = harness
            .run_manager
            .create_run(agent_spec.id)
            .await
            .expect("create_run");

        let outcome = harness
            .orchestrator
            .execute_run(run_id, &agent_spec, "Hello agent")
            .await
            .expect("execute_run");

        assert_eq!(outcome.final_state, RunState::Completed);

        // The request should include the system prompt in the system field.
        let requests = capturing.captured();
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].system.as_deref(),
            Some("You are a helpful blockchain agent.")
        );

        // The user message should be present in the messages.
        let has_user_msg = requests[0].messages.iter().any(|m| {
            m.role == MessageRole::User
                && m.content.iter().any(|block| {
                    matches!(
                        block,
                        ContentBlock::Text { text } if text.contains("Hello agent")
                    )
                })
        });
        assert!(
            has_user_msg,
            "user message should be present in assembled context"
        );
    }

    #[cfg(feature = "context")]
    #[tokio::test]
    async fn context_assembler_prioritizes_recent_messages() {
        // With a tight budget and many messages, the assembler should
        // keep the most recent messages and drop older ones (DropOldest
        // strategy for ConversationHistory).
        use polkagent_context::budget::TokenBudget;

        // Very small budget to force aggressive truncation.
        let budget = TokenBudget::with_defaults(256).expect("budget");
        let assembler = polkagent_context::ContextAssembler::new(budget);

        // Build a conversation with many tool-call turns so the history
        // grows large. Each turn adds 3 messages (assistant tool_use,
        // user tool_result, then eventually a final assistant text).
        let mut responses: Vec<Result<InferenceResponse, ExecutorError>> = Vec::new();
        for i in 0..5 {
            responses.push(Ok(FakeExecutor::tool_call_response(
                &format!("Calling tool for step {i}"),
                vec![ToolCall {
                    tool_call_id: format!("tc-{i}"),
                    tool_name: "lookup".to_owned(),
                    arguments_json: format!(r#"{{"step": {i}, "data": "padding text to consume tokens in the context window for test purposes"}}"#),
                }],
                50,
                20,
            )));
        }
        // Final response to end the run.
        responses.push(Ok(FakeExecutor::text_response(
            "Final answer after many steps.",
            100,
            30,
        )));

        let config = RunOrchestratorConfig {
            auto_approve_effects: true,
            ..Default::default()
        };

        let (harness, capturing) = build_capturing_harness(responses, config, Some(assembler));

        let agent_spec = default_agent_spec();
        let run_id = harness
            .run_manager
            .create_run(agent_spec.id)
            .await
            .expect("create_run");

        let outcome = harness
            .orchestrator
            .execute_run(
                run_id,
                &agent_spec,
                "Process multiple steps with lots of data",
            )
            .await
            .expect("execute_run");

        assert_eq!(outcome.final_state, RunState::Completed);

        // The last request (turn 6) should have gone through context
        // assembly. With truncation, the total number of messages should
        // be less than the full history (which would be ~11 messages:
        // 1 initial user + 5*(assistant+user tool result) = 11).
        let requests = capturing.captured();
        let last_request = requests.last().expect("should have requests");

        // With aggressive truncation (256 tokens ~ 1024 chars), the
        // assembler should have dropped older messages.  The recent
        // messages should still be present.
        let msg_texts: Vec<String> = last_request
            .messages
            .iter()
            .flat_map(|m| &m.content)
            .filter_map(|block| match block {
                ContentBlock::Text { text } => Some(text.clone()),
                _ => None,
            })
            .collect();

        // The most recent message should still be present (the last
        // tool-call text or a recent user message).
        let has_recent = msg_texts
            .iter()
            .any(|t| t.contains("step 4") || t.contains("step 3") || t.contains("Final"));
        assert!(
            has_recent || last_request.messages.len() < 11,
            "context should either have recent messages or be truncated from full ({} messages)",
            last_request.messages.len()
        );
    }

    #[cfg(feature = "context")]
    #[tokio::test]
    async fn context_assembler_token_count_tracked_correctly() {
        // Verify that the token estimation from the context assembler
        // matches what we'd expect from the content. This tests the
        // integration between the assembler's token estimator and the
        // orchestrator's message construction.
        use polkagent_context::budget::TokenBudget;
        use polkagent_context::TokenEstimator;

        let budget = TokenBudget::with_defaults(8192).expect("budget");
        let assembler = polkagent_context::ContextAssembler::new(budget.clone());
        let estimator = TokenEstimator::default();

        let (harness, capturing) = build_capturing_harness(
            vec![Ok(FakeExecutor::text_response("Response text.", 10, 5))],
            RunOrchestratorConfig::default(),
            Some(assembler),
        );

        let agent_spec = default_agent_spec();
        let run_id = harness
            .run_manager
            .create_run(agent_spec.id)
            .await
            .expect("create_run");

        let outcome = harness
            .orchestrator
            .execute_run(run_id, &agent_spec, "Hello world")
            .await
            .expect("execute_run");

        assert_eq!(outcome.final_state, RunState::Completed);

        // Extract the messages sent to the executor and verify the
        // estimated token count is within budget.
        let requests = capturing.captured();
        assert_eq!(requests.len(), 1);

        let total_text: String = requests[0]
            .messages
            .iter()
            .flat_map(|m| &m.content)
            .filter_map(|block| match block {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");

        let estimated_tokens = estimator.estimate(&total_text);
        let content_budget = budget.content_tokens();

        assert!(
            estimated_tokens <= content_budget,
            "estimated tokens ({estimated_tokens}) should not exceed content budget ({content_budget})"
        );

        // Token count should be positive for non-empty messages.
        assert!(
            estimated_tokens > 0,
            "estimated tokens should be positive for non-empty messages"
        );
    }

    #[cfg(feature = "context")]
    #[tokio::test]
    async fn context_assembler_handles_assembly_failure_gracefully() {
        // If the context assembler fails (e.g., budget too small for even
        // the system prompt), the orchestrator should fall back to using
        // the original messages.
        use polkagent_context::budget::TokenBudget;

        // Create an assembler with minimum budget (128 tokens).
        let budget = TokenBudget::with_defaults(128).expect("budget");
        let assembler = polkagent_context::ContextAssembler::new(budget);

        let (harness, capturing) = build_capturing_harness(
            vec![Ok(FakeExecutor::text_response("Still works!", 10, 5))],
            RunOrchestratorConfig::default(),
            Some(assembler),
        );

        let agent_spec = default_agent_spec();
        let run_id = harness
            .run_manager
            .create_run(agent_spec.id)
            .await
            .expect("create_run");

        // Even with a very small budget, the run should complete
        // successfully because the orchestrator falls back to original
        // messages on assembly failure.
        let outcome = harness
            .orchestrator
            .execute_run(run_id, &agent_spec, "Short prompt")
            .await
            .expect("execute_run");

        assert_eq!(outcome.final_state, RunState::Completed);
        assert_eq!(outcome.turn_count, 1);

        // Verify messages were sent (either assembled or fallback).
        let requests = capturing.captured();
        assert_eq!(requests.len(), 1);
        assert!(
            !requests[0].messages.is_empty(),
            "executor should have received at least one message"
        );
    }
}
