//! Main TUI application shell.
//!
//! Owns the terminal state machine, drives the render loop, dispatches
//! [`TuiAction`]s, and coordinates background data refreshes.
//!
//! # Frame budget (target 60 fps, flush every 6 frames → ~10 fps effective)
//!
//! | Phase            | Budget    |
//! |------------------|-----------|
//! | Input poll       | ~0.1 ms   |
//! | Data refresh     | ~2.0 ms   |
//! | Render           | ~4.0 ms   |
//! | Terminal flush   | ~2.0 ms   |
//! | **Total**        | **~8 ms** |

use std::io::Stdout;
use std::ops::{Deref, DerefMut};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use crossterm::{
    cursor::Show,
    event::{DisableMouseCapture, EnableMouseCapture, Event},
    execute,
    terminal::{enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::Style,
    widgets::Block,
    Frame, Terminal,
};

use polkagent_config::Config;
use polkagent_store_sqlite::SqlitePool;

use crate::tui::{
    input::{key_to_action, InputMode, TuiAction},
    interaction::{ControllerEvent, RunController},
    state::TuiState,
    theme::Theme,
    views,
    widgets::{header_bar, status_bar},
};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Internal tick rate.
pub const TARGET_FPS: u64 = 60;
/// Duration of one frame.
pub const FRAME_DURATION: Duration = Duration::from_micros(1_000_000 / TARGET_FPS);
/// Flush the terminal every N frames (10 fps effective).
pub const FLUSH_DIVISOR: u64 = 6;
/// Flush divisor used when the TUI has been idle for 5 seconds (5 fps).
pub const FLUSH_DIVISOR_IDLE: u64 = 12;
/// Refresh database data every N seconds.
pub const REFRESH_INTERVAL_SECS: u64 = 5;

// ---------------------------------------------------------------------------
// Tab
// ---------------------------------------------------------------------------

/// Top-level region tab (F1–F9) plus pseudo-tabs for drill-down views.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    /// F1 — Overview: agent grid, run summary, system health.
    #[default]
    Dashboard,
    /// F2 — Agent list and detail.
    Agents,
    /// F3 — Run list and detail.
    Runs,
    /// F4 — System health and configuration.
    System,
    /// F5 — Event timeline for selected run.
    Timeline,
    /// F6 — Approval queue for pending effects.
    Approvals,
    /// F7 — Memory browser.
    Memory,
    /// F8 — Audit log.
    Audit,
    /// F9 — Interactive agent console.
    Console,
    /// Run detail (entered from Runs via Enter; not a top-level F-key tab).
    RunDetail,
}

#[allow(dead_code)]
impl Tab {
    /// All tabs in display order (excludes pseudo-tabs like `RunDetail`).
    pub const ALL: [Tab; 9] = [
        Tab::Dashboard,
        Tab::Agents,
        Tab::Runs,
        Tab::System,
        Tab::Timeline,
        Tab::Approvals,
        Tab::Memory,
        Tab::Audit,
        Tab::Console,
    ];

    /// Display name used in the header bar and status bar.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Dashboard => "DASHBOARD",
            Self::Agents => "AGENTS",
            Self::Runs => "RUNS",
            Self::System => "SYSTEM",
            Self::Timeline => "TIMELINE",
            Self::Approvals => "APPROVALS",
            Self::Memory => "MEMORY",
            Self::Audit => "AUDIT",
            Self::Console => "CONSOLE",
            Self::RunDetail => "RUN DETAIL",
        }
    }

    /// F-key indicator shown next to the tab name.
    #[must_use]
    pub fn fkey_label(self) -> &'static str {
        match self {
            Self::Dashboard => "[F1]",
            Self::Agents => "[F2]",
            Self::Runs => "[F3]",
            Self::System => "[F4]",
            Self::Timeline => "[F5]",
            Self::Approvals => "[F6]",
            Self::Memory => "[F7]",
            Self::Audit => "[F8]",
            Self::Console => "[F9]",
            Self::RunDetail => "[--]",
        }
    }

    /// Return the next tab in display order (wraps around).
    /// `RunDetail` maps to its parent (Runs).
    #[must_use]
    pub fn next(self) -> Self {
        match self {
            Self::Dashboard => Self::Agents,
            Self::Agents => Self::Runs,
            Self::Runs | Self::RunDetail => Self::System,
            Self::System => Self::Timeline,
            Self::Timeline => Self::Approvals,
            Self::Approvals => Self::Memory,
            Self::Memory => Self::Audit,
            Self::Audit => Self::Console,
            Self::Console => Self::Dashboard,
        }
    }

    /// Parse a tab name from the `--tab` CLI flag value.
    ///
    /// Accepted values match the flag's documented names (case-insensitive).
    /// Unknown names fall back to [`Tab::Dashboard`].
    #[must_use]
    pub fn from_cli_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "agents" => Self::Agents,
            "runs" => Self::Runs,
            "system" => Self::System,
            "timeline" => Self::Timeline,
            "approvals" => Self::Approvals,
            "memory" => Self::Memory,
            "audit" => Self::Audit,
            "console" | "chat" => Self::Console,
            _ => Self::Dashboard,
        }
    }

    /// Return the previous tab in display order (wraps around).
    /// `RunDetail` maps to its parent (Runs).
    #[must_use]
    pub fn prev(self) -> Self {
        match self {
            Self::Dashboard => Self::Console,
            Self::Agents => Self::Dashboard,
            Self::Runs => Self::Agents,
            Self::System | Self::RunDetail => Self::Runs,
            Self::Timeline => Self::System,
            Self::Approvals => Self::Timeline,
            Self::Memory => Self::Approvals,
            Self::Audit => Self::Memory,
            Self::Console => Self::Audit,
        }
    }
}

// ---------------------------------------------------------------------------
// App
// ---------------------------------------------------------------------------

/// Primary TUI application shell.
///
/// Owns terminal state, navigation, theme, and all mutable data. The render
/// path reads only `&self` — zero I/O in the hot path.
pub struct App {
    // -- Navigation ----------------------------------------------------------
    pub active_tab: Tab,

    // -- State ---------------------------------------------------------------
    pub tui_state: TuiState,
    pub theme: Theme,

    // -- Input ---------------------------------------------------------------
    pub input_mode: InputMode,

    // -- Loop control --------------------------------------------------------
    pub running: bool,
    pub frame_counter: u64,
    pub last_input: Instant,
    pub last_refresh: Instant,

    // -- Data source ---------------------------------------------------------
    pub pool: SqlitePool,

    // -- Chain polling -------------------------------------------------------
    pub chain_poller: crate::tui::db::ChainPoller,

    // -- Interactive execution ---------------------------------------------
    pub run_controller: RunController,
}

impl App {
    /// Create a new `App` with the given theme, database pool, and initial tab.
    pub fn new(theme: Theme, pool: SqlitePool, config: Config, initial_tab: Tab) -> Self {
        let run_controller = RunController::new(pool.clone(), config);
        Self {
            active_tab: initial_tab,
            tui_state: TuiState::default(),
            theme,
            input_mode: InputMode::default(),
            running: true,
            frame_counter: 0,
            last_input: Instant::now(),
            last_refresh: Instant::now()
                .checked_sub(Duration::from_secs(REFRESH_INTERVAL_SECS + 1))
                .unwrap_or_else(Instant::now),
            pool,
            chain_poller: crate::tui::db::ChainPoller::new(),
            run_controller,
        }
    }

    // ── Event loop ──────────────────────────────────────────────────────────

    /// Run the main event loop until `self.running` becomes `false`.
    pub fn run(&mut self, terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
        let mut last_frame = Instant::now();

        // Initial data load.
        self.refresh_data();

        loop {
            let frame_start = Instant::now();

            // 1. Poll input (non-blocking).
            let timeout = FRAME_DURATION.saturating_sub(last_frame.elapsed());
            if crossterm::event::poll(timeout)? {
                let event = crossterm::event::read()?;
                if matches!(event, Event::Key(_)) {
                    self.last_input = Instant::now();
                }
                if let Some(action) = terminal_event_to_action(&event, self.input_mode) {
                    self.apply_action(action);
                }
            }

            // 2. Background data refresh (every REFRESH_INTERVAL_SECS).
            self.drain_run_events();

            if self.last_refresh.elapsed().as_secs() >= REFRESH_INTERVAL_SECS {
                self.refresh_data();
                self.last_refresh = Instant::now();
            }

            // 2b. Chain poller (every 6 seconds, independent of DB refresh).
            if self.chain_poller.should_poll() {
                self.chain_poller.poll(&mut self.tui_state);
            }

            // 3. Render (throttled to effective ~10 fps, or ~5 fps when idle).
            self.frame_counter = self.frame_counter.wrapping_add(1);
            let idle = self.last_input.elapsed().as_secs() > 5;
            let divisor = if idle {
                FLUSH_DIVISOR_IDLE
            } else {
                FLUSH_DIVISOR
            };
            if self.frame_counter.is_multiple_of(divisor) || self.tui_state.dirty {
                terminal.draw(|frame| self.render(frame))?;
                self.tui_state.dirty = false;
            }

            // 4. Exit check.
            if !self.running {
                break;
            }

            last_frame = frame_start;
        }

        Ok(())
    }

    // ── Action dispatch ─────────────────────────────────────────────────────

    /// Apply a [`TuiAction`] to `self`, mutating `TuiState` as needed.
    #[allow(
        clippy::too_many_lines,
        reason = "the exhaustive TUI state-machine dispatcher keeps action ordering and shared dirty-state updates in one auditable boundary"
    )]
    #[allow(
        clippy::needless_pass_by_value,
        reason = "the event loop transfers ownership of each short-lived action into the state-machine dispatch boundary"
    )]
    pub fn apply_action(&mut self, action: TuiAction) {
        match action {
            TuiAction::NavigateTab(tab) => {
                self.active_tab = tab;
                if tab == Tab::Console {
                    self.ensure_console_agent();
                }
                // When switching to timeline, refresh events for the selected run.
                if tab == Tab::Timeline {
                    self.refresh_run_events();
                }
                // When switching to approvals, refresh the approval queue.
                if tab == Tab::Approvals {
                    self.refresh_approvals();
                }
                // When switching to memory, refresh memory entries.
                if tab == Tab::Memory {
                    self.refresh_memory();
                }
                // When switching to audit, refresh the audit log.
                if tab == Tab::Audit {
                    self.refresh_audit_log();
                }
                self.tui_state.mark_dirty();
            }

            TuiAction::NavigateUp => match self.active_tab {
                Tab::Agents => {
                    self.tui_state.agents_scroll.up();
                    self.tui_state.mark_dirty();
                }
                Tab::Runs => {
                    self.tui_state.runs_scroll.up();
                    self.tui_state.mark_dirty();
                }
                Tab::Timeline => {
                    self.tui_state.timeline_scroll.up();
                    self.tui_state.mark_dirty();
                }
                Tab::Approvals => {
                    self.tui_state.approvals_scroll.up();
                    self.tui_state.mark_dirty();
                }
                Tab::Memory => {
                    self.tui_state.memory_scroll.up();
                    self.tui_state.mark_dirty();
                }
                Tab::Audit => {
                    self.tui_state.audit_scroll.up();
                    self.tui_state.mark_dirty();
                }
                _ => {}
            },

            TuiAction::NavigateDown => {
                let visible = Self::visible_rows();
                match self.active_tab {
                    Tab::Agents => {
                        let total = self.tui_state.agents.len();
                        self.tui_state.agents_scroll.down(total, visible);
                        self.tui_state.mark_dirty();
                    }
                    Tab::Runs => {
                        let total = self.tui_state.runs.len();
                        self.tui_state.runs_scroll.down(total, visible);
                        self.tui_state.mark_dirty();
                    }
                    Tab::Timeline => {
                        let total = self.tui_state.run_events.len();
                        self.tui_state.timeline_scroll.down(total, visible);
                        self.tui_state.mark_dirty();
                    }
                    Tab::Approvals => {
                        let total = self.tui_state.pending_approvals.len();
                        self.tui_state.approvals_scroll.down(total, visible);
                        self.tui_state.mark_dirty();
                    }
                    Tab::Memory => {
                        let total = self.tui_state.memory_entries.len();
                        self.tui_state.memory_scroll.down(total, visible);
                        self.tui_state.mark_dirty();
                    }
                    Tab::Audit => {
                        let total = self.tui_state.audit_log.len();
                        self.tui_state.audit_scroll.down(total, visible);
                        self.tui_state.mark_dirty();
                    }
                    _ => {}
                }
            }

            TuiAction::ScrollUp(n) => {
                for _ in 0..n {
                    match self.active_tab {
                        Tab::Agents => self.tui_state.agents_scroll.up(),
                        Tab::Runs => self.tui_state.runs_scroll.up(),
                        Tab::Timeline => self.tui_state.timeline_scroll.up(),
                        Tab::Approvals => self.tui_state.approvals_scroll.up(),
                        Tab::Memory => self.tui_state.memory_scroll.up(),
                        Tab::Audit => self.tui_state.audit_scroll.up(),
                        _ => {}
                    }
                }
                self.tui_state.mark_dirty();
            }

            TuiAction::ScrollDown(n) => {
                let visible = Self::visible_rows();
                for _ in 0..n {
                    match self.active_tab {
                        Tab::Agents => {
                            let t = self.tui_state.agents.len();
                            self.tui_state.agents_scroll.down(t, visible);
                        }
                        Tab::Runs => {
                            let t = self.tui_state.runs.len();
                            self.tui_state.runs_scroll.down(t, visible);
                        }
                        Tab::Timeline => {
                            let t = self.tui_state.run_events.len();
                            self.tui_state.timeline_scroll.down(t, visible);
                        }
                        Tab::Approvals => {
                            let t = self.tui_state.pending_approvals.len();
                            self.tui_state.approvals_scroll.down(t, visible);
                        }
                        Tab::Memory => {
                            let t = self.tui_state.memory_entries.len();
                            self.tui_state.memory_scroll.down(t, visible);
                        }
                        Tab::Audit => {
                            let t = self.tui_state.audit_log.len();
                            self.tui_state.audit_scroll.down(t, visible);
                        }
                        _ => {}
                    }
                }
                self.tui_state.mark_dirty();
            }

            TuiAction::Select => {
                match self.active_tab {
                    Tab::Agents => {
                        if self.tui_state.agents_scroll.selected.is_none() {
                            self.tui_state.agents_scroll.selected = Some(0);
                        }
                        self.tui_state.mark_dirty();
                    }
                    Tab::Runs => {
                        // Select a run and drill into run detail.
                        let sel = self.tui_state.runs_scroll.selected.unwrap_or(0);
                        self.tui_state.runs_scroll.selected = Some(sel);
                        if let Some(run) = self.tui_state.runs.get(sel) {
                            self.tui_state.selected_run = Some(run.id.clone());
                            self.refresh_run_detail();
                            self.refresh_run_events();
                            self.active_tab = Tab::RunDetail;
                        }
                        self.tui_state.mark_dirty();
                    }
                    Tab::Timeline => {
                        // Ensure an event is selected for the detail panel.
                        if self.tui_state.timeline_scroll.selected.is_none()
                            && !self.tui_state.run_events.is_empty()
                        {
                            self.tui_state.timeline_scroll.selected = Some(0);
                        }
                        self.tui_state.mark_dirty();
                    }
                    Tab::Approvals => {
                        // Ensure an approval item is selected for the detail panel.
                        if self.tui_state.approvals_scroll.selected.is_none()
                            && !self.tui_state.pending_approvals.is_empty()
                        {
                            self.tui_state.approvals_scroll.selected = Some(0);
                        }
                        self.tui_state.mark_dirty();
                    }
                    Tab::Memory => {
                        // Ensure a memory entry is selected for the detail panel.
                        if self.tui_state.memory_scroll.selected.is_none()
                            && !self.tui_state.memory_entries.is_empty()
                        {
                            self.tui_state.memory_scroll.selected = Some(0);
                        }
                        self.tui_state.mark_dirty();
                    }
                    Tab::Audit => {
                        // Ensure an audit event is selected.
                        if self.tui_state.audit_scroll.selected.is_none()
                            && !self.tui_state.audit_log.is_empty()
                        {
                            self.tui_state.audit_scroll.selected = Some(0);
                        }
                        self.tui_state.mark_dirty();
                    }
                    _ => {}
                }
            }

            TuiAction::Back => {
                if self.input_mode == InputMode::Prompt {
                    self.input_mode = InputMode::Normal;
                    self.tui_state.interaction.clear_prompt();
                    self.tui_state.mark_dirty();
                    return;
                }
                match self.active_tab {
                    Tab::Agents => {
                        self.tui_state.agents_scroll.selected = None;
                        self.tui_state.mark_dirty();
                    }
                    Tab::Runs => {
                        self.tui_state.runs_scroll.selected = None;
                        self.tui_state.mark_dirty();
                    }
                    Tab::RunDetail => {
                        // Go back to runs list.
                        self.active_tab = Tab::Runs;
                        self.tui_state.mark_dirty();
                    }
                    Tab::Timeline => {
                        if self.tui_state.timeline_scroll.selected.is_some() {
                            self.tui_state.timeline_scroll.selected = None;
                        } else {
                            self.active_tab = Tab::Dashboard;
                        }
                        self.tui_state.mark_dirty();
                    }
                    Tab::Approvals => {
                        // Cancel any open confirmation dialog first.
                        if !matches!(
                            self.tui_state.confirm_dialog,
                            crate::tui::state::ConfirmDialog::None
                        ) {
                            self.tui_state.confirm_dialog = crate::tui::state::ConfirmDialog::None;
                        } else if self.tui_state.approvals_scroll.selected.is_some() {
                            self.tui_state.approvals_scroll.selected = None;
                        } else {
                            self.active_tab = Tab::Dashboard;
                        }
                        self.tui_state.mark_dirty();
                    }
                    Tab::Memory => {
                        if self.input_mode == InputMode::Insert {
                            self.input_mode = InputMode::Normal;
                            self.tui_state.memory_search_query.clear();
                            self.refresh_memory();
                        } else if self.tui_state.memory_scroll.selected.is_some() {
                            self.tui_state.memory_scroll.selected = None;
                        } else {
                            self.active_tab = Tab::Dashboard;
                        }
                        self.tui_state.mark_dirty();
                    }
                    Tab::Audit => {
                        if self.tui_state.audit_scroll.selected.is_some() {
                            self.tui_state.audit_scroll.selected = None;
                        } else {
                            self.active_tab = Tab::Dashboard;
                        }
                        self.tui_state.mark_dirty();
                    }
                    _ => self.active_tab = Tab::Dashboard,
                }
            }

            TuiAction::SelectRun => {
                // Same as Select when on the Runs tab.
                if self.active_tab == Tab::Runs {
                    self.apply_action(TuiAction::Select);
                }
            }

            TuiAction::ViewTimeline => {
                self.active_tab = Tab::Timeline;
                self.refresh_run_events();
                self.tui_state.mark_dirty();
            }

            TuiAction::ViewApprovals => {
                self.active_tab = Tab::Approvals;
                self.refresh_approvals();
                self.tui_state.mark_dirty();
            }

            TuiAction::ApproveEffect => {
                use crate::tui::state::ConfirmDialog;
                if self.active_tab == Tab::Approvals {
                    match &self.tui_state.confirm_dialog {
                        ConfirmDialog::ConfirmApprove(effect_id) => {
                            // User confirmed — execute approval.
                            let effect_id = effect_id.clone();
                            self.execute_approve(&effect_id);
                            self.tui_state.confirm_dialog = ConfirmDialog::None;
                        }
                        ConfirmDialog::None => {
                            // First press — show confirmation dialog.
                            if let Some(sel) = self.tui_state.approvals_scroll.selected {
                                if let Some(item) = self.tui_state.pending_approvals.get(sel) {
                                    self.tui_state.confirm_dialog =
                                        ConfirmDialog::ConfirmApprove(item.effect_id.clone());
                                }
                            }
                        }
                        ConfirmDialog::ConfirmDeny(_) => {
                            // A deny dialog is open; cancel it, then set approve.
                            self.tui_state.confirm_dialog = ConfirmDialog::None;
                        }
                    }
                }
                self.tui_state.mark_dirty();
            }

            TuiAction::DenyEffect => {
                use crate::tui::state::ConfirmDialog;
                if self.active_tab == Tab::Approvals {
                    match &self.tui_state.confirm_dialog {
                        ConfirmDialog::ConfirmDeny(effect_id) => {
                            // User confirmed denial.
                            let effect_id = effect_id.clone();
                            self.execute_deny(&effect_id);
                            self.tui_state.confirm_dialog = ConfirmDialog::None;
                        }
                        ConfirmDialog::None => {
                            // First press — show confirmation dialog.
                            if let Some(sel) = self.tui_state.approvals_scroll.selected {
                                if let Some(item) = self.tui_state.pending_approvals.get(sel) {
                                    self.tui_state.confirm_dialog =
                                        ConfirmDialog::ConfirmDeny(item.effect_id.clone());
                                }
                            }
                        }
                        ConfirmDialog::ConfirmApprove(_) => {
                            // An approve dialog is open; cancel it, then set deny.
                            self.tui_state.confirm_dialog = ConfirmDialog::None;
                        }
                    }
                }
                self.tui_state.mark_dirty();
            }

            TuiAction::ScrollToBottom => {
                let visible = Self::visible_rows();
                match self.active_tab {
                    Tab::Audit => {
                        let total = self.tui_state.audit_log.len();
                        if total > 0 {
                            self.tui_state.audit_scroll.selected = Some(total - 1);
                            self.tui_state.audit_scroll.offset = total.saturating_sub(visible);
                        }
                    }
                    Tab::Timeline => {
                        let total = self.tui_state.run_events.len();
                        if total > 0 {
                            self.tui_state.timeline_scroll.selected = Some(total - 1);
                            self.tui_state.timeline_scroll.offset = total.saturating_sub(visible);
                        }
                    }
                    _ => {}
                }
                self.tui_state.mark_dirty();
            }

            TuiAction::ScrollToTop => {
                match self.active_tab {
                    Tab::Audit => {
                        self.tui_state.audit_scroll.selected = Some(0);
                        self.tui_state.audit_scroll.offset = 0;
                    }
                    Tab::Timeline => {
                        self.tui_state.timeline_scroll.selected = Some(0);
                        self.tui_state.timeline_scroll.offset = 0;
                    }
                    _ => {}
                }
                self.tui_state.mark_dirty();
            }

            TuiAction::DeleteEntry => {
                if self.active_tab == Tab::Memory {
                    if let Some(sel) = self.tui_state.memory_scroll.selected {
                        if sel < self.tui_state.memory_entries.len() {
                            let entry_id = self.tui_state.memory_entries[sel].id.clone();
                            self.execute_delete_memory(&entry_id);
                            // Adjust selection after deletion.
                            let new_len = self.tui_state.memory_entries.len();
                            if new_len == 0 {
                                self.tui_state.memory_scroll.selected = None;
                            } else {
                                let new_sel = sel.min(new_len - 1);
                                self.tui_state.memory_scroll.selected = Some(new_sel);
                            }
                        }
                    }
                }
                self.tui_state.mark_dirty();
            }

            TuiAction::CycleFilter => {
                if self.active_tab == Tab::Audit {
                    use crate::tui::views::audit::AuditFilter;
                    // Cycle through: None -> BySeverity("warn") -> BySeverity("error") -> None.
                    self.tui_state.audit_filter = match &self.tui_state.audit_filter {
                        AuditFilter::None => AuditFilter::BySeverity("warn".to_owned()),
                        AuditFilter::BySeverity(s) if s == "warn" => {
                            AuditFilter::BySeverity("error".to_owned())
                        }
                        _ => AuditFilter::None,
                    };
                }
                self.tui_state.mark_dirty();
            }

            TuiAction::Search => {
                // Activate the memory search bar. Switch to the Memory tab
                // if not already there, then enter Insert mode so subsequent
                // keystrokes are captured as search input.
                if self.active_tab != Tab::Memory {
                    self.active_tab = Tab::Memory;
                    self.refresh_memory();
                }
                self.input_mode = InputMode::Insert;
                self.tui_state.mark_dirty();
            }

            TuiAction::SearchInput(c) => {
                self.tui_state.memory_search_query.push(c);
                self.refresh_memory();
                self.tui_state.memory_scroll.offset = 0;
                self.tui_state.memory_scroll.selected = None;
                self.tui_state.mark_dirty();
            }

            TuiAction::SearchBackspace => {
                self.tui_state.memory_search_query.pop();
                self.refresh_memory();
                self.tui_state.memory_scroll.offset = 0;
                self.tui_state.memory_scroll.selected = None;
                self.tui_state.mark_dirty();
            }

            TuiAction::SearchSubmit => {
                self.input_mode = InputMode::Normal;
                self.tui_state.mark_dirty();
            }

            TuiAction::TogglePanel => {
                if self.active_tab == Tab::RunDetail {
                    self.tui_state.detail_panel_index = (self.tui_state.detail_panel_index + 1) % 2;
                    self.tui_state.mark_dirty();
                }
            }

            TuiAction::OpenPrompt => {
                let run_active = self
                    .tui_state
                    .interaction
                    .run
                    .as_ref()
                    .is_some_and(|run| !run.status.is_terminal());
                if run_active {
                    self.tui_state.last_error =
                        Some("a console run is already active; press x to cancel it".to_owned());
                } else if self.ensure_console_agent() {
                    self.active_tab = Tab::Console;
                    self.input_mode = InputMode::Prompt;
                    self.tui_state.last_error = None;
                }
                self.tui_state.mark_dirty();
            }

            TuiAction::PromptInput(c) => {
                self.tui_state.interaction.push_char(c);
                self.tui_state.mark_dirty();
            }

            TuiAction::PromptNewline => {
                self.tui_state.interaction.insert_newline();
                self.tui_state.mark_dirty();
            }

            TuiAction::PromptBackspace => {
                self.tui_state.interaction.backspace();
                self.tui_state.mark_dirty();
            }

            TuiAction::PromptDelete => {
                self.tui_state.interaction.delete();
                self.tui_state.mark_dirty();
            }

            TuiAction::PromptMoveLeft => {
                self.tui_state.interaction.move_left();
                self.tui_state.mark_dirty();
            }

            TuiAction::PromptMoveRight => {
                self.tui_state.interaction.move_right();
                self.tui_state.mark_dirty();
            }

            TuiAction::PromptMoveUp => {
                self.tui_state.interaction.move_up();
                self.tui_state.mark_dirty();
            }

            TuiAction::PromptMoveDown => {
                self.tui_state.interaction.move_down();
                self.tui_state.mark_dirty();
            }

            TuiAction::PromptMoveHome => {
                self.tui_state.interaction.move_home();
                self.tui_state.mark_dirty();
            }

            TuiAction::PromptMoveEnd => {
                self.tui_state.interaction.move_end();
                self.tui_state.mark_dirty();
            }

            TuiAction::PromptSubmit => match self.tui_state.interaction.submit() {
                Ok(request) => {
                    self.input_mode = InputMode::Normal;
                    if let Err(error) = self.run_controller.start(request) {
                        self.tui_state
                            .interaction
                            .apply(ControllerEvent::Failed(error.to_owned()));
                        self.tui_state.last_error = Some(error.to_owned());
                    } else {
                        self.tui_state.last_error = None;
                    }
                    self.tui_state.mark_dirty();
                }
                Err(error) => {
                    self.tui_state.last_error = Some(error.to_owned());
                    self.tui_state.mark_dirty();
                }
            },

            TuiAction::CancelActiveRun => {
                if self.run_controller.cancel() {
                    self.tui_state.interaction.mark_cancelling();
                    self.tui_state.last_error = None;
                } else {
                    self.tui_state.last_error = Some("no console run is active".to_owned());
                }
                self.tui_state.mark_dirty();
            }

            TuiAction::Quit => {
                let _ = self.run_controller.cancel();
                self.running = false;
            }

            TuiAction::Refresh => {
                self.refresh_data();
                self.last_refresh = Instant::now();
                self.tui_state.mark_dirty();
            }

            TuiAction::Resize(_w, _h) => {
                self.tui_state.mark_dirty();
            }
        }
    }

    /// Select the highlighted active agent, retain the console target when it
    /// is still active, or fall back to the first active agent.
    fn ensure_console_agent(&mut self) -> bool {
        let selected = self
            .tui_state
            .agents_scroll
            .selected
            .and_then(|index| self.tui_state.agents.get(index))
            .filter(|agent| agent.state == "active")
            .or_else(|| {
                let current = self.tui_state.interaction.agent_id.as_deref();
                self.tui_state
                    .agents
                    .iter()
                    .find(|agent| Some(agent.id.as_str()) == current && agent.state == "active")
            })
            .or_else(|| {
                self.tui_state
                    .agents
                    .iter()
                    .find(|agent| agent.state == "active")
            })
            .map(|agent| (agent.id.clone(), agent.name.clone()));

        if let Some((id, name)) = selected {
            let changed = self.tui_state.interaction.agent_id.as_deref() != Some(id.as_str());
            if changed {
                self.tui_state.interaction.select_agent(id, name);
            }
            true
        } else {
            self.tui_state.last_error =
                Some("no active agent is available; activate an agent before prompting".to_owned());
            false
        }
    }

    /// Drain controller events on the terminal thread and immediately refresh
    /// durable run projections when lifecycle state changes.
    fn drain_run_events(&mut self) {
        let mut refresh = false;
        while let Some(event) = self.run_controller.try_recv() {
            if let ControllerEvent::Started { run_id, .. } = &event {
                self.tui_state.selected_run = Some(run_id.clone());
                refresh = true;
            }
            if matches!(
                event,
                ControllerEvent::Completed { .. }
                    | ControllerEvent::Failed(_)
                    | ControllerEvent::Cancelled(_)
                    | ControllerEvent::TimedOut
            ) {
                refresh = true;
            }
            self.tui_state.interaction.apply(event);
            self.tui_state.mark_dirty();
        }
        if refresh {
            self.refresh_data();
            self.last_refresh = Instant::now();
        }
    }

    // ── Data refresh ────────────────────────────────────────────────────────

    /// Load fresh data from the database into `tui_state`.
    ///
    /// Errors are stored in `tui_state.last_error` rather than propagated so
    /// that the TUI keeps running even if the database is temporarily unavailable.
    fn refresh_data(&mut self) {
        use crate::tui::db::TuiDb;

        let db_path_display = self.pool.path().display().to_string();

        match TuiDb::from_pool(&self.pool) {
            Ok(db) => {
                // Agents.
                match db.agents() {
                    Ok(agents) => self.tui_state.agents = agents,
                    Err(e) => {
                        self.tui_state.last_error = Some(format!("agents: {e}"));
                    }
                }

                // Recent runs (up to 100).
                match db.recent_runs(100) {
                    Ok(runs) => self.tui_state.runs = runs,
                    Err(e) => {
                        self.tui_state.last_error = Some(format!("runs: {e}"));
                    }
                }

                // System health.
                self.tui_state.health = db.system_health(&db_path_display);

                // Pending approvals (always refresh — visible on dashboard too).
                match db.pending_effects(100) {
                    Ok(approvals) => self.tui_state.pending_approvals = approvals,
                    Err(e) => {
                        self.tui_state.last_error = Some(format!("approvals: {e}"));
                    }
                }

                // If a run is selected, refresh its detail and events.
                if let Some(run_id) = &self.tui_state.selected_run.clone() {
                    match db.run_detail(run_id) {
                        Ok(detail) => self.tui_state.run_detail = detail,
                        Err(e) => {
                            self.tui_state.last_error = Some(format!("run_detail: {e}"));
                        }
                    }
                    match db.run_events(run_id, 500) {
                        Ok(events) => self.tui_state.run_events = events,
                        Err(e) => {
                            self.tui_state.last_error = Some(format!("run_events: {e}"));
                        }
                    }
                }

                // Error count for dashboard digest.
                self.tui_state.error_count = db.recent_error_count() as usize;

                // Budget remaining for the budget gauge.
                let (spent, ceiling) = db.budget_status();
                self.tui_state.budget_remaining = if ceiling > 0.0 {
                    (ceiling - spent) / ceiling * 100.0
                } else {
                    100.0
                };

                // Clear any previous error now that all queries succeeded.
                self.tui_state.last_error = None;
                self.tui_state.last_refresh = Some(chrono::Utc::now());
                self.tui_state.recompute_widget_data();
                self.tui_state.mark_dirty();
            }
            Err(e) => {
                // Can't open reader — update health to reflect the failure.
                self.tui_state.health.db_ok = false;
                self.tui_state.last_error = Some(format!("db: {e}"));
                self.tui_state.mark_dirty();
            }
        }
    }

    /// Refresh only the run detail for the currently selected run.
    fn refresh_run_detail(&mut self) {
        use crate::tui::db::TuiDb;

        let Some(run_id) = &self.tui_state.selected_run.clone() else {
            return;
        };
        if let Ok(db) = TuiDb::from_pool(&self.pool) {
            match db.run_detail(run_id) {
                Ok(detail) => self.tui_state.run_detail = detail,
                Err(e) => {
                    self.tui_state.last_error = Some(format!("run_detail: {e}"));
                }
            }
        }
    }

    /// Refresh only the events for the currently selected run.
    fn refresh_run_events(&mut self) {
        use crate::tui::db::TuiDb;

        let Some(run_id) = &self.tui_state.selected_run.clone() else {
            return;
        };
        if let Ok(db) = TuiDb::from_pool(&self.pool) {
            match db.run_events(run_id, 500) {
                Ok(events) => self.tui_state.run_events = events,
                Err(e) => {
                    self.tui_state.last_error = Some(format!("run_events: {e}"));
                }
            }
        }
    }

    /// Refresh only the approval queue.
    fn refresh_approvals(&mut self) {
        use crate::tui::db::TuiDb;

        if let Ok(db) = TuiDb::from_pool(&self.pool) {
            match db.pending_effects(100) {
                Ok(approvals) => self.tui_state.pending_approvals = approvals,
                Err(e) => {
                    self.tui_state.last_error = Some(format!("approvals: {e}"));
                }
            }
        }
    }

    /// Refresh the memory browser entries.
    fn refresh_memory(&mut self) {
        use crate::tui::db::TuiDb;

        if let Ok(_db) = TuiDb::from_pool(&self.pool) {
            let query = if self.tui_state.memory_search_query.is_empty() {
                None
            } else {
                Some(self.tui_state.memory_search_query.as_str())
            };
            match TuiDb::memory_entries(query, 200) {
                Ok(entries) => self.tui_state.memory_entries = entries,
                Err(e) => {
                    self.tui_state.last_error = Some(format!("memory: {e}"));
                }
            }
        }
    }

    /// Refresh the audit log.
    fn refresh_audit_log(&mut self) {
        use crate::tui::db::TuiDb;

        if let Ok(db) = TuiDb::from_pool(&self.pool) {
            match db.audit_events(500) {
                Ok(events) => self.tui_state.audit_log = events,
                Err(e) => {
                    self.tui_state.last_error = Some(format!("audit: {e}"));
                }
            }
        }
    }

    /// Execute an approve action on an effect intent (write to DB).
    ///
    /// Inserts an effect outcome row with status='success'. Errors are stored
    /// in `tui_state.last_error` rather than propagated.
    fn execute_approve(&mut self, effect_id: &str) {
        use crate::tui::db::TuiDb;

        if let Ok(db) = TuiDb::from_pool(&self.pool) {
            match db.approve_effect(effect_id) {
                Ok(()) => {
                    // Remove from the local pending list immediately.
                    self.tui_state
                        .pending_approvals
                        .retain(|a| a.effect_id != effect_id);
                    self.tui_state.last_error = None;
                }
                Err(e) => {
                    self.tui_state.last_error = Some(format!("approve: {e}"));
                }
            }
        }
    }

    /// Execute a deny action on an effect intent (write to DB).
    fn execute_deny(&mut self, effect_id: &str) {
        use crate::tui::db::TuiDb;

        if let Ok(db) = TuiDb::from_pool(&self.pool) {
            match db.deny_effect(effect_id) {
                Ok(()) => {
                    self.tui_state
                        .pending_approvals
                        .retain(|a| a.effect_id != effect_id);
                    self.tui_state.last_error = None;
                }
                Err(e) => {
                    self.tui_state.last_error = Some(format!("deny: {e}"));
                }
            }
        }
    }

    /// Execute a delete action on a memory entry.
    fn execute_delete_memory(&mut self, entry_id: &str) {
        use crate::tui::db::TuiDb;

        if let Ok(_db) = TuiDb::from_pool(&self.pool) {
            match TuiDb::delete_memory_entry(entry_id) {
                Ok(()) => {
                    self.tui_state.memory_entries.retain(|m| m.id != entry_id);
                    self.tui_state.last_error = None;
                }
                Err(e) => {
                    self.tui_state.last_error = Some(format!("delete_memory: {e}"));
                }
            }
        }
    }

    // ── Layout helpers ──────────────────────────────────────────────────────

    /// Estimate the number of visible data rows in the main content area.
    ///
    /// Uses the current terminal height minus chrome (header, tab bar, status
    /// bar, block borders, table header).
    fn visible_rows() -> usize {
        let h = crossterm::terminal::size().map_or(24, |(_, h)| h) as usize;
        // 3 chrome rows (header + tab_bar + status_bar) + 2 border + 1 table header
        h.saturating_sub(6)
    }

    // ── Render pipeline ─────────────────────────────────────────────────────

    /// Render the full TUI frame.
    ///
    /// Called from `terminal.draw(|frame| self.render(frame))`. Zero I/O.
    fn render(&self, frame: &mut Frame) {
        let size = frame.area();

        // Fill the entire terminal with the void background.
        let bg = Block::default().style(Style::default().bg(self.theme.bg_void));
        frame.render_widget(bg, size);

        // Compute layout regions.
        let layout = compute_layout(size);

        // Chrome.
        header_bar::render(frame, layout.header, self.active_tab, &self.theme);
        status_bar::render(
            frame,
            layout.status,
            self.active_tab,
            self.input_mode,
            self.tui_state.last_error.as_deref(),
            &self.theme,
        );

        // Tab bar (F1–F4 indicators at the very bottom of the header area).
        self.render_tab_bar(frame, layout.tab_bar);

        // Active view.
        match self.active_tab {
            Tab::Dashboard => {
                views::dashboard::render(frame, layout.main, &self.tui_state, &self.theme);
            }
            Tab::Agents => {
                views::agents::render(frame, layout.main, &self.tui_state, &self.theme);
            }
            Tab::Runs => {
                views::runs::render(frame, layout.main, &self.tui_state, &self.theme);
            }
            Tab::System => {
                views::system::render(frame, layout.main, &self.tui_state, &self.theme);
            }
            Tab::RunDetail => {
                views::run_detail::render(frame, layout.main, &self.tui_state, &self.theme);
            }
            Tab::Timeline => {
                views::timeline::render(frame, layout.main, &self.tui_state, &self.theme);
            }
            Tab::Approvals => {
                views::approvals::render(frame, layout.main, &self.tui_state, &self.theme);
            }
            Tab::Memory => {
                views::memory::render(frame, layout.main, &self.tui_state, &self.theme);
            }
            Tab::Audit => {
                views::audit::render(frame, layout.main, &self.tui_state, &self.theme);
            }
            Tab::Console => {
                views::console::render(
                    frame,
                    layout.main,
                    &self.tui_state,
                    self.input_mode,
                    &self.theme,
                );
            }
        }
    }

    /// Render the horizontal tab indicator row.
    fn render_tab_bar(&self, frame: &mut Frame, area: Rect) {
        use ratatui::{
            style::Modifier,
            text::{Line, Span},
            widgets::Paragraph,
        };

        let tabs = [
            (Tab::Dashboard, "F1 Dashboard"),
            (Tab::Agents, "F2 Agents"),
            (Tab::Runs, "F3 Runs"),
            (Tab::System, "F4 System"),
            (Tab::Timeline, "F5 Timeline"),
            (Tab::Approvals, "F6 Approvals"),
            (Tab::Memory, "F7 Memory"),
            (Tab::Audit, "F8 Audit"),
            (Tab::Console, "F9 Console"),
        ];

        let mut spans = Vec::with_capacity(tabs.len() * 2);
        for (tab, label) in &tabs {
            let style = if *tab == self.active_tab {
                Style::default()
                    .fg(self.theme.rose_bright)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(self.theme.text_dim)
            };
            spans.push(Span::styled(format!("  {label}  "), style));
        }

        let bar_bg = Block::default().style(Style::default().bg(self.theme.bg_raised));
        frame.render_widget(bar_bg, area);
        frame.render_widget(Paragraph::new(Line::from(spans)), area);
    }
}

/// Translate one already-read terminal event into an application action.
///
/// Keeping event acquisition outside this function ensures each successful
/// poll consumes exactly one event. In particular, resize events must not
/// trigger a second blocking read while the first event is discarded.
fn terminal_event_to_action(event: &Event, input_mode: InputMode) -> Option<TuiAction> {
    match event {
        Event::Key(key) => key_to_action(*key, input_mode),
        Event::Resize(width, height) => Some(TuiAction::Resize(*width, *height)),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Layout computation
// ---------------------------------------------------------------------------

/// Computed layout regions for the TUI frame.
struct TuiLayout {
    header: Rect,
    tab_bar: Rect,
    main: Rect,
    status: Rect,
}

fn compute_layout(area: Rect) -> TuiLayout {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // header bar
            Constraint::Length(1), // tab bar
            Constraint::Min(5),    // main content
            Constraint::Length(1), // status bar
        ])
        .split(area);

    TuiLayout {
        header: rows[0],
        tab_bar: rows[1],
        main: rows[2],
        status: rows[3],
    }
}

// ---------------------------------------------------------------------------
// Terminal init / teardown
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RestoreStep {
    RawMode,
    AlternateScreen,
    MouseCapture,
    Cursor,
}

impl RestoreStep {
    const ALL: [Self; 4] = [
        Self::RawMode,
        Self::AlternateScreen,
        Self::MouseCapture,
        Self::Cursor,
    ];

    const fn mask(self) -> u8 {
        match self {
            Self::RawMode => 1 << 0,
            Self::AlternateScreen => 1 << 1,
            Self::MouseCapture => 1 << 2,
            Self::Cursor => 1 << 3,
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::RawMode => "disable raw mode",
            Self::AlternateScreen => "leave alternate screen",
            Self::MouseCapture => "disable mouse capture",
            Self::Cursor => "show cursor",
        }
    }
}

trait RestoreActions {
    fn restore(&mut self, step: RestoreStep) -> std::io::Result<()>;
}

struct CrosstermRestoreActions;

impl RestoreActions for CrosstermRestoreActions {
    fn restore(&mut self, step: RestoreStep) -> std::io::Result<()> {
        match step {
            RestoreStep::RawMode => crossterm::terminal::disable_raw_mode(),
            RestoreStep::AlternateScreen => {
                let mut stdout = std::io::stdout();
                execute!(stdout, LeaveAlternateScreen)
            }
            RestoreStep::MouseCapture => {
                let mut stdout = std::io::stdout();
                execute!(stdout, DisableMouseCapture)
            }
            RestoreStep::Cursor => {
                let mut stdout = std::io::stdout();
                execute!(stdout, Show)
            }
        }
    }
}

struct RestoreProgress(u8);

impl RestoreProgress {
    const fn pending() -> Self {
        Self((1 << RestoreStep::ALL.len()) - 1)
    }

    const fn contains(&self, step: RestoreStep) -> bool {
        self.0 & step.mask() != 0
    }

    fn complete(&mut self, step: RestoreStep) {
        self.0 &= !step.mask();
    }
}

struct RestorationGuard<A: RestoreActions> {
    actions: A,
    pending: RestoreProgress,
}

impl<A: RestoreActions> RestorationGuard<A> {
    const fn new(actions: A) -> Self {
        Self {
            actions,
            pending: RestoreProgress::pending(),
        }
    }

    fn restore(&mut self) -> Result<()> {
        let mut errors = Vec::new();

        for step in RestoreStep::ALL {
            if !self.pending.contains(step) {
                continue;
            }
            match self.actions.restore(step) {
                Ok(()) => self.pending.complete(step),
                Err(error) => errors.push(format!("{}: {error}", step.label())),
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(anyhow!(
                "terminal restoration failed: {}",
                errors.join("; ")
            ))
        }
    }
}

impl<A: RestoreActions> Drop for RestorationGuard<A> {
    fn drop(&mut self) {
        // A best-effort retry covers setup failures, early returns, and
        // unwinding. Explicit teardown still reports its first-pass errors.
        drop(self.restore());
    }
}

/// A configured terminal paired with a drop-backed restoration guard.
///
/// The wrapper dereferences to ratatui's terminal so callers can render as
/// usual. If setup, the event loop, or explicit teardown returns an error (or
/// unwinds), the guard still attempts every terminal restoration action.
pub struct TuiTerminal {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    restoration: RestorationGuard<CrosstermRestoreActions>,
}

impl Deref for TuiTerminal {
    type Target = Terminal<CrosstermBackend<Stdout>>;

    fn deref(&self) -> &Self::Target {
        &self.terminal
    }
}

impl DerefMut for TuiTerminal {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.terminal
    }
}

/// Enter alternate screen mode and return a guarded, configured terminal.
pub fn enter_tui() -> Result<TuiTerminal> {
    let restoration = RestorationGuard::new(CrosstermRestoreActions);
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;
    Ok(TuiTerminal {
        terminal,
        restoration,
    })
}

/// Restore the terminal to its previous state, attempting every action.
pub fn exit_tui(terminal: &mut TuiTerminal) -> Result<()> {
    terminal.restoration.restore()
}

#[cfg(test)]
mod terminal_tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use std::sync::{Arc, Mutex};

    use super::*;

    struct RecordingRestoreActions {
        calls: Arc<Mutex<Vec<RestoreStep>>>,
        failing_step: Option<RestoreStep>,
    }

    impl RestoreActions for RecordingRestoreActions {
        fn restore(&mut self, step: RestoreStep) -> std::io::Result<()> {
            self.calls.lock().unwrap().push(step);
            if self.failing_step == Some(step) {
                Err(std::io::Error::other("injected restore failure"))
            } else {
                Ok(())
            }
        }
    }

    fn recording_actions(
        failing_step: Option<RestoreStep>,
    ) -> (RecordingRestoreActions, Arc<Mutex<Vec<RestoreStep>>>) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        (
            RecordingRestoreActions {
                calls: Arc::clone(&calls),
                failing_step,
            },
            calls,
        )
    }

    fn fail_while_guarded(actions: RecordingRestoreActions) -> Result<()> {
        let _restoration = RestorationGuard::new(actions);
        Err(anyhow!("injected event-loop failure"))
    }

    #[test]
    fn restoration_guard_restores_on_normal_error() {
        let (actions, calls) = recording_actions(None);

        let result = fail_while_guarded(actions);

        assert!(result.is_err());
        assert_eq!(*calls.lock().unwrap(), RestoreStep::ALL);
    }

    #[test]
    fn restoration_guard_restores_while_unwinding_a_panic() {
        let (actions, calls) = recording_actions(None);

        let result = std::panic::catch_unwind(|| {
            let _restoration = RestorationGuard::new(actions);
            panic!("injected event-loop panic");
        });

        assert!(result.is_err());
        assert_eq!(*calls.lock().unwrap(), RestoreStep::ALL);
    }

    #[test]
    fn restoration_attempts_every_action_after_one_fails() {
        let (actions, calls) = recording_actions(Some(RestoreStep::RawMode));
        let mut restoration = RestorationGuard::new(actions);

        let error = restoration
            .restore()
            .expect_err("raw-mode failure should surface");

        assert!(error.to_string().contains("disable raw mode"));
        assert_eq!(*calls.lock().unwrap(), RestoreStep::ALL);
    }

    #[test]
    fn terminal_events_translate_without_another_read() {
        let resize = Event::Resize(120, 42);
        assert!(matches!(
            terminal_event_to_action(&resize, InputMode::Normal),
            Some(TuiAction::Resize(120, 42))
        ));

        let key = Event::Key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('q'),
            crossterm::event::KeyModifiers::NONE,
        ));
        assert!(matches!(
            terminal_event_to_action(&key, InputMode::Normal),
            Some(TuiAction::Quit)
        ));

        assert!(terminal_event_to_action(&Event::FocusGained, InputMode::Normal).is_none());
    }
}
