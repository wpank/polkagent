//! System tab view (F4 — Screen 6.1 System Health / 6.2 Configuration).
//!
//! Displays configuration values, database statistics, and system state.

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
    Frame,
};

use crate::tui::state::TuiState;
use crate::tui::theme::Theme;

// ---------------------------------------------------------------------------
// Public render entry point
// ---------------------------------------------------------------------------

pub fn render(frame: &mut Frame, area: Rect, state: &TuiState, theme: &Theme) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    render_health(frame, rows[0], state, theme);
    render_config(frame, rows[1], state, theme);
}

// ---------------------------------------------------------------------------
// Health panel
// ---------------------------------------------------------------------------

fn render_health(frame: &mut Frame, area: Rect, state: &TuiState, theme: &Theme) {
    let block = Block::default()
        .title(Span::styled(
            " SYSTEM HEALTH ",
            Style::default().fg(theme.rose).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.border))
        .style(Style::default().bg(theme.bg_raised));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let h = &state.health;
    let db_color = if h.db_ok { theme.success } else { theme.danger };
    let db_label = if h.db_ok { "OK" } else { "ERROR" };
    let sampled = h.sampled_at.format("%Y-%m-%d %H:%M:%S UTC").to_string();
    let agent_total = h.agent_count.to_string();
    let agent_active = h.active_agent_count.to_string();
    let run_total = h.total_run_count.to_string();
    let run_active = h.active_run_count.to_string();
    let sampled_label = format!("  Last sampled: {sampled}");

    let lines = vec![
        section_header("Database", theme),
        kv_line("  Status", db_label, db_color, theme),
        kv_line("  Path", &h.db_path, theme.text_primary, theme),
        Line::from(""),
        section_header("Agents", theme),
        kv_line("  Total", &agent_total, theme.text_primary, theme),
        kv_line("  Active", &agent_active, theme.rose, theme),
        Line::from(""),
        section_header("Runs", theme),
        kv_line("  Total", &run_total, theme.text_primary, theme),
        kv_line("  Working", &run_active, theme.rose, theme),
        Line::from(""),
        Line::from(Span::styled(
            sampled_label,
            Style::default().fg(theme.text_dim),
        )),
    ];

    frame.render_widget(Paragraph::new(lines), inner);
}

// ---------------------------------------------------------------------------
// Config panel
// ---------------------------------------------------------------------------

fn render_config(frame: &mut Frame, area: Rect, state: &TuiState, theme: &Theme) {
    let block = Block::default()
        .title(Span::styled(
            " CONFIGURATION ",
            Style::default().fg(theme.rose).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.border))
        .style(Style::default().bg(theme.bg_raised));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let db_path = state.health.db_path.clone();
    let last_refresh = state
        .last_refresh
        .map(|t| t.format("%Y-%m-%d %H:%M:%S UTC").to_string())
        .unwrap_or_else(|| "(never)".into());
    let err_display = state
        .last_error
        .clone()
        .unwrap_or_else(|| "none".into());
    let err_color = if err_display == "none" { theme.success } else { theme.danger };
    let theme_label = if std::env::var_os("NO_COLOR").is_some() { "no_color" } else { "dark (ROSEDUST)" };

    let lines = vec![
        section_header("Storage", theme),
        kv_line("  Database", &db_path, theme.text_primary, theme),
        Line::from(""),
        section_header("TUI", theme),
        kv_line("  Theme", theme_label, theme.text_primary, theme),
        kv_line("  Last refresh", &last_refresh, theme.text_primary, theme),
        kv_line("  Last error", &err_display, err_color, theme),
        Line::from(""),
        section_header("Keybindings", theme),
        kv_line("  F1-F4 / 1-4", "Switch tabs", theme.text_primary, theme),
        kv_line("  j / k", "Scroll up / down", theme.text_primary, theme),
        kv_line("  Enter", "Select / drill-down", theme.text_primary, theme),
        kv_line("  r / F5", "Refresh data", theme.text_primary, theme),
        kv_line("  q / Ctrl+C", "Quit", theme.text_primary, theme),
    ];

    frame.render_widget(Paragraph::new(lines), inner);
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn section_header(title: &str, theme: &Theme) -> Line<'static> {
    Line::from(Span::styled(
        format!("  {title}"),
        Style::default()
            .fg(theme.bone)
            .add_modifier(Modifier::BOLD),
    ))
}

fn kv_line(
    key: &str,
    value: &str,
    value_color: ratatui::style::Color,
    theme: &Theme,
) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{key:<18}"),
            Style::default().fg(theme.text_dim),
        ),
        Span::styled(value.to_owned(), Style::default().fg(value_color)),
    ])
}
