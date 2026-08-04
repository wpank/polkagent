//! Dashboard view (F1 — Screen 1.1).
//!
//! Layout tiers (from PRD-13 Appendix B.2.1):
//! - Compact  (< 80 cols):  single column, stacked panels
//! - Standard (80–119):     agents panel + runs panel side-by-side + health bar
//! - Wide     (120+):       three columns — agents, runs, health sidebar
//!
//! All data is read from `&TuiState`; no I/O occurs on the render path.
//!
//! ## Widget integration
//!
//! A horizontal widget strip at the bottom of every layout shows:
//! - `token_sparkline` — recent per-turn token usage (bone/amber Braille)
//! - `progress_bar`    — active run progress (turns completed / estimated max)
//! - `context_gauge`   — context-window fill for the active run
//! - error count summary from `TuiState::error_count`

use chrono::Utc;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
    Frame,
};

use crate::tui::state::{AgentSummary, RunSummary, SystemHealth, TuiState};
use crate::tui::theme::Theme;
use crate::tui::widgets::{context_gauge, progress_bar, token_sparkline};

// ---------------------------------------------------------------------------
// Layout breakpoints
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Breakpoint {
    Compact,
    Standard,
    Wide,
}

impl Breakpoint {
    fn from_cols(cols: u16) -> Self {
        if cols >= 120 {
            Self::Wide
        } else if cols >= 80 {
            Self::Standard
        } else {
            Self::Compact
        }
    }
}

// ---------------------------------------------------------------------------
// Public render entry point
// ---------------------------------------------------------------------------

/// Render the dashboard view into `area`.
pub fn render(frame: &mut Frame, area: Rect, state: &TuiState, theme: &Theme) {
    let bp = Breakpoint::from_cols(area.width);

    match bp {
        Breakpoint::Compact => render_compact(frame, area, state, theme),
        Breakpoint::Standard => render_standard(frame, area, state, theme),
        Breakpoint::Wide => render_wide(frame, area, state, theme),
    }
}

// ---------------------------------------------------------------------------
// Compact layout (< 80 cols)
// ---------------------------------------------------------------------------

fn render_compact(frame: &mut Frame, area: Rect, state: &TuiState, theme: &Theme) {
    // Stack: agents | runs | health | widget strip
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(28),
            Constraint::Percentage(44),
            Constraint::Percentage(16),
            Constraint::Percentage(12),
        ])
        .split(area);

    render_agents_panel(frame, rows[0], &state.agents, theme);
    render_runs_panel(frame, rows[1], &state.runs, theme);
    render_health_panel(frame, rows[2], &state.health, theme);
    render_widget_strip(frame, rows[3], state, theme);
}

// ---------------------------------------------------------------------------
// Standard layout (80–119 cols)
// ---------------------------------------------------------------------------

fn render_standard(frame: &mut Frame, area: Rect, state: &TuiState, theme: &Theme) {
    // Rows: top panels | health bar (6) | widget strip (4)
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(8),
            Constraint::Length(6),
            Constraint::Length(4),
        ])
        .split(area);

    // Top: agents (35%) | runs (65%)
    let top = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(35), Constraint::Percentage(65)])
        .split(rows[0]);

    render_agents_panel(frame, top[0], &state.agents, theme);
    render_runs_panel(frame, top[1], &state.runs, theme);
    render_health_panel(frame, rows[1], &state.health, theme);
    render_widget_strip(frame, rows[2], state, theme);
}

// ---------------------------------------------------------------------------
// Wide layout (120+ cols)
// ---------------------------------------------------------------------------

fn render_wide(frame: &mut Frame, area: Rect, state: &TuiState, theme: &Theme) {
    // Rows: panels | widget strip (4)
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(8), Constraint::Length(4)])
        .split(area);

    // Three columns: agents | runs | sidebar (health + activity)
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(25),
            Constraint::Percentage(50),
            Constraint::Percentage(25),
        ])
        .split(rows[0]);

    render_agents_panel(frame, cols[0], &state.agents, theme);
    render_runs_panel(frame, cols[1], &state.runs, theme);

    // Split the right sidebar: health (top) + recent activity (bottom)
    let sidebar = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(8), Constraint::Min(4)])
        .split(cols[2]);

    render_health_sidebar(frame, sidebar[0], &state.health, theme);
    render_activity_panel(frame, sidebar[1], state, theme);
    render_widget_strip(frame, rows[1], state, theme);
}

// ---------------------------------------------------------------------------
// Agents panel
// ---------------------------------------------------------------------------

fn render_agents_panel(frame: &mut Frame, area: Rect, agents: &[AgentSummary], theme: &Theme) {
    let block = styled_block(" AGENTS ", theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if agents.is_empty() {
        let empty = Paragraph::new(Span::styled(
            "  No agents found.",
            Style::default().fg(theme.text_dim),
        ));
        frame.render_widget(empty, inner);
        return;
    }

    // Show up to `inner.height` agents.
    let visible = agents
        .iter()
        .take(inner.height as usize)
        .collect::<Vec<_>>();

    let lines: Vec<Line> = visible.iter().map(|a| agent_line(a, theme)).collect();

    let para = Paragraph::new(lines);
    frame.render_widget(para, inner);
}

fn agent_line<'a>(a: &'a AgentSummary, theme: &'a Theme) -> Line<'a> {
    let glyph_color = match a.state.as_str() {
        "active" => theme.rose,
        "paused" => theme.warning,
        "configured" | "created" => theme.text_dim,
        _ => theme.danger,
    };

    // Truncate name to fit a typical panel width.
    let name_max = 18usize;
    let name = if a.name.len() > name_max {
        format!("{}…", &a.name[..name_max.saturating_sub(1)])
    } else {
        a.name.clone()
    };

    let subtitle = if a.active_runs > 0 {
        format!(
            " {runs} run{s}",
            runs = a.active_runs,
            s = if a.active_runs == 1 { "" } else { "s" }
        )
    } else {
        format!(" {}", a.status_label())
    };

    Line::from(vec![
        Span::styled(
            format!("  {glyph}", glyph = a.glyph()),
            Style::default().fg(glyph_color),
        ),
        Span::raw(" "),
        Span::styled(
            name,
            Style::default()
                .fg(theme.text_primary)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(subtitle, Style::default().fg(theme.text_dim)),
    ])
}

// ---------------------------------------------------------------------------
// Runs panel
// ---------------------------------------------------------------------------

fn render_runs_panel(frame: &mut Frame, area: Rect, runs: &[RunSummary], theme: &Theme) {
    let block = styled_block(" ACTIVE RUNS ", theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if runs.is_empty() {
        let empty = Paragraph::new(Span::styled(
            "  No runs found.",
            Style::default().fg(theme.text_dim),
        ));
        frame.render_widget(empty, inner);
        return;
    }

    let now = Utc::now();
    let active: Vec<&RunSummary> = runs
        .iter()
        .filter(|r| {
            !matches!(
                r.state.as_str(),
                "completed" | "failed" | "cancelled" | "timed_out"
            )
        })
        .collect();

    let to_show: Vec<&RunSummary> = if active.is_empty() {
        runs.iter().take(inner.height as usize).collect()
    } else {
        active.into_iter().take(inner.height as usize).collect()
    };

    let lines: Vec<Line> = to_show.iter().map(|r| run_line(r, now, theme)).collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

fn run_line<'a>(r: &'a RunSummary, now: chrono::DateTime<Utc>, theme: &'a Theme) -> Line<'a> {
    let state_color = theme.status_color(&r.state);
    let duration = r.duration_display(now);

    // Truncate agent name.
    let agent_max = 14usize;
    let agent = if r.agent_name.len() > agent_max {
        format!("{}…", &r.agent_name[..agent_max.saturating_sub(1)])
    } else {
        r.agent_name.clone()
    };

    Line::from(vec![
        Span::styled(
            format!("  {glyph}", glyph = r.state_glyph()),
            Style::default().fg(state_color),
        ),
        Span::raw(" "),
        Span::styled(r.short_id.clone(), Style::default().fg(theme.text_dim)),
        Span::raw("  "),
        Span::styled(agent, Style::default().fg(theme.text_primary)),
        Span::raw("  "),
        Span::styled(r.state.clone(), Style::default().fg(state_color)),
        Span::raw("  "),
        Span::styled(duration, Style::default().fg(theme.text_dim)),
    ])
}

// ---------------------------------------------------------------------------
// Health panel (bottom bar, horizontal)
// ---------------------------------------------------------------------------

fn render_health_panel(frame: &mut Frame, area: Rect, health: &SystemHealth, theme: &Theme) {
    let block = styled_block(" SYSTEM HEALTH ", theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let db_color = if health.db_ok {
        theme.success
    } else {
        theme.danger
    };
    let db_label = if health.db_ok { "OK" } else { "ERROR" };

    let lines = vec![
        Line::from(vec![
            Span::styled("  Database: ", Style::default().fg(theme.text_dim)),
            Span::styled(
                db_label,
                Style::default().fg(db_color).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("  {}", health.db_path),
                Style::default().fg(theme.text_dim),
            ),
        ]),
        Line::from(vec![
            Span::styled("  Agents:   ", Style::default().fg(theme.text_dim)),
            Span::styled(
                health.agent_count.to_string(),
                Style::default()
                    .fg(theme.text_primary)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" total  ", Style::default().fg(theme.text_dim)),
            Span::styled(
                health.active_agent_count.to_string(),
                Style::default().fg(theme.rose),
            ),
            Span::styled(" active", Style::default().fg(theme.text_dim)),
        ]),
        Line::from(vec![
            Span::styled("  Runs:     ", Style::default().fg(theme.text_dim)),
            Span::styled(
                health.total_run_count.to_string(),
                Style::default()
                    .fg(theme.text_primary)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" total  ", Style::default().fg(theme.text_dim)),
            Span::styled(
                health.active_run_count.to_string(),
                Style::default().fg(theme.rose),
            ),
            Span::styled(" active", Style::default().fg(theme.text_dim)),
        ]),
    ];

    frame.render_widget(Paragraph::new(lines), inner);
}

// ---------------------------------------------------------------------------
// Health sidebar (wide layout only)
// ---------------------------------------------------------------------------

fn render_health_sidebar(frame: &mut Frame, area: Rect, health: &SystemHealth, theme: &Theme) {
    let block = styled_block(" HEALTH ", theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let db_color = if health.db_ok {
        theme.success
    } else {
        theme.danger
    };

    let agent_count = health.agent_count.to_string();
    let active_agent_count = health.active_agent_count.to_string();
    let total_run_count = health.total_run_count.to_string();
    let active_run_count = health.active_run_count.to_string();

    let lines = vec![
        stat_line(
            "DB",
            if health.db_ok { "OK" } else { "ERR" },
            db_color,
            theme,
        ),
        stat_line("Agents", &agent_count, theme.text_primary, theme),
        stat_line("Active", &active_agent_count, theme.rose, theme),
        stat_line("Runs", &total_run_count, theme.text_primary, theme),
        stat_line("Working", &active_run_count, theme.rose, theme),
    ];

    frame.render_widget(Paragraph::new(lines), inner);
}

fn stat_line(
    label: &str,
    value: &str,
    value_color: ratatui::style::Color,
    theme: &Theme,
) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("  {label:<8}"), Style::default().fg(theme.text_dim)),
        Span::styled(
            value.to_owned(),
            Style::default()
                .fg(value_color)
                .add_modifier(Modifier::BOLD),
        ),
    ])
}

// ---------------------------------------------------------------------------
// Activity panel (wide layout sidebar — recent activity & chain status)
// ---------------------------------------------------------------------------

/// Render a compact activity summary below the health sidebar in the wide
/// layout.  Shows chain connection status, budget remaining, and recent
/// run events so the right column provides useful information beyond
/// health metrics alone.
fn render_activity_panel(frame: &mut Frame, area: Rect, state: &TuiState, theme: &Theme) {
    let block = styled_block(" ACTIVITY ", theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines: Vec<Line> = Vec::new();

    // Chain connection status
    let (chain_color, chain_label) = if state.chain_connected {
        (theme.success, "Connected")
    } else {
        (theme.text_dim, "Offline")
    };
    let chain_display = if state.chain_connected && !state.chain_name.is_empty() {
        format!("{} #{}", state.chain_name, state.best_block)
    } else {
        chain_label.to_owned()
    };
    lines.push(Line::from(vec![
        Span::styled("  Chain   ", Style::default().fg(theme.text_dim)),
        Span::styled(
            chain_display,
            Style::default()
                .fg(chain_color)
                .add_modifier(Modifier::BOLD),
        ),
    ]));

    // Budget remaining
    let budget_pct = (state.budget_remaining * 100.0).round() as u64;
    let budget_color = if budget_pct > 50 {
        theme.success
    } else if budget_pct > 20 {
        theme.warning
    } else {
        theme.danger
    };
    lines.push(Line::from(vec![
        Span::styled("  Budget  ", Style::default().fg(theme.text_dim)),
        Span::styled(
            format!("{budget_pct}%"),
            Style::default()
                .fg(budget_color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" remaining", Style::default().fg(theme.text_dim)),
    ]));

    // Separator
    lines.push(Line::from(""));

    // Recent events header
    if !state.run_events.is_empty() {
        lines.push(Line::from(Span::styled(
            "  Recent events",
            Style::default().fg(theme.text_dim),
        )));

        let max_events = (inner.height as usize).saturating_sub(lines.len());
        for event in state.run_events.iter().take(max_events) {
            let ts = event.timestamp.format("%H:%M:%S").to_string();
            // Truncate description to fit the sidebar width.
            let desc_max = inner.width as usize;
            let desc_max = desc_max.saturating_sub(12); // account for timestamp + padding
            let desc = if event.description.len() > desc_max {
                format!("{}…", &event.description[..desc_max.saturating_sub(1)])
            } else {
                event.description.clone()
            };
            lines.push(Line::from(vec![
                Span::styled(format!("  {ts} "), Style::default().fg(theme.text_dim)),
                Span::styled(desc, Style::default().fg(theme.text_primary)),
            ]));
        }
    } else {
        lines.push(Line::from(Span::styled(
            "  No recent events.",
            Style::default().fg(theme.text_dim),
        )));
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

// ---------------------------------------------------------------------------
// Widget strip (token sparkline, progress bar, context gauge, error count)
// ---------------------------------------------------------------------------

/// Render the dashboard widget strip — a compact horizontal band of live
/// widgets showing token usage, run progress, context fill, and error count.
fn render_widget_strip(frame: &mut Frame, area: Rect, state: &TuiState, theme: &Theme) {
    if area.height == 0 {
        return;
    }

    // Horizontal split: sparkline | progress bar | context gauge | error count
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(35), // token sparkline
            Constraint::Percentage(25), // run progress bar
            Constraint::Percentage(25), // context gauge
            Constraint::Percentage(15), // error summary
        ])
        .split(area);

    // ── Token sparkline ───────────────────────────────────────────────────
    {
        let block = styled_block(" Token Usage ", theme);
        let inner = block.inner(cols[0]);
        frame.render_widget(block, cols[0]);

        let limit = state
            .token_history
            .iter()
            .copied()
            .max()
            .unwrap_or(1)
            .max(1);

        if inner.height >= 2 {
            let parts = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(1), Constraint::Min(1)])
                .split(inner);

            let last = state.token_history.last().copied().unwrap_or(0);
            let label = if state.token_history.is_empty() {
                "no data".to_owned()
            } else {
                format!("last: {last}t")
            };
            frame.render_widget(
                Paragraph::new(Span::styled(label, Style::default().fg(theme.text_dim))),
                parts[0],
            );
            token_sparkline::render(frame, parts[1], &state.token_history, limit, 0.75, theme);
        } else {
            token_sparkline::render(frame, inner, &state.token_history, limit, 0.75, theme);
        }
    }

    // ── Active run progress bar ───────────────────────────────────────────
    {
        let block = styled_block(" Run Progress ", theme);
        let inner = block.inner(cols[1]);
        frame.render_widget(block, cols[1]);

        let (turns_done, max_turns) = active_run_progress(state);
        let ratio = if max_turns > 0 {
            (turns_done as f64 / max_turns as f64).clamp(0.0, 1.0)
        } else {
            0.0
        };

        if inner.height >= 2 {
            let parts = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(1), Constraint::Min(1)])
                .split(inner);

            let label = if max_turns == 0 {
                "idle".to_owned()
            } else {
                format!("{turns_done}/{max_turns} turns")
            };
            frame.render_widget(
                Paragraph::new(Span::styled(label, Style::default().fg(theme.text_dim))),
                parts[0],
            );
            progress_bar::render(frame, parts[1], ratio, theme);
        } else {
            progress_bar::render(frame, inner, ratio, theme);
        }
    }

    // ── Context gauge ─────────────────────────────────────────────────────
    {
        let block = styled_block(" Context ", theme);
        let inner = block.inner(cols[2]);
        frame.render_widget(block, cols[2]);

        if inner.height >= 2 {
            let parts = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(1), Constraint::Min(1)])
                .split(inner);

            frame.render_widget(
                Paragraph::new(Span::styled(
                    "window fill",
                    Style::default().fg(theme.text_dim),
                )),
                parts[0],
            );
            context_gauge::render(
                frame,
                parts[1],
                state.context_used,
                state.context_total,
                theme,
            );
        } else {
            context_gauge::render(frame, inner, state.context_used, state.context_total, theme);
        }
    }

    // ── Error count summary ───────────────────────────────────────────────
    {
        let block = styled_block(" Errors ", theme);
        let inner = block.inner(cols[3]);
        frame.render_widget(block, cols[3]);

        let (err_color, err_text) = if state.error_count == 0 {
            (theme.success, "0 errors".to_owned())
        } else {
            (
                theme.danger,
                format!(
                    "{} error{}",
                    state.error_count,
                    if state.error_count == 1 { "" } else { "s" },
                ),
            )
        };

        let lines = vec![
            Line::from(Span::styled(
                err_text,
                Style::default().fg(err_color).add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled(
                "last hour",
                Style::default().fg(theme.text_dim),
            )),
        ];
        frame.render_widget(Paragraph::new(lines), inner);
    }
}

/// Derive `(turns_done, estimated_max_turns)` for the progress bar.
///
/// Prefers the explicitly selected `RunDetail`; falls back to the most-active
/// non-terminal run in the summary list.  Returns `(0, 0)` when no runs are
/// active.
fn active_run_progress(state: &TuiState) -> (u32, u32) {
    if let Some(detail) = &state.run_detail {
        let done = detail.turn_count;
        // Heuristic: max is turns_done + a headroom of 5 (or at least 20).
        let max = done.max(20).max(done + 5);
        return (done, max);
    }

    let active = state.runs.iter().filter(|r| {
        matches!(
            r.state.as_str(),
            "working" | "started" | "created" | "queued"
        )
    });
    if let Some(run) = active.max_by_key(|r| r.turn_count) {
        let done = run.turn_count;
        let max = done.max(20).max(done + 5);
        return (done, max);
    }

    (0, 0)
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn styled_block<'a>(title: &'a str, theme: &'a Theme) -> Block<'a> {
    Block::default()
        .title(Span::styled(
            title,
            Style::default().fg(theme.rose).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.border))
        .style(Style::default().bg(theme.bg_raised))
}
