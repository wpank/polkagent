//! Run record types and the `RunManager` used by the API layer.
//!
//! These types are self-contained within `polkagent-api` to avoid taking a
//! dependency on `polkagent-run` (which has transitive dependencies not yet
//! fully implemented). When `polkagent-run` stabilises, this module can be
//! replaced by a thin re-export.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use polkagent_core::{AgentId, RunId};
use polkagent_core::run::RunState;
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

/// Errors produced by the [`RunManager`].
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
// RunManager
// ---------------------------------------------------------------------------

/// Manages the full lifecycle of [`RunRecord`]s.
///
/// State is kept in a `RwLock<HashMap>` so that multiple concurrent handlers
/// can read runs without blocking writers. A production implementation would
/// delegate to a durable `RunStore`; this in-memory version is sufficient for
/// the API layer in tests and local mode.
pub struct RunManager {
    runs: Arc<RwLock<HashMap<RunId, RunRecord>>>,
}

impl std::fmt::Debug for RunManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunManager").finish_non_exhaustive()
    }
}

impl Default for RunManager {
    fn default() -> Self {
        Self::new()
    }
}

impl RunManager {
    /// Create a new, empty manager.
    #[must_use]
    pub fn new() -> Self {
        Self {
            runs: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Create and immediately start a run for the given agent.
    #[instrument(skip(self, input), fields(agent_id = %agent_id))]
    pub async fn create_run(
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

    /// Retrieve a single run by ID.
    pub async fn get_run(&self, run_id: RunId) -> Result<RunRecord, RunError> {
        let guard = self.runs.read().await;
        guard
            .get(&run_id)
            .cloned()
            .ok_or(RunError::NotFound(run_id))
    }

    /// Cancel a run, transitioning it to `Cancelled`.
    ///
    /// Returns `RunError::InvalidTransition` if the run is already terminal.
    #[instrument(skip(self), fields(run_id = %run_id))]
    pub async fn cancel_run(&self, run_id: RunId) -> Result<RunRecord, RunError> {
        let mut guard = self.runs.write().await;
        let record = guard
            .get_mut(&run_id)
            .ok_or(RunError::NotFound(run_id))?;

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

    /// List runs, applying filter and cursor-based pagination from `params`.
    ///
    /// Returns `(page, has_more)`.
    pub async fn list_runs(
        &self,
        params: ListRunsParams,
    ) -> Result<(Vec<RunRecord>, bool), RunError> {
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
                .map(|p| p + 1)
                .unwrap_or(0),
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
}
