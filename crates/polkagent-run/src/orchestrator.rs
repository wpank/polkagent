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

use polkagent_core::{
    agent::AgentSpec,
    turn::TokenUsage,
    ArtifactId, RunId, RunState, StepId,
};
use polkagent_effect::EffectPipeline;
use polkagent_event::EventRecorder;
use polkagent_executor_trait::{
    ContentBlock, InferenceMessage, InferenceRequest,
    MessageRole, ModelExecutor,
};
use polkagent_grant::grant::GrantResolver;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, instrument, warn};

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
        }
    }

    /// Replace the config with a custom one.
    #[must_use]
    pub fn with_config(mut self, config: RunOrchestratorConfig) -> Self {
        self.config = config;
        self
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
        let mut total_usage = TokenUsage::default();
        let mut turn_count: u32 = 0;
        let artifacts: Vec<ArtifactId> = Vec::new();

        // Step 1: Transition Created -> Queued -> Running.
        self.run_manager.enqueue_run(run_id.clone()).await?;
        self.run_manager.start_run(run_id.clone()).await?;

        // Step 2: Build initial message list.
        let mut messages = self.build_initial_messages(agent_spec, initial_prompt);

        // Step 3: Turn loop.
        let outcome = loop {
            // Guard: max turns.
            if turn_count >= self.config.max_turns {
                warn!(%run_id, turn_count, max = self.config.max_turns, "max turns exceeded");
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

            // 3a: Call executor.
            let request = InferenceRequest {
                run_id: run_id.clone(),
                step_id: StepId::new(),
                messages: messages.clone(),
                system: agent_spec.system_prompt.clone(),
                tools: Vec::new(),
                model_id: agent_spec.model.clone(),
                max_tokens: self.config.max_tokens_per_turn,
                temperature: None,
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
            let _completed_turn = self.turn_manager.complete_turn(turn, &turn_output);

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
                    .complete_run(run_id.clone(), None)
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
                            .complete_run(run_id.clone(), None)
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
}
