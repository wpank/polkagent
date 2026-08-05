//! Run record types, the [`RunManagerTrait`] abstraction, and an in-memory
//! implementation.
//!
//! These types are self-contained within `polkagent-api` to avoid taking a
//! dependency on `polkagent-run` (which has transitive dependencies not yet
//! fully implemented). When `polkagent-run` stabilises, this module can be
//! replaced by a thin re-export.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use polkagent_core::run::RunState;
use polkagent_core::{AgentId, RunId};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::RwLock;
use tracing::{info, instrument};

// ---------------------------------------------------------------------------
// RunRecord
// ---------------------------------------------------------------------------

/// A persisted run record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunRecord {
    /// Unique identifier for this run.
    pub id: RunId,
    /// The agent that owns this run.
    pub agent_id: AgentId,
    /// Current lifecycle state.
    pub state: RunState,
    /// The user-supplied prompt or input (opaque JSON).
    pub input: serde_json::Value,
    /// Number of turns completed so far.
    pub turns_completed: u32,
    /// When the run was created.
    pub created_at: DateTime<Utc>,
    /// When the run started executing (i.e., left `Queued`).
    pub started_at: Option<DateTime<Utc>>,
    /// When the run reached a terminal state.
    pub completed_at: Option<DateTime<Utc>>,
    /// Human-readable reason for the terminal state, if any.
    pub terminal_reason: Option<String>,
}

// ---------------------------------------------------------------------------
// RunFilter / ListRunsParams
// ---------------------------------------------------------------------------

/// Filter parameters for listing runs.
#[derive(Debug, Clone, Default)]
pub struct RunFilter {
    /// Restrict to runs belonging to this agent.
    pub agent_id: Option<AgentId>,
    /// Restrict to runs in this state.
    pub state: Option<RunState>,
}

/// Pagination parameters for `list_runs`.
#[derive(Debug, Clone)]
pub struct ListRunsParams {
    /// Opaque cursor from a previous response (`RunId` encoded as string).
    pub after: Option<RunId>,
    /// Maximum number of records to return (capped at 100).
    pub limit: u32,
    /// Applied filters.
    pub filter: RunFilter,
}

impl Default for ListRunsParams {
    fn default() -> Self {
        Self {
            after: None,
            limit: 50,
            filter: RunFilter::default(),
        }
    }
}

// ---------------------------------------------------------------------------
// RunError
// ---------------------------------------------------------------------------

/// Errors produced by run manager implementations.
#[derive(Debug, Error)]
pub enum RunError {
    /// A run with the given ID was not found.
    #[error("run not found: {0}")]
    NotFound(RunId),

    /// The requested state transition is not permitted.
    #[error("invalid state transition for run {id}: {reason}")]
    InvalidTransition { id: RunId, reason: String },

    /// An agent ID was required but not present.
    #[error("agent not found: {0}")]
    AgentNotFound(AgentId),

    /// A generic internal error.
    #[error("internal error: {0}")]
    Internal(String),
}

// ---------------------------------------------------------------------------
// RunManagerTrait
// ---------------------------------------------------------------------------

/// Async trait abstracting over run lifecycle management.
///
/// Implementations must be `Send + Sync` so that `Arc<dyn RunManagerTrait>`
/// can be shared across Axum handlers. The in-memory implementation is
/// [`InMemoryRunManager`]; a durable SQLite-backed implementation can be
/// provided by an adapter crate.
#[async_trait]
pub trait RunManagerTrait: Send + Sync {
    /// Create and immediately start a run for the given agent.
    async fn create_run(
        &self,
        agent_id: AgentId,
        input: serde_json::Value,
    ) -> Result<RunRecord, RunError>;

    /// Retrieve a single run by ID.
    async fn get_run(&self, run_id: RunId) -> Result<RunRecord, RunError>;

    /// Cancel a run, transitioning it to `Cancelled`.
    ///
    /// Returns `RunError::InvalidTransition` if the run is already terminal.
    async fn cancel_run(&self, run_id: RunId) -> Result<RunRecord, RunError>;

    /// Resume a paused run, transitioning it from `AwaitingApproval` to `Running`.
    ///
    /// Returns `RunError::InvalidTransition` if the run is not in
    /// `AwaitingApproval` state. Returns `RunError::NotFound` if no run with
    /// the given ID exists.
    async fn resume_run(&self, run_id: RunId) -> Result<RunRecord, RunError>;

    /// List runs, applying filter and cursor-based pagination from `params`.
    ///
    /// Returns `(page, has_more)`.
    async fn list_runs(&self, params: ListRunsParams) -> Result<(Vec<RunRecord>, bool), RunError>;

    /// List turns for a run, ordered by sequence ascending.
    ///
    /// Returns an empty `Vec` if no turns exist for the run.
    /// Returns `RunError::NotFound` if the run does not exist.
    async fn list_turns(&self, run_id: RunId) -> Result<Vec<crate::dto::TurnSummary>, RunError>;

    /// Stop all active (non-terminal) runs for an agent.
    ///
    /// Returns the number of runs that were cancelled.
    async fn stop_agent_runs(&self, agent_id: AgentId) -> Result<u32, RunError>;

    /// Pause all running runs for an agent (transition to `AwaitingApproval`).
    ///
    /// Returns the number of runs that were paused.
    async fn pause_agent_runs(&self, agent_id: AgentId) -> Result<u32, RunError>;

    /// Resume all paused runs for an agent (transition from `AwaitingApproval` to `Running`).
    ///
    /// Returns the number of runs that were resumed.
    async fn resume_agent_runs(&self, agent_id: AgentId) -> Result<u32, RunError>;
}

// ---------------------------------------------------------------------------
// InMemoryRunManager
// ---------------------------------------------------------------------------

/// In-memory run manager for tests and local development.
///
/// State is kept in a `RwLock<HashMap>` so that multiple concurrent handlers
/// can read runs without blocking writers. A production implementation would
/// delegate to a durable store.
pub struct InMemoryRunManager {
    runs: Arc<RwLock<HashMap<RunId, RunRecord>>>,
}

impl std::fmt::Debug for InMemoryRunManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InMemoryRunManager").finish_non_exhaustive()
    }
}

impl Default for InMemoryRunManager {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryRunManager {
    /// Create a new, empty manager.
    #[must_use]
    pub fn new() -> Self {
        Self {
            runs: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Force an existing run into `AwaitingApproval` state.
    ///
    /// This method exists to support integration tests that need to put a run
    /// into a paused/waiting-for-approval state. In production the
    /// `AwaitingApproval` state is entered through the normal orchestration
    /// flow; this shortcut avoids the need for a full orchestrator in tests.
    pub async fn force_awaiting_approval(&self, run_id: RunId) {
        let mut guard = self.runs.write().await;
        if let Some(record) = guard.get_mut(&run_id) {
            record.state = RunState::AwaitingApproval {
                request_id: "test-approval-request".to_owned(),
            };
        }
    }
}

#[async_trait]
impl RunManagerTrait for InMemoryRunManager {
    #[instrument(skip(self, input), fields(agent_id = %agent_id))]
    async fn create_run(
        &self,
        agent_id: AgentId,
        input: serde_json::Value,
    ) -> Result<RunRecord, RunError> {
        let id = RunId::new();
        let now = Utc::now();
        let record = RunRecord {
            id,
            agent_id,
            state: RunState::Running,
            input,
            turns_completed: 0,
            created_at: now,
            started_at: Some(now),
            completed_at: None,
            terminal_reason: None,
        };

        {
            let mut guard = self.runs.write().await;
            guard.insert(id, record.clone());
        }

        info!(run_id = %id, "run created");
        Ok(record)
    }

    async fn get_run(&self, run_id: RunId) -> Result<RunRecord, RunError> {
        let guard = self.runs.read().await;
        guard
            .get(&run_id)
            .cloned()
            .ok_or(RunError::NotFound(run_id))
    }

    #[instrument(skip(self), fields(run_id = %run_id))]
    async fn cancel_run(&self, run_id: RunId) -> Result<RunRecord, RunError> {
        let mut guard = self.runs.write().await;
        let record = guard.get_mut(&run_id).ok_or(RunError::NotFound(run_id))?;

        if record.state.is_terminal() {
            return Err(RunError::InvalidTransition {
                id: run_id,
                reason: format!("run is already in terminal state {:?}", record.state),
            });
        }

        record.state = RunState::Cancelled {
            reason: "cancelled by user".to_owned(),
        };
        record.completed_at = Some(Utc::now());
        record.terminal_reason = Some("cancelled by user".to_owned());

        info!(run_id = %run_id, "run cancelled");
        Ok(record.clone())
    }

    #[instrument(skip(self), fields(run_id = %run_id))]
    async fn resume_run(&self, run_id: RunId) -> Result<RunRecord, RunError> {
        let mut guard = self.runs.write().await;
        let record = guard.get_mut(&run_id).ok_or(RunError::NotFound(run_id))?;

        match &record.state {
            RunState::AwaitingApproval { .. } => {
                record.state = RunState::Running;
                info!(run_id = %run_id, "run resumed");
                Ok(record.clone())
            }
            other => Err(RunError::InvalidTransition {
                id: run_id,
                reason: format!(
                    "can only resume a run in AwaitingApproval state, \
                     current state is {other:?}"
                ),
            }),
        }
    }

    async fn list_runs(&self, params: ListRunsParams) -> Result<(Vec<RunRecord>, bool), RunError> {
        let guard = self.runs.read().await;

        // Collect and sort by `created_at` ascending for stable pagination.
        let mut all: Vec<RunRecord> = guard.values().cloned().collect();
        all.sort_by_key(|r| r.created_at);

        // Apply filters.
        let filtered: Vec<RunRecord> = all
            .into_iter()
            .filter(|r| {
                if let Some(aid) = params.filter.agent_id {
                    if r.agent_id != aid {
                        return false;
                    }
                }
                if let Some(ref state) = params.filter.state {
                    if &r.state != state {
                        return false;
                    }
                }
                true
            })
            .collect();

        // Apply cursor: skip everything up to and including the cursor ID.
        let start = match params.after {
            Some(cursor) => filtered
                .iter()
                .position(|r| r.id == cursor)
                .map_or(0, |p| p + 1),
            None => 0,
        };

        // Fetch one extra item to detect `has_more`.
        let page: Vec<RunRecord> = filtered
            .iter()
            .skip(start)
            .take(params.limit as usize + 1)
            .cloned()
            .collect();

        let has_more = page.len() > params.limit as usize;
        let page = page.into_iter().take(params.limit as usize).collect();

        Ok((page, has_more))
    }

    async fn list_turns(&self, run_id: RunId) -> Result<Vec<crate::dto::TurnSummary>, RunError> {
        // Verify the run exists.
        let guard = self.runs.read().await;
        if !guard.contains_key(&run_id) {
            return Err(RunError::NotFound(run_id));
        }
        // The in-memory manager does not persist turns, so return an empty list.
        Ok(vec![])
    }

    async fn stop_agent_runs(&self, agent_id: AgentId) -> Result<u32, RunError> {
        let mut guard = self.runs.write().await;
        let mut count = 0u32;
        for record in guard.values_mut() {
            if record.agent_id == agent_id && !record.state.is_terminal() {
                record.state = RunState::Cancelled {
                    reason: "agent stopped".to_owned(),
                };
                record.completed_at = Some(Utc::now());
                record.terminal_reason = Some("agent stopped".to_owned());
                count += 1;
            }
        }
        Ok(count)
    }

    async fn pause_agent_runs(&self, agent_id: AgentId) -> Result<u32, RunError> {
        let mut guard = self.runs.write().await;
        let mut count = 0u32;
        for record in guard.values_mut() {
            if record.agent_id == agent_id && record.state == RunState::Running {
                record.state = RunState::AwaitingApproval {
                    request_id: format!("agent-pause-{}", record.id),
                };
                count += 1;
            }
        }
        Ok(count)
    }

    async fn resume_agent_runs(&self, agent_id: AgentId) -> Result<u32, RunError> {
        let mut guard = self.runs.write().await;
        let mut count = 0u32;
        for record in guard.values_mut() {
            if record.agent_id == agent_id
                && matches!(&record.state, RunState::AwaitingApproval { .. })
            {
                record.state = RunState::Running;
                count += 1;
            }
        }
        Ok(count)
    }
}
