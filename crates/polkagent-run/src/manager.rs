//! `RunManager` — the central coordinator for run lifecycle operations.
//!
//! The manager is the single entry-point that the API layer (and any other
//! caller) uses to create, start, query, and cancel runs. It:
//!
//! 1. Validates every state transition via [`RunStateMachine`].
//! 2. Persists the new state to the [`RunStore`].
//! 3. Records a corresponding [`RunEvent`] via the [`EventRecorder`].
//!
//! It is deliberately thin — no business logic lives here. The state machine
//! owns the transition rules; the store and recorder own persistence.
//!
//! # Concurrency
//!
//! `RunManager` is `Clone + Send + Sync`. It holds only `Arc`-backed state so
//! it can be freely shared across Tokio tasks. Concurrent writes for the same
//! run are serialised by the store's atomic `update_state` contract.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use polkagent_core::{
    event::{EventCorrelation, EventKind, RunEvent},
    turn::Turn,
    AgentId, EventId, RunId, RunState,
};
use polkagent_event::EventRecorder;
use polkagent_store_trait::{RunStatus, RunStore};
use tracing::{info, instrument, warn};

use crate::{
    error::RunError,
    state_machine::{RunStateMachine, RunTransition},
};

// ---------------------------------------------------------------------------
// RunManager
// ---------------------------------------------------------------------------

/// Manages the full lifecycle of runs.
///
/// Constructed with a [`RunStore`] for persistence and an [`EventRecorder`]
/// for event sourcing. All state-changing methods validate the transition,
/// update the store, and emit the appropriate event.
///
/// # Clone semantics
///
/// `RunManager` is cheaply cloneable (Arc-backed). All clones share the same
/// store and recorder.
#[derive(Clone)]
pub struct RunManager {
    store: Arc<dyn RunStore>,
    events: EventRecorder,
    machine: RunStateMachine,
    /// Per-run sequence counters. Each run gets a monotonically increasing
    /// sequence number for its events. The `Mutex` is held only briefly to
    /// read-and-increment, so contention is negligible.
    sequences: Arc<Mutex<HashMap<String, u64>>>,
}

impl std::fmt::Debug for RunManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunManager").finish_non_exhaustive()
    }
}

impl RunManager {
    /// Create a new manager backed by the given store and event recorder.
    #[must_use]
    pub fn new(store: Arc<dyn RunStore>, events: EventRecorder) -> Self {
        Self {
            store,
            events,
            machine: RunStateMachine::new(),
            sequences: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    // -----------------------------------------------------------------------
    // Run lifecycle methods
    // -----------------------------------------------------------------------

    /// Create a new run for `agent_id` in the `Created` state.
    ///
    /// Persists the run to the store and emits a [`RunCreated`] event.
    ///
    /// # Errors
    ///
    /// Returns [`RunError::Store`] if the store write fails, or
    /// [`RunError::Event`] if event recording fails.
    #[instrument(skip(self), fields(agent_id = %agent_id))]
    pub async fn create_run(&self, agent_id: AgentId) -> Result<RunId, RunError> {
        let run_id = RunId::new();
        let status = RunStatus::new(RunState::Created.to_string());

        let agent_id_str = agent_id.to_string();
        self.store
            .create(run_id.clone(), &agent_id_str, status)
            .await
            .map_err(|e| RunError::Store(e.to_string()))?;

        self.emit_event(run_id.clone(), EventKind::RunCreated).await?;

        info!(%run_id, "run created");
        Ok(run_id)
    }

    /// Enqueue a run: transition `Created → Queued`.
    ///
    /// # Errors
    ///
    /// Returns [`RunError::Transition`] if the run is not in `Created` state,
    /// [`RunError::NotFound`] if the run does not exist, or [`RunError::Store`]
    /// / [`RunError::Event`] for persistence failures.
    #[instrument(skip(self), fields(run_id = %run_id))]
    pub async fn enqueue_run(&self, run_id: RunId) -> Result<(), RunError> {
        let current = self.current_state(run_id.clone()).await?;
        let next = self.machine.transition(&current, RunTransition::Start)?;

        self.apply_transition(run_id.clone(), next, EventKind::RunQueued)
            .await?;

        info!(%run_id, "run enqueued");
        Ok(())
    }

    /// Start a run: transition `Queued → Running`.
    ///
    /// # Errors
    ///
    /// Returns [`RunError::Transition`] if the run is not in `Queued` state.
    #[instrument(skip(self), fields(run_id = %run_id))]
    pub async fn start_run(&self, run_id: RunId) -> Result<(), RunError> {
        let current = self.current_state(run_id.clone()).await?;
        let next = self.machine.transition(&current, RunTransition::WorkerClaimed)?;

        self.apply_transition(run_id.clone(), next, EventKind::RunStarted)
            .await?;

        info!(%run_id, "run started");
        Ok(())
    }

    /// Move a run to `Completing` state (final turn finished, artifacts being
    /// written): transition `Running → Completing`.
    ///
    /// # Errors
    ///
    /// Returns [`RunError::Transition`] if the run is not in `Running` state.
    #[instrument(skip(self), fields(run_id = %run_id))]
    pub async fn completing_run(&self, run_id: RunId) -> Result<(), RunError> {
        let current = self.current_state(run_id.clone()).await?;
        let next = self.machine.transition(&current, RunTransition::Complete)?;

        self.apply_transition(run_id.clone(), next, EventKind::RunCompleting)
            .await?;

        info!(%run_id, "run completing");
        Ok(())
    }

    /// Complete a run: transition `Completing → Completed`.
    ///
    /// # Errors
    ///
    /// Returns [`RunError::Transition`] if the run is not in `Completing` state.
    #[instrument(skip(self), fields(run_id = %run_id))]
    pub async fn complete_run(
        &self,
        run_id: RunId,
        output_artifact_id: Option<polkagent_core::ArtifactId>,
    ) -> Result<(), RunError> {
        let current = self.current_state(run_id.clone()).await?;
        let next = self.machine.transition(&current, RunTransition::Complete)?;

        self.apply_transition(
            run_id.clone(),
            next,
            EventKind::RunCompleted { output_artifact_id },
        )
        .await?;

        info!(%run_id, "run completed");
        Ok(())
    }

    /// Fail a run: transition any non-terminal state to `Failed`.
    ///
    /// # Errors
    ///
    /// Returns [`RunError::Transition`] if the run is already terminal.
    #[instrument(skip(self), fields(run_id = %run_id, %reason))]
    pub async fn fail_run(&self, run_id: RunId, reason: &str) -> Result<(), RunError> {
        let reason = reason.to_owned();
        let current = self.current_state(run_id.clone()).await?;
        let next = self
            .machine
            .transition(&current, RunTransition::Fail(reason.clone()))?;

        self.apply_transition(
            run_id.clone(),
            next,
            EventKind::RunFailed {
                reason: reason.clone(),
            },
        )
        .await?;

        warn!(%run_id, %reason, "run failed");
        Ok(())
    }

    /// Cancel a run: transition any non-terminal state to `Cancelled`.
    ///
    /// # Errors
    ///
    /// Returns [`RunError::Transition`] if the run is already terminal.
    #[instrument(skip(self), fields(run_id = %run_id, %reason))]
    pub async fn cancel_run(&self, run_id: RunId, reason: &str) -> Result<(), RunError> {
        let reason = reason.to_owned();
        let current = self.current_state(run_id.clone()).await?;
        let next = self
            .machine
            .transition(&current, RunTransition::Cancel(reason.clone()))?;

        self.apply_transition(
            run_id.clone(),
            next,
            EventKind::RunCancelled {
                reason: reason.clone(),
            },
        )
        .await?;

        info!(%run_id, %reason, "run cancelled");
        Ok(())
    }

    /// Time out a run: transition any non-terminal state to `TimedOut`.
    ///
    /// # Errors
    ///
    /// Returns [`RunError::Transition`] if the run is already terminal.
    #[instrument(skip(self), fields(run_id = %run_id))]
    pub async fn timeout_run(&self, run_id: RunId) -> Result<(), RunError> {
        let current = self.current_state(run_id.clone()).await?;
        let next = self.machine.transition(&current, RunTransition::Timeout)?;

        self.apply_transition(run_id.clone(), next, EventKind::RunTimedOut)
            .await?;

        warn!(%run_id, "run timed out");
        Ok(())
    }

    /// Request approval: transition `Running → AwaitingApproval`.
    ///
    /// # Errors
    ///
    /// Returns [`RunError::Transition`] if the run is not in `Running` state.
    #[instrument(skip(self), fields(run_id = %run_id, %request_id))]
    pub async fn request_approval(&self, run_id: RunId, request_id: &str) -> Result<(), RunError> {
        let request_id = request_id.to_owned();
        let current = self.current_state(run_id.clone()).await?;
        let next = self
            .machine
            .transition(&current, RunTransition::RequestApproval)?;

        self.apply_transition(
            run_id.clone(),
            next,
            EventKind::ApprovalRequested {
                request_id: request_id.clone(),
            },
        )
        .await?;

        info!(%run_id, %request_id, "approval requested");
        Ok(())
    }

    /// Grant approval: transition `AwaitingApproval → Running`.
    ///
    /// # Errors
    ///
    /// Returns [`RunError::Transition`] if the run is not in `AwaitingApproval`.
    #[instrument(skip(self), fields(run_id = %run_id, %approval_id))]
    pub async fn grant_approval(&self, run_id: RunId, approval_id: &str) -> Result<(), RunError> {
        let approval_id = approval_id.to_owned();
        let current = self.current_state(run_id.clone()).await?;
        let next = self
            .machine
            .transition(&current, RunTransition::GrantApproval)?;

        self.apply_transition(
            run_id.clone(),
            next,
            EventKind::ApprovalGranted {
                approval_id: approval_id.clone(),
            },
        )
        .await?;

        info!(%run_id, %approval_id, "approval granted");
        Ok(())
    }

    /// Deny approval: transition `AwaitingApproval → Cancelled`.
    ///
    /// # Errors
    ///
    /// Returns [`RunError::Transition`] if the run is not in `AwaitingApproval`.
    #[instrument(skip(self), fields(run_id = %run_id, %reason))]
    pub async fn deny_approval(&self, run_id: RunId, reason: &str) -> Result<(), RunError> {
        let reason = reason.to_owned();
        let current = self.current_state(run_id.clone()).await?;
        let next = self
            .machine
            .transition(&current, RunTransition::DenyApproval(reason.clone()))?;

        self.apply_transition(
            run_id.clone(),
            next,
            EventKind::ApprovalDenied {
                reason: reason.clone(),
            },
        )
        .await?;

        info!(%run_id, %reason, "approval denied");
        Ok(())
    }

    /// Retrieve the current run state from the store.
    ///
    /// # Errors
    ///
    /// Returns [`RunError::NotFound`] if the run does not exist, or
    /// [`RunError::Store`] for other store errors.
    pub async fn get_state(&self, run_id: RunId) -> Result<RunState, RunError> {
        self.current_state(run_id).await
    }

    /// Persist a completed [`Turn`] (including token usage) to the store.
    ///
    /// The turn's `sequence` is stored as 1-based in the database (the
    /// schema uses 1-based sequences). Callers should pass the turn exactly
    /// as returned by [`TurnManager::complete_turn`](crate::turn::TurnManager::complete_turn).
    ///
    /// # Errors
    ///
    /// Returns [`RunError::Store`] if the store write fails.
    #[instrument(skip(self, turn), fields(turn_id = %turn.id, run_id = %turn.run_id))]
    pub async fn record_turn(&self, turn: &Turn) -> Result<(), RunError> {
        let role = match turn.role {
            polkagent_core::turn::MessageRole::User => "user",
            polkagent_core::turn::MessageRole::Assistant => "assistant",
            polkagent_core::turn::MessageRole::System => "system",
        };

        let started_at = turn.started_at.to_rfc3339();
        let completed_at = turn.completed_at.map(|t| t.to_rfc3339());

        // The schema uses 1-based sequences.
        let sequence = turn.sequence.saturating_add(1);

        self.store
            .insert_turn(
                turn.id,
                turn.run_id,
                sequence,
                role,
                &started_at,
                completed_at.as_deref(),
                turn.token_usage.input_tokens,
                turn.token_usage.output_tokens,
            )
            .await
            .map_err(|e| RunError::Store(e.to_string()))?;

        info!(
            turn_id = %turn.id,
            run_id = %turn.run_id,
            sequence,
            input_tokens = turn.token_usage.input_tokens,
            output_tokens = turn.token_usage.output_tokens,
            "turn persisted"
        );

        Ok(())
    }

    // -----------------------------------------------------------------------
    // Private helpers
    // -----------------------------------------------------------------------

    /// Fetch the current state string from the store and parse it into
    /// [`RunState`].
    async fn current_state(&self, run_id: RunId) -> Result<RunState, RunError> {
        let summary = self
            .store
            .get(run_id.clone())
            .await
            .map_err(|e| match e {
                polkagent_store_trait::StoreError::NotFound { .. } => {
                    RunError::NotFound(run_id.clone())
                }
                other => RunError::Store(other.to_string()),
            })?;

        // Parse the RunStatus string back into a RunState.
        parse_run_state(summary.status.as_str()).map_err(|e| RunError::Store(e))
    }

    /// Atomically apply a state transition:
    /// 1. Update the store.
    /// 2. Record the event.
    async fn apply_transition(
        &self,
        run_id: RunId,
        new_state: RunState,
        kind: EventKind,
    ) -> Result<(), RunError> {
        let status = RunStatus::new(new_state.to_string());

        self.store
            .update_state(run_id.clone(), status)
            .await
            .map_err(|e| RunError::Store(e.to_string()))?;

        self.emit_event(run_id, kind).await?;

        Ok(())
    }

    /// Build a minimal [`RunEvent`] and record it via the [`EventRecorder`].
    async fn emit_event(&self, run_id: RunId, kind: EventKind) -> Result<(), RunError> {
        let sequence = {
            let mut seqs = self.sequences.lock().expect("sequence lock poisoned");
            let entry = seqs.entry(run_id.to_string()).or_insert(0);
            *entry += 1;
            *entry
        };

        let correlation = EventCorrelation {
            run_id: run_id.clone(),
            ..Default::default()
        };
        let event = RunEvent::new_durable(EventId::new(), run_id, sequence, kind, correlation);

        self.events
            .record(event)
            .await
            .map_err(|e| RunError::Event(e.to_string()))?;

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// State serialisation helpers
// ---------------------------------------------------------------------------

/// Parse a `RunStatus` string (as persisted by the store) back to a
/// [`RunState`].
///
/// The store layer uses the `Display` representation of `RunState`, which is a
/// lowercase tag string (e.g. `"running"`, `"failed:some reason"`).
fn parse_run_state(s: &str) -> Result<RunState, String> {
    // Handle tagged states first (states with extra data separated by ':').
    if let Some(reason) = s.strip_prefix("failed:") {
        return Ok(RunState::Failed {
            reason: reason.to_owned(),
        });
    }
    if let Some(reason) = s.strip_prefix("cancelled:") {
        return Ok(RunState::Cancelled {
            reason: reason.to_owned(),
        });
    }

    match s {
        "created" => Ok(RunState::Created),
        "queued" => Ok(RunState::Queued),
        "running" => Ok(RunState::Running),
        "completing" => Ok(RunState::Completing),
        "completed" => Ok(RunState::Completed),
        "timed_out" => Ok(RunState::TimedOut),
        // AwaitingApproval and WaitingEffect can't fully round-trip through the
        // plain string without extra fields. Represent them as Running when
        // fetched from the store (the event log is the authoritative source of
        // truth for the full state). In a real system the store would store the
        // full JSON-serialised state.
        s if s.starts_with("awaiting_approval") => Ok(RunState::AwaitingApproval {
            request_id: String::new(),
        }),
        s if s.starts_with("waiting_effect") => Ok(RunState::WaitingEffect {
            pending_intent_ids: vec![],
        }),
        other => Err(format!("unknown run state string: {other}")),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use polkagent_core::RunId;
    use polkagent_event::{EventBus, EventRecorder};
    use polkagent_store_trait::{
        event::{EventFilter, EventStore, EventStoreError, StoredEvent},
        RunStatus, RunStore, RunSummary, StoreError,
    };
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    // ── In-memory RunStore ─────────────────────────────────────────────────

    #[derive(Debug, Default)]
    struct MemRunStore {
        runs: Mutex<HashMap<String, RunSummary>>,
    }

    #[async_trait::async_trait]
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
            runs.sort_by(|a, b| b.created_at.cmp(&a.created_at));
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
            runs.sort_by(|a, b| b.created_at.cmp(&a.created_at));
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
            // In-memory test store: no-op — turns are not queried in manager tests.
            Ok(())
        }
    }

    // ── In-memory EventStore ───────────────────────────────────────────────

    #[derive(Debug, Default)]
    struct MemEventStore {
        durable: Mutex<Vec<StoredEvent>>,
        sequences: Mutex<HashMap<String, u64>>,
        terminal: Mutex<std::collections::HashSet<String>>,
    }

    const TERMINAL_TYPES: &[&str] =
        &["run_completed", "run_failed", "run_cancelled", "run_timed_out"];

    #[async_trait::async_trait]
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

    // ── Test helpers ────────────────────────────────────────────────────────

    fn make_manager() -> (RunManager, Arc<MemEventStore>) {
        let run_store: Arc<dyn RunStore> = Arc::new(MemRunStore::default());
        let event_store = Arc::new(MemEventStore::default());
        let event_store_dyn = Arc::clone(&event_store) as Arc<dyn EventStore>;
        let bus = EventBus::new(64);
        let recorder = EventRecorder::new(event_store_dyn, bus);
        let manager = RunManager::new(run_store, recorder);
        (manager, event_store)
    }

    // ── Tests ───────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn create_run_returns_new_run_id() {
        let (mgr, _) = make_manager();
        let agent_id = AgentId::new();
        let run_id = mgr.create_run(agent_id).await.expect("create_run");
        let state = mgr.get_state(run_id.clone()).await.expect("get_state");
        assert_eq!(state, RunState::Created);
    }

    #[tokio::test]
    async fn create_run_emits_run_created_event() {
        let (mgr, events) = make_manager();
        let agent_id = AgentId::new();
        let run_id = mgr.create_run(agent_id).await.expect("create_run");

        let stored = events.read_run_events(run_id).await.expect("read_run_events");
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].event_type, "run_created");
    }

    #[tokio::test]
    async fn full_happy_path_created_to_completed() {
        let (mgr, events) = make_manager();
        let agent_id = AgentId::new();

        let run_id = mgr.create_run(agent_id).await.expect("create");
        mgr.enqueue_run(run_id.clone()).await.expect("enqueue");
        mgr.start_run(run_id.clone()).await.expect("start");
        mgr.completing_run(run_id.clone()).await.expect("completing");
        mgr.complete_run(run_id.clone(), None).await.expect("complete");

        let state = mgr.get_state(run_id.clone()).await.expect("get_state");
        assert_eq!(state, RunState::Completed);

        let stored = events.read_run_events(run_id).await.expect("read");
        let event_types: Vec<&str> = stored.iter().map(|e| e.event_type.as_str()).collect();
        assert_eq!(
            event_types,
            &["run_created", "run_queued", "run_started", "run_completing", "run_completed"]
        );
    }

    #[tokio::test]
    async fn cancel_run_transitions_to_cancelled() {
        let (mgr, _) = make_manager();
        let agent_id = AgentId::new();
        let run_id = mgr.create_run(agent_id).await.expect("create");
        mgr.enqueue_run(run_id.clone()).await.expect("enqueue");
        mgr.start_run(run_id.clone()).await.expect("start");
        mgr.cancel_run(run_id.clone(), "user request")
            .await
            .expect("cancel");

        let state = mgr.get_state(run_id).await.expect("get_state");
        assert!(matches!(state, RunState::Cancelled { .. }));
    }

    #[tokio::test]
    async fn fail_run_transitions_to_failed() {
        let (mgr, _) = make_manager();
        let agent_id = AgentId::new();
        let run_id = mgr.create_run(agent_id).await.expect("create");
        mgr.enqueue_run(run_id.clone()).await.expect("enqueue");
        mgr.start_run(run_id.clone()).await.expect("start");
        mgr.fail_run(run_id.clone(), "provider error")
            .await
            .expect("fail");

        let state = mgr.get_state(run_id).await.expect("get_state");
        assert!(matches!(state, RunState::Failed { .. }));
    }

    #[tokio::test]
    async fn timeout_run_transitions_to_timed_out() {
        let (mgr, _) = make_manager();
        let agent_id = AgentId::new();
        let run_id = mgr.create_run(agent_id).await.expect("create");
        mgr.enqueue_run(run_id.clone()).await.expect("enqueue");
        mgr.start_run(run_id.clone()).await.expect("start");
        mgr.timeout_run(run_id.clone()).await.expect("timeout");

        let state = mgr.get_state(run_id).await.expect("get_state");
        assert_eq!(state, RunState::TimedOut);
    }

    #[tokio::test]
    async fn invalid_transition_returns_error() {
        let (mgr, _) = make_manager();
        let agent_id = AgentId::new();
        let run_id = mgr.create_run(agent_id).await.expect("create");

        // Try to start a run that is still in Created (must enqueue first).
        let err = mgr.start_run(run_id).await.unwrap_err();
        assert!(matches!(err, RunError::Transition(_)));
    }

    #[tokio::test]
    async fn cancel_terminal_run_returns_error() {
        let (mgr, _) = make_manager();
        let agent_id = AgentId::new();
        let run_id = mgr.create_run(agent_id).await.expect("create");
        mgr.enqueue_run(run_id.clone()).await.expect("enqueue");
        mgr.start_run(run_id.clone()).await.expect("start");
        mgr.completing_run(run_id.clone()).await.expect("completing");
        mgr.complete_run(run_id.clone(), None).await.expect("complete");

        // Now try to cancel the already-completed run.
        let err = mgr
            .cancel_run(run_id, "too late")
            .await
            .unwrap_err();
        assert!(matches!(err, RunError::Transition(_)));
    }

    #[tokio::test]
    async fn get_state_returns_not_found_for_unknown_run() {
        let (mgr, _) = make_manager();
        let unknown_id = RunId::new();
        let err = mgr.get_state(unknown_id).await.unwrap_err();
        assert!(matches!(err, RunError::NotFound(_)));
    }

    #[tokio::test]
    async fn approval_flow() {
        let (mgr, events) = make_manager();
        let agent_id = AgentId::new();
        let run_id = mgr.create_run(agent_id).await.expect("create");
        mgr.enqueue_run(run_id.clone()).await.expect("enqueue");
        mgr.start_run(run_id.clone()).await.expect("start");

        // Request approval.
        mgr.request_approval(run_id.clone(), "req-1")
            .await
            .expect("request_approval");
        let state = mgr.get_state(run_id.clone()).await.expect("state");
        assert!(matches!(state, RunState::AwaitingApproval { .. }));

        // Grant approval.
        mgr.grant_approval(run_id.clone(), "approval-1")
            .await
            .expect("grant_approval");
        let state = mgr.get_state(run_id.clone()).await.expect("state");
        assert_eq!(state, RunState::Running);

        let stored = events.read_run_events(run_id).await.expect("read");
        let types: Vec<&str> = stored.iter().map(|e| e.event_type.as_str()).collect();
        assert!(types.contains(&"approval_requested"));
        assert!(types.contains(&"approval_granted"));
    }

    #[tokio::test]
    async fn deny_approval_cancels_run() {
        let (mgr, _) = make_manager();
        let agent_id = AgentId::new();
        let run_id = mgr.create_run(agent_id).await.expect("create");
        mgr.enqueue_run(run_id.clone()).await.expect("enqueue");
        mgr.start_run(run_id.clone()).await.expect("start");
        mgr.request_approval(run_id.clone(), "req-1")
            .await
            .expect("request");
        mgr.deny_approval(run_id.clone(), "budget exceeded")
            .await
            .expect("deny");

        let state = mgr.get_state(run_id).await.expect("state");
        assert!(matches!(state, RunState::Cancelled { .. }));
    }

    // ── parse_run_state helper tests ────────────────────────────────────────

    #[test]
    fn parse_created_state() {
        assert_eq!(parse_run_state("created").unwrap(), RunState::Created);
    }

    #[test]
    fn parse_queued_state() {
        assert_eq!(parse_run_state("queued").unwrap(), RunState::Queued);
    }

    #[test]
    fn parse_running_state() {
        assert_eq!(parse_run_state("running").unwrap(), RunState::Running);
    }

    #[test]
    fn parse_completed_state() {
        assert_eq!(parse_run_state("completed").unwrap(), RunState::Completed);
    }

    #[test]
    fn parse_timed_out_state() {
        assert_eq!(parse_run_state("timed_out").unwrap(), RunState::TimedOut);
    }

    #[test]
    fn parse_failed_state_with_reason() {
        let state = parse_run_state("failed:provider timeout").unwrap();
        assert_eq!(
            state,
            RunState::Failed {
                reason: "provider timeout".to_owned()
            }
        );
    }

    #[test]
    fn parse_cancelled_state_with_reason() {
        let state = parse_run_state("cancelled:user request").unwrap();
        assert_eq!(
            state,
            RunState::Cancelled {
                reason: "user request".to_owned()
            }
        );
    }

    #[test]
    fn parse_unknown_state_returns_error() {
        assert!(parse_run_state("bogus_state").is_err());
    }
}
