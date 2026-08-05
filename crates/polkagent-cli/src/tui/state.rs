//! TUI application state — all mutable data owned by `App`.
#![allow(dead_code)]
//!
//! `TuiState` is the single source of truth for the running TUI. Every
//! background data-fetch writes to this struct; the render path reads from it
//! without performing I/O.

use chrono::{DateTime, Utc};

use crate::tui::interaction::InteractionState;
use crate::tui::views::audit::AuditFilter;

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
    #[must_use]
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
    #[must_use]
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
    #[must_use]
    pub fn duration_display(&self, now: DateTime<Utc>) -> String {
        let end = self.completed_at.unwrap_or(now);
        let secs = u64::try_from((end - self.created_at).num_seconds()).unwrap_or(0);
        if secs < 60 {
            format!("{secs}s")
        } else if secs < 3600 {
            format!("{}m {}s", secs / 60, secs % 60)
        } else {
            format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
        }
    }

    /// State glyph.
    #[must_use]
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
    /// Failure reason extracted from the terminal `RunFailed` / `RunCancelled` / `RunTimedOut` event.
    pub failure_reason: Option<String>,
    /// Per-turn summaries for the turn list panel.
    pub turns: Vec<TurnSummary>,
}

impl RunDetail {
    /// Duration string for completed runs, or elapsed for active ones.
    #[must_use]
    pub fn duration_display(&self, now: DateTime<Utc>) -> String {
        let end = self.completed_at.unwrap_or(now);
        let secs = u64::try_from((end - self.created_at).num_seconds()).unwrap_or(0);
        if secs < 60 {
            format!("{secs}s")
        } else if secs < 3600 {
            format!("{}m {}s", secs / 60, secs % 60)
        } else {
            format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
        }
    }

    /// State glyph.
    #[must_use]
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
// Memory entry
// ---------------------------------------------------------------------------

/// A single memory entry shown in the memory browser view.
#[derive(Debug, Clone)]
pub struct MemoryEntry {
    /// Primary key (UUID string).
    pub id: String,
    /// Memory type: "episodic", "semantic", "working", "procedural".
    pub memory_type: String,
    /// Agent that owns this memory.
    pub agent_name: String,
    /// Relevance / importance score in [0.0, 1.0].
    pub relevance_score: f64,
    /// Full text content.
    pub content: String,
    /// When the entry was created.
    pub created_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Audit event
// ---------------------------------------------------------------------------

/// A single audit log event shown in the audit log view.
#[derive(Debug, Clone)]
pub struct AuditEvent {
    /// Primary key.
    pub id: String,
    /// Severity level: "info", "warn", "error", "debug".
    pub severity: String,
    /// Event kind / category.
    pub kind: String,
    /// Agent name associated with this event.
    pub agent_name: String,
    /// Run ID (if relevant).
    pub run_id: Option<String>,
    /// Human-readable message.
    pub message: String,
    /// When the event occurred.
    pub timestamp: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Confirmation dialog state
// ---------------------------------------------------------------------------

/// Tracks whether a confirm/deny dialog is shown for the approvals view.
#[derive(Debug, Clone, Default)]
pub enum ConfirmDialog {
    /// No dialog is active.
    #[default]
    None,
    /// User pressed 'a' — confirm approval for the given `effect_id`.
    ConfirmApprove(String),
    /// User pressed 'd' — confirm denial for the given `effect_id`.
    ConfirmDeny(String),
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

    /// Memory entries for the memory browser view.
    pub memory_entries: Vec<MemoryEntry>,

    /// Current FTS search query for the memory browser.
    pub memory_search_query: String,

    /// Audit log events.
    pub audit_log: Vec<AuditEvent>,

    /// Active filter for the audit log view.
    pub audit_filter: AuditFilter,

    /// Confirmation dialog state for approve/deny actions.
    pub confirm_dialog: ConfirmDialog,

    /// Actionable console prompt and most-recent run projection.
    pub interaction: InteractionState,

    // -- Scroll / selection --------------------------------------------------
    /// Scroll state for the agents list.
    pub agents_scroll: ScrollState,

    /// Scroll state for the runs list.
    pub runs_scroll: ScrollState,

    /// Scroll state for the event timeline.
    pub timeline_scroll: ScrollState,

    /// Scroll state for the approval queue.
    pub approvals_scroll: ScrollState,

    /// Scroll state for the memory browser.
    pub memory_scroll: ScrollState,

    /// Scroll state for the audit log.
    pub audit_scroll: ScrollState,

    /// Which sub-panel is focused in the run detail view (0 = info, 1 = turns).
    pub detail_panel_index: usize,

    // -- Chain data -----------------------------------------------------------
    /// Best (head) block number from the connected chain.
    pub best_block: u64,

    /// Last finalized block number from the connected chain.
    pub finalized_block: u64,

    /// Human-readable chain name (e.g. "Polkadot", "Westend").
    pub chain_name: String,

    /// Node implementation version string (e.g. "Parity Polkadot/v1.7.0").
    pub node_version: String,

    /// Whether the TUI has an active RPC connection to a chain node.
    pub chain_connected: bool,

    // -- Widget data ---------------------------------------------------------
    /// Recent per-turn token counts for the token sparkline widget.
    ///
    /// Populated from the turns of the currently selected run (or the most
    /// recent active run). Each element is `input_tokens + output_tokens` for
    /// one turn.
    pub token_history: Vec<u64>,

    /// Number of error-level events in the last hour, shown by the error
    /// digest count on the dashboard.
    pub error_count: usize,

    /// Tokens consumed in the active run's context window.
    ///
    /// Derived from the sum of tokens across the turns of the selected run.
    pub context_used: u64,

    /// Total context window capacity for the active run's model.
    ///
    /// A default of `200_000` is used when the model is unknown.
    pub context_total: u64,

    /// Remaining spend budget as a fraction of total budget (0.0–1.0).
    ///
    /// Derived from `budget_status()` DB query.  1.0 = fully remaining,
    /// 0.0 = fully spent.
    pub budget_remaining: f64,

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

    /// Recompute derived widget fields from the currently loaded run detail.
    ///
    /// Call this whenever `run_detail` is updated so that sparkline, gauge,
    /// and progress bar data stays consistent with the run data.
    pub fn recompute_widget_data(&mut self) {
        if let Some(detail) = &self.run_detail {
            // Build per-turn token history for the sparkline.
            self.token_history = detail
                .turns
                .iter()
                .map(|t| t.input_tokens + t.output_tokens)
                .collect();

            // Context gauge: total tokens consumed vs model context limit.
            self.context_used = detail.input_tokens + detail.output_tokens;
            // Use a conservative 200k context window as the default.
            self.context_total = 200_000;
        } else {
            // No run selected: derive a coarse token history from recent runs.
            if self.token_history.is_empty() {
                self.token_history = self
                    .runs
                    .iter()
                    .take(60)
                    .rev()
                    .map(|r| r.input_tokens + r.output_tokens)
                    .collect();
            }
            self.context_used = 0;
            self.context_total = 200_000;
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ── Helper builders ──────────────────────────────────────────────────────

    fn make_turn(seq: u32, input: u64, output: u64) -> TurnSummary {
        TurnSummary {
            sequence: seq,
            role: "assistant".to_owned(),
            started_at: Utc::now(),
            completed_at: None,
            input_tokens: input,
            output_tokens: output,
        }
    }

    fn make_run_detail(turns: Vec<TurnSummary>) -> RunDetail {
        let total_in: u64 = turns.iter().map(|t| t.input_tokens).sum();
        let total_out: u64 = turns.iter().map(|t| t.output_tokens).sum();
        let turn_count = u32::try_from(turns.len()).unwrap_or(u32::MAX);
        RunDetail {
            id: "run-test-id-00000000".to_owned(),
            short_id: "run-test".to_owned(),
            agent_name: "test-agent".to_owned(),
            state: "working".to_owned(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            completed_at: None,
            turn_count,
            input_tokens: total_in,
            output_tokens: total_out,
            effect_count: 0,
            effects_succeeded: 0,
            effects_failed: 0,
            effects_pending: 0,
            failure_reason: None,
            turns,
        }
    }

    // ── TuiState::recompute_widget_data ──────────────────────────────────────

    #[test]
    fn test_recompute_no_run_detail_clears_context() {
        let mut state = TuiState::default();
        state.context_used = 99_999;
        state.context_total = 50_000;
        state.run_detail = None;

        state.recompute_widget_data();

        assert_eq!(
            state.context_used, 0,
            "context_used must be 0 with no detail"
        );
        assert_eq!(
            state.context_total, 200_000,
            "context_total must be 200k default"
        );
    }

    #[test]
    fn test_recompute_with_run_detail_builds_token_history() {
        let mut state = TuiState::default();
        let turns = vec![
            make_turn(1, 100, 50),
            make_turn(2, 200, 80),
            make_turn(3, 300, 120),
        ];
        state.run_detail = Some(make_run_detail(turns));
        state.recompute_widget_data();

        assert_eq!(state.token_history, vec![150, 280, 420]);
    }

    #[test]
    fn test_recompute_sets_context_used_from_total_tokens() {
        let mut state = TuiState::default();
        let turns = vec![make_turn(1, 1_000, 500), make_turn(2, 2_000, 1_000)];
        state.run_detail = Some(make_run_detail(turns));
        state.recompute_widget_data();

        // context_used = sum of all input+output across the detail struct.
        assert_eq!(state.context_used, 4_500);
    }

    #[test]
    fn test_recompute_context_total_is_200k_default() {
        let mut state = TuiState::default();
        state.run_detail = Some(make_run_detail(vec![make_turn(1, 10, 5)]));
        state.recompute_widget_data();

        assert_eq!(state.context_total, 200_000);
    }

    #[test]
    fn test_recompute_empty_turns_yields_empty_history() {
        let mut state = TuiState::default();
        state.run_detail = Some(make_run_detail(vec![]));
        state.recompute_widget_data();

        assert!(state.token_history.is_empty());
        assert_eq!(state.context_used, 0);
    }

    #[test]
    fn test_budget_remaining_default_is_zero() {
        let state = TuiState::default();
        assert_eq!(state.budget_remaining, 0.0);
    }

    #[test]
    fn test_error_count_default_is_zero() {
        let state = TuiState::default();
        assert_eq!(state.error_count, 0);
    }

    #[test]
    fn test_token_history_default_is_empty() {
        let state = TuiState::default();
        assert!(state.token_history.is_empty());
    }

    // ── Chain data defaults ─────────────────────────────────────────────────

    #[test]
    fn test_chain_data_defaults_show_zeros_and_not_connected() {
        let state = TuiState::default();
        assert_eq!(state.best_block, 0, "default best_block must be 0");
        assert_eq!(
            state.finalized_block, 0,
            "default finalized_block must be 0"
        );
        assert!(
            state.chain_name.is_empty(),
            "default chain_name must be empty"
        );
        assert!(
            state.node_version.is_empty(),
            "default node_version must be empty"
        );
        assert!(
            !state.chain_connected,
            "default chain_connected must be false"
        );
    }

    #[test]
    fn test_chain_state_update_reflects_new_block_numbers() {
        let mut state = TuiState::default();
        state.best_block = 22_500_000;
        state.finalized_block = 22_499_990;
        state.chain_name = "Polkadot".to_owned();
        state.node_version = "Parity Polkadot/v1.7.0".to_owned();
        state.chain_connected = true;

        assert_eq!(state.best_block, 22_500_000);
        assert_eq!(state.finalized_block, 22_499_990);
        assert_eq!(state.chain_name, "Polkadot");
        assert_eq!(state.node_version, "Parity Polkadot/v1.7.0");
        assert!(state.chain_connected);
    }

    #[test]
    fn test_chain_name_displayed_correctly_after_update() {
        let mut state = TuiState::default();
        assert!(state.chain_name.is_empty());

        state.chain_name = "Westend".to_owned();
        assert_eq!(state.chain_name, "Westend");

        state.chain_name = "Kusama".to_owned();
        assert_eq!(state.chain_name, "Kusama");
    }
}
