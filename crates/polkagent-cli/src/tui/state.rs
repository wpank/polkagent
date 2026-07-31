//! TUI application state — all mutable data owned by `App`.
#![allow(dead_code)]
//!
//! `TuiState` is the single source of truth for the running TUI. Every
//! background data-fetch writes to this struct; the render path reads from it
//! without performing I/O.

use chrono::{DateTime, Utc};

// ---------------------------------------------------------------------------
// Agent summary
// ---------------------------------------------------------------------------

/// Compact summary of an agent shown in the list and dashboard views.
#[derive(Debug, Clone)]
pub struct AgentSummary {
    /// UUID string — primary key.
    pub id: String,
    /// Display name.
    pub name: String,
    /// State string from the database (`active`, `configured`, etc.).
    pub state: String,
    /// Optional model identifier string.
    pub model: String,
    /// Total number of runs ever created for this agent.
    pub total_runs: u32,
    /// Number of currently non-terminal runs.
    pub active_runs: u32,
    /// When the agent record was last updated.
    pub updated_at: DateTime<Utc>,
}

impl AgentSummary {
    /// The Unicode glyph for this agent's state (PRD-13 Appendix B.2).
    pub fn glyph(&self) -> &'static str {
        match self.state.as_str() {
            "active" => "◉",
            "configured" | "created" => "○",
            "paused" => "⏸",
            "deactivated" | "archived" => "▪",
            _ => "■",
        }
    }

    /// Human-readable status label.
    pub fn status_label(&self) -> &str {
        match self.state.as_str() {
            "active" => "Active",
            "configured" => "Configured",
            "created" => "Created",
            "paused" => "Paused",
            "deactivated" => "Deactivated",
            "archived" => "Archived",
            _ => "Error",
        }
    }
}

// ---------------------------------------------------------------------------
// Run summary
// ---------------------------------------------------------------------------

/// Compact summary of a run shown in the list and dashboard views.
#[derive(Debug, Clone)]
pub struct RunSummary {
    /// UUID string — primary key.
    pub id: String,
    /// Short display ID (first 8 chars of UUID).
    pub short_id: String,
    /// Agent name (joined at query time).
    pub agent_name: String,
    /// Run state string.
    pub state: String,
    /// When the run was created.
    pub created_at: DateTime<Utc>,
    /// When the run completed (if terminal).
    pub completed_at: Option<DateTime<Utc>>,
    /// Total turn count.
    pub turn_count: u32,
    /// Total input tokens across all turns.
    pub input_tokens: u64,
    /// Total output tokens across all turns.
    pub output_tokens: u64,
}

impl RunSummary {
    /// Duration string for completed runs, or elapsed for active ones.
    pub fn duration_display(&self, now: DateTime<Utc>) -> String {
        let end = self.completed_at.unwrap_or(now);
        let secs = (end - self.created_at).num_seconds().max(0) as u64;
        if secs < 60 {
            format!("{secs}s")
        } else if secs < 3600 {
            format!("{}m {}s", secs / 60, secs % 60)
        } else {
            format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
        }
    }

    /// State glyph.
    pub fn state_glyph(&self) -> &'static str {
        match self.state.as_str() {
            "working" | "started" => "▶",
            "completed" => "✓",
            "failed" | "timed_out" => "✗",
            "cancelled" => "▪",
            "created" | "queued" => "◦",
            _ => "?",
        }
    }
}

// ---------------------------------------------------------------------------
// Run detail
// ---------------------------------------------------------------------------

/// Full detail of a single run, loaded when drilling into the run detail view.
#[derive(Debug, Clone)]
pub struct RunDetail {
    /// UUID string -- primary key.
    pub id: String,
    /// Short display ID (first 8 chars of UUID).
    pub short_id: String,
    /// Agent name (joined at query time).
    pub agent_name: String,
    /// Run state string.
    pub state: String,
    /// When the run was created.
    pub created_at: DateTime<Utc>,
    /// When the run was last updated.
    pub updated_at: DateTime<Utc>,
    /// When the run completed (if terminal).
    pub completed_at: Option<DateTime<Utc>>,
    /// Total turn count.
    pub turn_count: u32,
    /// Total input tokens across all turns.
    pub input_tokens: u64,
    /// Total output tokens across all turns.
    pub output_tokens: u64,
    /// Total effect intent count.
    pub effect_count: u32,
    /// Number of effects with a successful outcome.
    pub effects_succeeded: u32,
    /// Number of effects with a failed outcome.
    pub effects_failed: u32,
    /// Number of effects still pending (no outcome).
    pub effects_pending: u32,
    /// Per-turn summaries for the turn list panel.
    pub turns: Vec<TurnSummary>,
}

impl RunDetail {
    /// Duration string for completed runs, or elapsed for active ones.
    pub fn duration_display(&self, now: DateTime<Utc>) -> String {
        let end = self.completed_at.unwrap_or(now);
        let secs = (end - self.created_at).num_seconds().max(0) as u64;
        if secs < 60 {
            format!("{secs}s")
        } else if secs < 3600 {
            format!("{}m {}s", secs / 60, secs % 60)
        } else {
            format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
        }
    }

    /// State glyph.
    pub fn state_glyph(&self) -> &'static str {
        match self.state.as_str() {
            "working" | "started" => "▶",
            "completed" => "✓",
            "failed" | "timed_out" => "✗",
            "cancelled" => "▪",
            "created" | "queued" => "◦",
            _ => "?",
        }
    }
}

/// Compact summary of a single turn within a run.
#[derive(Debug, Clone)]
pub struct TurnSummary {
    /// Turn sequence number (1-based).
    pub sequence: u32,
    /// Role of this turn (user, assistant, system, tool).
    pub role: String,
    /// When the turn started.
    pub started_at: DateTime<Utc>,
    /// When the turn completed (if finished).
    pub completed_at: Option<DateTime<Utc>>,
    /// Input tokens for this turn.
    pub input_tokens: u64,
    /// Output tokens for this turn.
    pub output_tokens: u64,
}

// ---------------------------------------------------------------------------
// Event summary
// ---------------------------------------------------------------------------

/// Compact summary of a run event for the timeline view.
#[derive(Debug, Clone)]
pub struct EventSummary {
    /// Event ID.
    pub id: String,
    /// Timestamp of the event.
    pub timestamp: DateTime<Utc>,
    /// Event type / kind string.
    pub event_type: String,
    /// Brief human-readable description extracted from the payload.
    pub description: String,
    /// Raw JSON payload for the detail panel.
    pub payload: String,
}

// ---------------------------------------------------------------------------
// Approval item
// ---------------------------------------------------------------------------

/// An effect intent awaiting approval, shown in the approval queue.
#[derive(Debug, Clone)]
pub struct ApprovalItem {
    /// Effect intent ID.
    pub effect_id: String,
    /// Effect kind (sign, broadcast, tool, etc.).
    pub kind: String,
    /// Run ID that originated this effect.
    pub run_id: String,
    /// Agent name for the owning run.
    pub agent_name: String,
    /// When the effect was created.
    pub created_at: DateTime<Utc>,
    /// Current state of the effect (pending, claimed, etc.).
    pub state: String,
}

// ---------------------------------------------------------------------------
// System health
// ---------------------------------------------------------------------------

/// Snapshot of system health metrics for the dashboard.
#[derive(Debug, Clone)]
pub struct SystemHealth {
    /// Whether the database file exists and is reachable.
    pub db_ok: bool,
    /// Path to the database file.
    pub db_path: String,
    /// Total number of agents in the database.
    pub agent_count: u32,
    /// Number of agents in the `active` state.
    pub active_agent_count: u32,
    /// Total number of runs ever created.
    pub total_run_count: u32,
    /// Runs in non-terminal states.
    pub active_run_count: u32,
    /// When this snapshot was taken.
    pub sampled_at: DateTime<Utc>,
}

impl Default for SystemHealth {
    fn default() -> Self {
        Self {
            db_ok: false,
            db_path: String::from("(not configured)"),
            agent_count: 0,
            active_agent_count: 0,
            total_run_count: 0,
            active_run_count: 0,
            sampled_at: Utc::now(),
        }
    }
}

// ---------------------------------------------------------------------------
// Scroll state
// ---------------------------------------------------------------------------

/// Simple scroll offset tracker for list views.
#[derive(Debug, Clone, Default)]
pub struct ScrollState {
    /// Current scroll offset (0-based row index of the top visible item).
    pub offset: usize,
    /// Index of the selected item within the visible list.
    pub selected: Option<usize>,
}

impl ScrollState {
    /// Move the selection down, adjusting the offset when needed.
    pub fn down(&mut self, total_items: usize, visible_rows: usize) {
        let selected = self.selected.unwrap_or(0);
        let next = (selected + 1).min(total_items.saturating_sub(1));
        self.selected = Some(next);
        // Scroll the viewport when the selection moves past the bottom.
        if next >= self.offset + visible_rows {
            self.offset = next.saturating_sub(visible_rows - 1);
        }
    }

    /// Move the selection up, adjusting the offset when needed.
    pub fn up(&mut self) {
        let selected = self.selected.unwrap_or(0);
        let prev = selected.saturating_sub(1);
        self.selected = Some(prev);
        // Scroll the viewport when the selection moves above the top.
        if prev < self.offset {
            self.offset = prev;
        }
    }

    /// Reset scroll to the top.
    pub fn reset(&mut self) {
        self.offset = 0;
        self.selected = Some(0);
    }
}

// ---------------------------------------------------------------------------
// Main TUI state
// ---------------------------------------------------------------------------

/// All mutable data for the running TUI.
///
/// The render path accesses this struct read-only. Background tasks write
/// to it through the `App` command channel or by direct mutation when called
/// from the event loop on the main thread.
#[derive(Debug, Default)]
pub struct TuiState {
    // -- Data ----------------------------------------------------------------
    /// Agent summaries for the agents view and dashboard.
    pub agents: Vec<AgentSummary>,

    /// Run summaries for the runs view and dashboard.
    pub runs: Vec<RunSummary>,

    /// System health snapshot.
    pub health: SystemHealth,

    // -- Run detail / timeline / approvals -----------------------------------
    /// Currently selected run ID (set when drilling into a run).
    pub selected_run: Option<String>,

    /// Full detail for the selected run.
    pub run_detail: Option<RunDetail>,

    /// Events for the selected run (timeline view).
    pub run_events: Vec<EventSummary>,

    /// Pending effects awaiting approval (approval queue view).
    pub pending_approvals: Vec<ApprovalItem>,

    // -- Scroll / selection --------------------------------------------------
    /// Scroll state for the agents list.
    pub agents_scroll: ScrollState,

    /// Scroll state for the runs list.
    pub runs_scroll: ScrollState,

    /// Scroll state for the event timeline.
    pub timeline_scroll: ScrollState,

    /// Scroll state for the approval queue.
    pub approvals_scroll: ScrollState,

    /// Which sub-panel is focused in the run detail view (0 = info, 1 = turns).
    pub detail_panel_index: usize,

    // -- Refresh bookkeeping -------------------------------------------------
    /// Whether the state has changed since the last render (triggers a draw).
    pub dirty: bool,

    /// When data was last loaded from the database.
    pub last_refresh: Option<DateTime<Utc>>,

    /// Error message from the last background refresh (displayed in status bar).
    pub last_error: Option<String>,
}

impl TuiState {
    /// Mark the state as requiring a re-render.
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }
}
