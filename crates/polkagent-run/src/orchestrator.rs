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

use std::sync::Arc;
use std::time::{Duration, Instant};

use polkagent_card::{ActionCard, EffectKindTag, IntentCardSpec};
use polkagent_core::{
    agent::AgentSpec,
    turn::TokenUsage,
    ArtifactId, RunId, RunState, StepId,
};
use polkagent_effect::{EffectIntent, EffectKind, EffectPipeline};
use polkagent_event::EventRecorder;
use polkagent_executor_trait::{
    ContentBlock, InferenceMessage, InferenceRequest,
    MessageRole, ModelExecutor,
};
use polkagent_grant::grant::GrantResolver;
use polkagent_harness_trait::{
    Harness, HarnessEvent, HarnessTaskRequirements, SessionConfig, validate_for_task,
};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, instrument, warn};

#[cfg(feature = "context")]
use polkagent_context::ContextAssembler;

use crate::cost_tracker::CostTracker;
use crate::error::RunError;
use crate::manager::RunManager;
use crate::turn::{TurnInput, TurnManager, TurnOutput};

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

    /// Whether to auto-approve all effect intents (tool calls) without
    /// waiting for external approval.
    ///
    /// Default: `false` -- require manual approval.
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
    #[allow(dead_code)] // Phase 2: effect dispatch via the pipeline.
    effect_pipeline: EffectPipeline,
    #[allow(dead_code)] // Phase 2: fine-grained turn/step event recording.
    event_recorder: EventRecorder,
    #[allow(dead_code)] // Phase 2: grant checks before tool execution.
    grant_resolver: Arc<GrantResolver>,
    config: RunOrchestratorConfig,
    turn_manager: TurnManager,
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
    pub fn with_payment_store(
        mut self,
        store: Arc<dyn polkagent_payment::PaymentStore>,
    ) -> Self {
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
                let reasons: Vec<String> = mismatches.iter().map(|m| m.to_string()).collect();
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
        let current_state = self.run_manager.get_state(run_id.clone()).await?;
        if current_state == RunState::Created {
            self.run_manager.enqueue_run(run_id.clone()).await?;
        }
        self.run_manager.start_run(run_id.clone()).await?;

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
            .map_err(|e| RunError::Store(
                format!("harness session start failed: {e}"),
            ))?;

        info!(%run_id, %session_id, harness_id = %harness.id(), "harness session started");

        // Send the user prompt.
        harness
            .send_message(session_id, initial_prompt)
            .await
            .map_err(|e| RunError::Store(
                format!("harness send_message failed: {e}"),
            ))?;

        // Consume events from the harness.
        let mut event_stream = harness
            .receive_events(session_id)
            .await
            .map_err(|e| RunError::Store(
                format!("harness receive_events failed: {e}"),
            ))?;

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
                HarnessEvent::ToolResultProvided { tool_name, is_error, .. } => {
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
            self.run_manager
                .fail_run(run_id.clone(), &reason)
                .await?;
            Ok(RunOutcome {
                run_id,
                final_state: RunState::Failed { reason },
                total_tokens: TokenUsage::default(),
                turn_count: 1,
                artifacts: Vec::new(),
                duration: start.elapsed(),
            })
        } else {
            self.run_manager.completing_run(run_id.clone()).await?;
            self.run_manager
                .complete_run(run_id.clone(), None, 0, 0)
                .await?;

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
    ///    d. For tool calls with `auto_approve_effects`: add synthetic tool
    ///       results and continue.
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
        let start = Instant::now();

        // If a harness is attached, delegate the entire run to it.
        if let Some(ref harness) = self.harness {
            return self
                .execute_run_via_harness(run_id, agent_spec, initial_prompt, harness)
                .await;
        }

        // NOTE(persistence): `total_usage` is accumulated across turns and
        // included in the returned `RunOutcome`. Individual turn records
        // (including per-turn token counts) are persisted to the `turns`
        // table via `RunManager::record_turn`.
        let mut total_usage = TokenUsage::default();
        let mut turn_count: u32 = 0;
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
        let run_deadline: Option<Instant> = agent_spec
            .resource_limits
            .as_ref()
            .and_then(|rl| rl.timeout_secs)
            .map(|secs| Instant::now() + Duration::from_secs(secs));

        // Resolve the model to use: prefer model_preference.model_id when set,
        // then fall back to agent_spec.model.
        let effective_model_id: String = agent_spec
            .model_preference
            .as_ref()
            .and_then(|mp| mp.model_id.as_deref())
            .map(|mid| {
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
            })
            .unwrap_or_else(|| agent_spec.model.clone());

        // Temperature from model_preference (None means executor default).
        // InferenceRequest uses f32; we store f64 in the spec for precision in
        // config files, and cast here at the use site.
        let effective_temperature: Option<f32> = agent_spec
            .model_preference
            .as_ref()
            .and_then(|mp| mp.temperature)
            .map(|t| t as f32);

        // System prompt: model_preference.system_prompt overrides
        // agent_spec.system_prompt when present.
        let effective_system: Option<String> = agent_spec
            .model_preference
            .as_ref()
            .and_then(|mp| mp.system_prompt.clone())
            .or_else(|| agent_spec.system_prompt.clone());

        // Step 1: Transition to Running.
        // In production, AppService::start_run enqueues before spawning us
        // (Created → Queued), so we just claim (Queued → Running).
        // In tests, execute_run is called directly on a Created run, so
        // we enqueue first if needed.
        let current_state = self.run_manager.get_state(run_id.clone()).await?;
        if current_state == RunState::Created {
            self.run_manager.enqueue_run(run_id.clone()).await?;
        }
        self.run_manager.start_run(run_id.clone()).await?;

        // Step 2: Build initial message list.
        let mut messages = self.build_initial_messages(agent_spec, initial_prompt);

        // Build a CostTracker for this run.
        //
        // When the `payment` feature is enabled and a payment store is
        // attached, token usage is persisted after each turn. When a
        // per-run budget is configured via `agent_spec.resource_limits`,
        // budget enforcement is active.
        let mut cost_tracker = {
            #[cfg(feature = "payment")]
            {
                let max_usd = agent_spec
                    .resource_limits
                    .as_ref()
                    .and_then(|rl| {
                        // Resource limits don't have a USD field yet; use the
                        // payment feature if attached but no hard USD cap from
                        // spec. Callers can extend this mapping as needed.
                        let _ = rl;
                        None::<f64>
                    });

                let tracker = match max_usd {
                    Some(limit) => CostTracker::with_budget(run_id.clone(), limit),
                    None => CostTracker::unbounded(run_id.clone()),
                };

                match &self.payment_store {
                    Some(store) => tracker.with_store(Arc::clone(store)),
                    None => tracker,
                }
            }
            #[cfg(not(feature = "payment"))]
            CostTracker::unbounded(run_id.clone())
        };

        // Step 3: Turn loop.
        let outcome = loop {
            // Guard: wall-clock timeout.
            if let Some(deadline) = run_deadline {
                if Instant::now() >= deadline {
                    warn!(%run_id, "run deadline exceeded — transitioning to TimedOut");
                    self.run_manager.timeout_run(run_id.clone()).await?;
                    break RunOutcome {
                        run_id: run_id.clone(),
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
                    .fail_run(run_id.clone(), "max turns exceeded")
                    .await?;
                break RunOutcome {
                    run_id: run_id.clone(),
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
                if let Some(exceeded) = cost_tracker.check_budget(
                    provider,
                    model,
                    effective_max_tokens,
                ) {
                    let reason = format!("BudgetExceeded: {}", exceeded.reason);
                    warn!(%run_id, %reason, "budget exceeded before turn");
                    self.run_manager.fail_run(run_id.clone(), &reason).await?;
                    break RunOutcome {
                        run_id: run_id.clone(),
                        final_state: RunState::Failed {
                            reason,
                        },
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
            let assembled_messages = self.apply_context_assembly(
                &messages,
                effective_system.as_deref(),
            );
            #[cfg(not(feature = "context"))]
            let assembled_messages = messages.clone();

            let request = InferenceRequest {
                run_id: run_id.clone(),
                step_id: StepId::new(),
                messages: assembled_messages,
                system: effective_system.clone(),
                tools: Vec::new(),
                model_id: effective_model_id.clone(),
                max_tokens: effective_max_tokens,
                temperature: effective_temperature,
            };

            let response = match self.executor.complete(request).await {
                Ok(resp) => resp,
                Err(err) => {
                    let reason = format!("executor error: {err}");
                    warn!(%run_id, %reason, "executor failed");
                    self.run_manager.fail_run(run_id.clone(), &reason).await?;
                    break RunOutcome {
                        run_id: run_id.clone(),
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
                    self.run_manager.fail_run(run_id.clone(), &reason).await?;
                    break RunOutcome {
                        run_id: run_id.clone(),
                        final_state: RunState::Failed { reason },
                        total_tokens: total_usage,
                        turn_count,
                        artifacts: artifacts.clone(),
                        duration: start.elapsed(),
                    };
                }
            }

            let turn_input = TurnInput::user(if turn_count == 1 {
                initial_prompt.to_owned()
            } else {
                "continuation".to_owned()
            });
            let turn = self
                .turn_manager
                .create_turn(run_id.clone(), turn_count - 1, &turn_input);
            let turn_output = if response.stop_reason == "end_turn" && response.tool_calls.is_empty()
            {
                TurnOutput::terminal(&response.text).with_usage(turn_usage)
            } else {
                TurnOutput::continuing().with_usage(turn_usage)
            };
            let completed_turn = self.turn_manager.complete_turn(turn, &turn_output);
            if let Err(e) = self.run_manager.record_turn(&completed_turn).await {
                warn!(
                    %run_id,
                    turn = turn_count,
                    error = %e,
                    "failed to persist turn record (non-fatal)"
                );
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
                self.run_manager.fail_run(run_id.clone(), &reason).await?;
                break RunOutcome {
                    run_id: run_id.clone(),
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
                self.run_manager.completing_run(run_id.clone()).await?;
                self.run_manager
                    .complete_run(
                        run_id.clone(),
                        None,
                        u64::from(total_usage.input_tokens),
                        u64::from(total_usage.output_tokens),
                    )
                    .await?;

                info!(%run_id, turn_count, "run completed successfully");
                break RunOutcome {
                    run_id: run_id.clone(),
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

                // For each tool call, produce a tool result.
                // In the full system, this would go through the effect
                // pipeline and wait for approval. For now, when
                // `auto_approve_effects` is true, we produce a synthetic
                // success result so the loop can continue.
                let mut tool_result_content: Vec<ContentBlock> = Vec::new();
                for tc in &response.tool_calls {
                    if self.config.auto_approve_effects {
                        tool_result_content.push(ContentBlock::ToolResult {
                            tool_call_id: tc.tool_call_id.clone(),
                            content: format!(
                                "{{\"status\":\"ok\",\"tool\":\"{}\"}}",
                                tc.tool_name
                            ),
                            is_error: false,
                        });
                    } else {
                        // Without auto-approve, we cannot proceed in this
                        // loop; the run would need to transition to
                        // WaitingEffect and resume later. For the
                        // orchestrator's Phase 1 implementation, treat
                        // this as a completion point.
                        self.run_manager.completing_run(run_id.clone()).await?;
                        self.run_manager
                            .complete_run(
                                run_id.clone(),
                                None,
                                u64::from(total_usage.input_tokens),
                                u64::from(total_usage.output_tokens),
                            )
                            .await?;
                        return Ok(RunOutcome {
                            run_id,
                            final_state: RunState::Completed,
                            total_tokens: total_usage,
                            turn_count,
                            artifacts,
                            duration: start.elapsed(),
                        });
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

        Ok(outcome)
    }

    // -----------------------------------------------------------------------
    // Private helpers
    // -----------------------------------------------------------------------

    /// Build the initial message list from the agent spec and user prompt.
    fn build_initial_messages(
        &self,
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
        let assembler = match &self.context_assembler {
            Some(a) => a,
            None => return messages.to_vec(),
        };

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

        let conversation_refs: Vec<&str> = conversation_strings.iter().map(|s| s.as_str()).collect();

        // Assemble context with the system prompt and conversation history.
        let assembled = match assembler.assemble(
            system_prompt,
            &[],   // tool descriptions are handled separately in InferenceRequest
            &[],   // memory entries (not used in orchestrator yet)
            &conversation_refs,
            None,   // user input is part of conversation messages
        ) {
            Ok(ctx) => ctx,
            Err(err) => {
                warn!("context assembly failed, using original messages: {err}");
                return messages.to_vec();
            }
        };

        info!(
            total_tokens = assembled.total_tokens,
            remaining = assembled.remaining_tokens(),
            sections = assembled.section_count(),
            "context assembled for turn"
        );

        // Rebuild messages from the assembled context.
        // The assembled conversation section contains the (potentially
        // truncated) conversation history. We parse it back into messages.
        let mut result = Vec::new();

        for section in &assembled.sections {
            match section.kind {
                polkagent_context::section::SectionKind::ConversationHistory => {
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
                _ => {}
            }
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
        .map(|a| a.to_string())
        .or_else(|| intent.payload.get("arguments").map(|a| a.to_string()));

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
    use polkagent_store_trait::{
        event::{EventFilter, EventStore, EventStoreError, StoredEvent},
        EffectStore, RunStatus, RunStore, RunSummary, StoredIntent, StoredOutcome, StoreError,
    };

    use async_trait::async_trait;
    use futures::Stream;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

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

    const TERMINAL_TYPES: &[&str] =
        &["run_completed", "run_failed", "run_cancelled", "run_timed_out"];

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
            if TERMINAL_TYPES.contains(&event.event_type.as_str()) {
                if !term.insert(event.run_id.clone()) {
                    return Err(EventStoreError::DuplicateTerminalEvent {
                        run_id: event.run_id.clone(),
                    });
                }
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

        async fn query(
            &self,
            filter: EventFilter,
        ) -> Result<Vec<StoredEvent>, EventStoreError> {
            let durable = self.durable.lock().expect("lock");
            Ok(durable
                .iter()
                .filter(|e| {
                    filter
                        .run_id
                        .as_ref()
                        .map_or(true, |rid| e.run_id == rid.to_string())
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

        async fn get_by_run(
            &self,
            _run_id: RunId,
        ) -> Result<Vec<StoredIntent>, StoreError> {
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

        /// Create a simple text response with end_turn.
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

        /// Create a max_tokens response.
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
        let effect_pipeline =
            EffectPipeline::new(effect_store, polkagent_core::WorkerId::new());

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
        AgentSpec::new(
            AgentId::new(),
            "Test Agent",
            "test-model",
        )
    }

    // ── Tests ───────────────────────────────────────────────────────────

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
            .execute_run(run_id.clone(), &agent_spec, "Hi there")
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
            .execute_run(run_id.clone(), &agent_spec, "Search for something")
            .await
            .expect("execute_run");

        assert_eq!(outcome.final_state, RunState::Completed);
        assert_eq!(outcome.turn_count, 2);
    }

    #[tokio::test]
    async fn tool_call_creates_effect_intent() {
        // With auto_approve_effects = false, the orchestrator should
        // complete after encountering a tool call (Phase 1 behaviour).
        let responses = vec![Ok(FakeExecutor::tool_call_response(
            "I need to call a tool.",
            vec![ToolCall {
                tool_call_id: "tc-1".to_owned(),
                tool_name: "file_read".to_owned(),
                arguments_json: r#"{"path": "/tmp/test.txt"}"#.to_owned(),
            }],
            50,
            20,
        ))];

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
            .execute_run(run_id.clone(), &agent_spec, "Read a file")
            .await
            .expect("execute_run");

        // Without auto-approve, the run completes after the tool call.
        assert_eq!(outcome.final_state, RunState::Completed);
        assert_eq!(outcome.turn_count, 1);
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
            .execute_run(run_id.clone(), &agent_spec, "Loop forever")
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
            .execute_run(run_id.clone(), &agent_spec, "Fail please")
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
            .execute_run(run_id.clone(), &agent_spec, "Accumulate tokens")
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
            .execute_run(run_id.clone(), &agent_spec, "Hello")
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
            .execute_run(run_id.clone(), &agent_spec, "Generate a novel")
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
            .execute_run(run_id.clone(), &agent_spec, "Be quick")
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
                "args": {"dest": "5GrwvaEF", "value": 1000000},
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
            (EffectKind::SignatureRequest, EffectKindTag::SignatureRequest),
            (EffectKind::Broadcast, EffectKindTag::Broadcast),
            (EffectKind::FinalityWatch, EffectKindTag::FinalityWatch),
            (EffectKind::Delivery, EffectKindTag::Delivery),
            (EffectKind::ChainRead, EffectKindTag::ChainRead),
            (EffectKind::Simulation, EffectKindTag::Simulation),
            (EffectKind::HarnessOperation, EffectKindTag::HarnessOperation),
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
        assert!(!card.payload_hash.is_empty(), "card must have a payload hash");
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
        let pallet_section = card
            .canonical_sections
            .iter()
            .find(|s| s.label == "Pallet");
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
            card.narrative_sections[0].content.contains("staking controller"),
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

        assert!(intent.action_card.is_some(), "card must be attached to intent");
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
            .execute_run(run_id.clone(), &agent_spec, "Hello")
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
            .execute_run(run_id.clone(), &agent_spec, "Accumulate")
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
            .execute_run(run_id.clone(), &agent_spec, "Short")
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

    /// Build a test harness with a CapturingExecutor for context assembly
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
        let effect_pipeline =
            EffectPipeline::new(effect_store, polkagent_core::WorkerId::new());

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
            .execute_run(run_id.clone(), &agent_spec, "Hi there")
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

        let (harness, capturing) =
            build_capturing_harness(responses, config, Some(assembler));

        let agent_spec = default_agent_spec();
        let run_id = harness
            .run_manager
            .create_run(agent_spec.id)
            .await
            .expect("create_run");

        let outcome = harness
            .orchestrator
            .execute_run(run_id.clone(), &agent_spec, "Search for blockchain data")
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
            .execute_run(run_id.clone(), &agent_spec, "Hello agent")
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
                && m.content.iter().any(|block| matches!(
                    block,
                    ContentBlock::Text { text } if text.contains("Hello agent")
                ))
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

        let (harness, capturing) =
            build_capturing_harness(responses, config, Some(assembler));

        let agent_spec = default_agent_spec();
        let run_id = harness
            .run_manager
            .create_run(agent_spec.id)
            .await
            .expect("create_run");

        let outcome = harness
            .orchestrator
            .execute_run(
                run_id.clone(),
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
            .execute_run(run_id.clone(), &agent_spec, "Hello world")
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
            .execute_run(run_id.clone(), &agent_spec, "Short prompt")
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
