//! TUI application state — all mutable data owned by `App`.
#![allow(dead_code)]
//!
//! `TuiState` is the single source of truth for the running TUI. Every
//! background data-fetch writes to this struct; the render path reads from it
//! without performing I/O.

use chrono::{DateTime, Utc};

use crate::tui::interaction::{ConsoleApprovalContext, InteractionState};
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

/// Redaction-safe projection of one durable approval request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalItem {
    /// Conversation whose authenticated approval scope owns this request.
    pub conversation_id: String,
    /// Stable durable approval-request identity.
    pub approval_id: String,
    /// Effect intent guarded by the request.
    pub effect_id: String,
    /// Parent run.
    pub run_id: String,
    /// Optional linked tool call.
    pub tool_call_id: Option<String>,
    /// Safe, bounded operation title supplied by the approval service.
    pub title: String,
    /// Safe, bounded operation description supplied by the approval service.
    pub description: String,
    /// Durable approval lifecycle state.
    pub status: String,
    /// Safe policy explanation, when available.
    pub policy_reason: Option<String>,
    /// Decision deadline, when available.
    pub expires_at: Option<DateTime<Utc>>,
}

impl ApprovalItem {
    /// Project the shared service view without exposing raw provider payloads.
    #[must_use]
    pub fn from_view(conversation_id: &str, view: &polkagent_interaction::ApprovalView) -> Self {
        use polkagent_interaction::ApprovalStatus;

        let status = match view.status {
            ApprovalStatus::Pending => "pending",
            ApprovalStatus::Approved => "approved",
            ApprovalStatus::Denied => "denied",
            ApprovalStatus::Expired => "expired",
            ApprovalStatus::Cancelled => "cancelled",
        };
        Self {
            conversation_id: conversation_id.to_owned(),
            approval_id: view.approval_id.to_string(),
            effect_id: view.effect_id.to_string(),
            run_id: view.run_id.to_string(),
            tool_call_id: view.tool_call_id.as_ref().map(ToString::to_string),
            title: approval_display_text(&view.title, 160),
            description: approval_display_text(&view.description, 1_024),
            status: status.to_owned(),
            policy_reason: view
                .policy_reason
                .as_deref()
                .map(|reason| approval_display_text(reason, 512)),
            expires_at: view.expires_at,
        }
    }
}

fn approval_display_text(value: &str, max_chars: usize) -> String {
    let redacted = polkagent_telemetry::redact_string(value);
    let normalized = redacted.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() <= max_chars {
        normalized
    } else {
        let mut bounded = normalized
            .chars()
            .take(max_chars.saturating_sub(1))
            .collect::<String>();
        bounded.push('…');
        bounded
    }
}

/// Exact target retained by a confirmation dialog and revalidated on submit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalActionTarget {
    pub conversation_id: String,
    pub approval_id: String,
}

/// Current asynchronous state of the scoped F6 approval surface.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ApprovalQueueStatus {
    /// No durable Console conversation is selected.
    #[default]
    Unscoped,
    /// A service list request is active.
    Loading,
    /// The scoped list is current.
    Ready,
    /// An approve/deny compare-and-set is active.
    Resolving,
    /// No approval authority is composed for this process.
    Unavailable,
    /// A scoped request failed for another reason.
    Failed,
}

/// Correlation and guidance for the approval queue.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ApprovalQueueState {
    pub conversation_id: Option<String>,
    pub request_id: Option<String>,
    pub status: ApprovalQueueStatus,
    pub message: Option<String>,
}

impl ApprovalQueueState {
    /// Reject stale async completions after either the request or visible
    /// conversation changes.
    #[must_use]
    pub fn is_current(
        &self,
        request_id: &str,
        conversation_id: &str,
        selected_conversation_id: Option<&str>,
    ) -> bool {
        self.request_id.as_deref() == Some(request_id)
            && self.conversation_id.as_deref() == Some(conversation_id)
            && selected_conversation_id == Some(conversation_id)
    }
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
    /// User pressed 'a' — confirm an exact scoped approval request.
    ConfirmApprove(ApprovalActionTarget),
    /// User pressed 'd' — confirm an exact scoped approval request.
    ConfirmDeny(ApprovalActionTarget),
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

    /// Pending durable approval requests for the selected Console conversation.
    pub pending_approvals: Vec<ApprovalItem>,

    /// Scope, correlation, and availability of the asynchronous approval queue.
    pub approval_queue: ApprovalQueueState,

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

    /// Derive Console approval availability from the existing correlated F6
    /// queue without copying that queue into the interaction reducer.
    #[must_use]
    pub fn console_approval_context(&self) -> Option<ConsoleApprovalContext> {
        if self.approval_queue.status != ApprovalQueueStatus::Ready {
            return None;
        }
        let conversation = self.tui_state_conversation_for_approvals()?;
        let conversation_id = conversation.parse().ok()?;
        let mut pending_approval_ids = Vec::with_capacity(self.pending_approvals.len());
        for approval in self.pending_approvals.iter().filter(|approval| {
            approval.conversation_id == conversation && approval.status == "pending"
        }) {
            pending_approval_ids.push(approval.approval_id.parse().ok()?);
        }
        Some(ConsoleApprovalContext::new(
            conversation_id,
            pending_approval_ids,
        ))
    }

    fn tui_state_conversation_for_approvals(&self) -> Option<&str> {
        let conversation = self.interaction.conversation_id.as_deref()?;
        (self.approval_queue.conversation_id.as_deref() == Some(conversation))
            .then_some(conversation)
    }

    /// Replace a scoped service result while preserving selection by durable
    /// approval identity. The queue is independently bounded from transport.
    pub fn replace_scoped_approvals(
        &mut self,
        conversation_id: &str,
        approvals: Vec<polkagent_interaction::ApprovalView>,
    ) {
        let selected_id = self
            .approvals_scroll
            .selected
            .and_then(|index| self.pending_approvals.get(index))
            .map(|item| item.approval_id.clone());
        self.pending_approvals = approvals
            .into_iter()
            .filter(|approval| approval.status == polkagent_interaction::ApprovalStatus::Pending)
            .take(100)
            .map(|approval| ApprovalItem::from_view(conversation_id, &approval))
            .collect();
        self.approvals_scroll.selected = selected_id
            .as_deref()
            .and_then(|approval_id| {
                self.pending_approvals
                    .iter()
                    .position(|item| item.approval_id == approval_id)
            })
            .or_else(|| (!self.pending_approvals.is_empty()).then_some(0));
        self.approvals_scroll.offset = 0;
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

    #[test]
    fn approval_projection_is_redacted_bounded_and_identity_exact() {
        let approval_id = polkagent_core::ApprovalId::new();
        let effect_id = polkagent_core::EffectId::new();
        let run_id = polkagent_core::RunId::new();
        let view = polkagent_interaction::ApprovalView {
            approval_id,
            effect_id,
            run_id,
            tool_call_id: Some(polkagent_interaction::ToolCallId::new()),
            title: format!(
                "write\nnotes with sk-abc123def456ghi789 {}",
                "x".repeat(300)
            ),
            description: format!("private\toperation {}", "d".repeat(2_000)),
            status: polkagent_interaction::ApprovalStatus::Pending,
            policy_reason: Some("credential sk-abc123def456ghi789".to_owned()),
            expires_at: None,
        };
        let item = ApprovalItem::from_view("conversation-exact", &view);

        assert_eq!(item.conversation_id, "conversation-exact");
        assert_eq!(item.approval_id, approval_id.to_string());
        assert_eq!(item.effect_id, effect_id.to_string());
        assert_eq!(item.run_id, run_id.to_string());
        assert_eq!(item.status, "pending");
        assert!(!item.title.contains('\n'));
        assert!(!item.description.contains('\t'));
        assert!(!item.title.contains("abc123def456ghi789"));
        assert!(!item
            .policy_reason
            .as_deref()
            .unwrap_or_default()
            .contains("abc123def456ghi789"));
        assert!(item.title.chars().count() <= 160);
        assert!(item.description.chars().count() <= 1_024);
    }

    #[test]
    fn approval_queue_rejects_stale_request_and_conversation_completions() {
        let queue = ApprovalQueueState {
            conversation_id: Some("conversation-new".to_owned()),
            request_id: Some("request-new".to_owned()),
            status: ApprovalQueueStatus::Loading,
            message: None,
        };
        assert!(queue.is_current("request-new", "conversation-new", Some("conversation-new")));
        assert!(!queue.is_current("request-old", "conversation-new", Some("conversation-new")));
        assert!(!queue.is_current("request-new", "conversation-old", Some("conversation-new")));
        assert!(!queue.is_current(
            "request-new",
            "conversation-new",
            Some("conversation-switched")
        ));
    }

    #[test]
    fn approval_queue_is_bounded_and_preserves_selection_by_approval_id() {
        fn view(title: String) -> polkagent_interaction::ApprovalView {
            polkagent_interaction::ApprovalView {
                approval_id: polkagent_core::ApprovalId::new(),
                effect_id: polkagent_core::EffectId::new(),
                run_id: polkagent_core::RunId::new(),
                tool_call_id: None,
                title,
                description: "safe".to_owned(),
                status: polkagent_interaction::ApprovalStatus::Pending,
                policy_reason: None,
                expires_at: None,
            }
        }

        let mut state = TuiState::default();
        let first = view("first".to_owned());
        let selected = view("selected".to_owned());
        state.replace_scoped_approvals("conversation", vec![first.clone(), selected.clone()]);
        state.approvals_scroll.selected = Some(1);
        let mut refreshed = vec![selected.clone(), first];
        refreshed.extend((0..120).map(|index| view(format!("extra-{index}"))));
        state.replace_scoped_approvals("conversation", refreshed);

        assert_eq!(state.pending_approvals.len(), 100);
        assert_eq!(state.approvals_scroll.selected, Some(0));
        assert_eq!(
            state.pending_approvals[0].approval_id,
            selected.approval_id.to_string()
        );
    }
}
