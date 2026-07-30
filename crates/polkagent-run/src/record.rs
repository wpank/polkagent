//! Run record types used by the API layer.

use chrono::{DateTime, Utc};
use polkagent_core::{AgentId, RunId, RunState};
use serde::{Deserialize, Serialize};

/// A persisted run record returned from the store.
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
    /// Opaque cursor from a previous response (`run_id` encoded as string).
    pub after: Option<RunId>,
    /// Maximum number of records to return. Capped at 100.
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
