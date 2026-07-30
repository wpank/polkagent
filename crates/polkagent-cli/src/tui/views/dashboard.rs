//! Dashboard view (F1 — Screen 1.1).
//!
//! Layout tiers (from PRD-13 Appendix B.2.1):
//! - Compact  (< 80 cols):  single column, stacked panels
//! - Standard (80–119):     agents panel + runs panel side-by-side + health bar
//! - Wide     (120+):       three columns — agents, runs, health sidebar
//!
//! All data is read from `&TuiState`; no I/O occurs on the render path.

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
    // Stack: agents (30%) | runs (50%) | health (20%)
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(30),
            Constraint::Percentage(50),
            Constraint::Percentage(20),
        ])
        .split(area);

    render_agents_panel(frame, rows[0], &state.agents, theme);
    render_runs_panel(frame, rows[1], &state.runs, theme);
    render_health_panel(frame, rows[2], &state.health, theme);
}

// ---------------------------------------------------------------------------
// Standard layout (80–119 cols)
// ---------------------------------------------------------------------------

fn render_standard(frame: &mut Frame, area: Rect, state: &TuiState, theme: &Theme) {
    // Rows: top panels (80%) | health bar (20%)
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(8), Constraint::Length(6)])
        .split(area);

    // Top: agents (35%) | runs (65%)
    let top = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(35), Constraint::Percentage(65)])
        .split(rows[0]);

    render_agents_panel(frame, top[0], &state.agents, theme);
    render_runs_panel(frame, top[1], &state.runs, theme);
    render_health_panel(frame, rows[1], &state.health, theme);
}

// ---------------------------------------------------------------------------
// Wide layout (120+ cols)
// ---------------------------------------------------------------------------

fn render_wide(frame: &mut Frame, area: Rect, state: &TuiState, theme: &Theme) {
    // Rows: panels (80%) | health (20%)
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(8), Constraint::Length(6)])
        .split(area);

    // Three columns: agents | runs | health sidebar
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
    render_health_sidebar(frame, cols[2], &state.health, theme);
    render_health_panel(frame, rows[1], &state.health, theme);
}

// ---------------------------------------------------------------------------
// Agents panel
// ---------------------------------------------------------------------------

fn render_agents_panel(
    frame: &mut Frame,
    area: Rect,
    agents: &[AgentSummary],
    theme: &Theme,
) {
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

    let lines: Vec<Line> = visible
        .iter()
        .map(|a| agent_line(a, theme))
        .collect();

    let para = Paragraph::new(lines);
    frame.render_widget(para, inner);
}

fn agent_line<'a>(a: &'a AgentSummary, theme: &'a Theme) -> Line<'a> {
    let glyph_color = match a.state.as_str() {
        "active"    => theme.rose,
        "paused"    => theme.warning,
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
        format!(" {runs} run{s}", runs = a.active_runs, s = if a.active_runs == 1 { "" } else { "s" })
    } else {
        format!(" {}", a.status_label())
    };

    Line::from(vec![
        Span::styled(format!("  {glyph}", glyph = a.glyph()), Style::default().fg(glyph_color)),
        Span::raw(" "),
        Span::styled(name, Style::default().fg(theme.text_primary).add_modifier(Modifier::BOLD)),
        Span::styled(subtitle, Style::default().fg(theme.text_dim)),
    ])
}

// ---------------------------------------------------------------------------
// Runs panel
// ---------------------------------------------------------------------------

fn render_runs_panel(
    frame: &mut Frame,
    area: Rect,
    runs: &[RunSummary],
    theme: &Theme,
) {
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
        .filter(|r| !matches!(r.state.as_str(), "completed" | "failed" | "cancelled" | "timed_out"))
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
        Span::styled(format!("  {glyph}", glyph = r.state_glyph()), Style::default().fg(state_color)),
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

fn render_health_panel(
    frame: &mut Frame,
    area: Rect,
    health: &SystemHealth,
    theme: &Theme,
) {
    let block = styled_block(" SYSTEM HEALTH ", theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let db_color = if health.db_ok { theme.success } else { theme.danger };
    let db_label = if health.db_ok { "OK" } else { "ERROR" };

    let lines = vec![
        Line::from(vec![
            Span::styled("  Database: ", Style::default().fg(theme.text_dim)),
            Span::styled(db_label, Style::default().fg(db_color).add_modifier(Modifier::BOLD)),
            Span::styled(
                format!("  {}", health.db_path),
                Style::default().fg(theme.text_dim),
            ),
        ]),
        Line::from(vec![
            Span::styled("  Agents:   ", Style::default().fg(theme.text_dim)),
            Span::styled(
                health.agent_count.to_string(),
                Style::default().fg(theme.text_primary).add_modifier(Modifier::BOLD),
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
                Style::default().fg(theme.text_primary).add_modifier(Modifier::BOLD),
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

fn render_health_sidebar(
    frame: &mut Frame,
    area: Rect,
    health: &SystemHealth,
    theme: &Theme,
) {
    let block = styled_block(" HEALTH ", theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let db_color = if health.db_ok { theme.success } else { theme.danger };

    let agent_count = health.agent_count.to_string();
    let active_agent_count = health.active_agent_count.to_string();
    let total_run_count = health.total_run_count.to_string();
    let active_run_count = health.active_run_count.to_string();

    let lines = vec![
        stat_line("DB", if health.db_ok { "OK" } else { "ERR" }, db_color, theme),
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
        Span::styled(
            format!("  {label:<8}"),
            Style::default().fg(theme.text_dim),
        ),
        Span::styled(
            value.to_owned(),
            Style::default().fg(value_color).add_modifier(Modifier::BOLD),
        ),
    ])
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn styled_block<'a>(title: &'a str, theme: &'a Theme) -> Block<'a> {
    Block::default()
        .title(Span::styled(
            title,
            Style::default()
                .fg(theme.rose)
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.border))
        .style(Style::default().bg(theme.bg_raised))
}
