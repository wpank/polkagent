//! System tab view (F4 — Screen 6.1 System Health / 6.2 Configuration).
//!
//! Displays:
//! - Database health, size, connection count
//! - Config source files loaded
//! - Loaded providers and their status
//! - Skill/tool count
//! - Memory usage statistics
//! - TUI configuration and keybindings

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
    // Wide layout: two columns for health+stats vs config.
    if area.width >= 100 {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(area);

        let left_rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
            .split(cols[0]);

        render_health(frame, left_rows[0], state, theme);
        render_stats(frame, left_rows[1], state, theme);
        render_config(frame, cols[1], state, theme);
    } else {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(40), Constraint::Percentage(30), Constraint::Percentage(30)])
            .split(area);

        render_health(frame, rows[0], state, theme);
        render_stats(frame, rows[1], state, theme);
        render_config(frame, rows[2], state, theme);
    }
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
// Stats panel (DB size, memory, pending effects count)
// ---------------------------------------------------------------------------

fn render_stats(frame: &mut Frame, area: Rect, state: &TuiState, theme: &Theme) {
    let block = Block::default()
        .title(Span::styled(
            " STATISTICS ",
            Style::default().fg(theme.rose).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.border))
        .style(Style::default().bg(theme.bg_raised));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    // DB size on disk.
    let db_size = if state.health.db_ok {
        std::fs::metadata(&state.health.db_path)
            .map(|m| format_bytes(m.len()))
            .unwrap_or_else(|_| "unknown".into())
    } else {
        "N/A".into()
    };

    // Pending effects count.
    let pending_count = state.pending_approvals.len().to_string();

    // Memory entries count.
    let memory_count = state.memory_entries.len().to_string();

    // Process memory usage (RSS via /proc/self/status or similar).
    let rss = process_rss_kb()
        .map(|kb| format_bytes(kb * 1024))
        .unwrap_or_else(|| "unknown".into());

    let lines = vec![
        section_header("Database", theme),
        kv_line("  Size on disk", &db_size, theme.text_primary, theme),
        kv_line("  Pending effects", &pending_count,
            if state.pending_approvals.is_empty() { theme.success } else { theme.warning },
            theme),
        Line::from(""),
        section_header("Memory Store", theme),
        kv_line("  Loaded entries", &memory_count, theme.text_primary, theme),
        Line::from(""),
        section_header("Process", theme),
        kv_line("  RSS (approx)", &rss, theme.text_primary, theme),
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

    // Config source detection.
    let config_sources = detect_config_sources();
    let config_sources_str = if config_sources.is_empty() {
        "(none found)".to_owned()
    } else {
        config_sources.join(", ")
    };

    let mut lines = vec![
        section_header("Storage", theme),
        kv_line("  Database", &db_path, theme.text_primary, theme),
        Line::from(""),
        section_header("Config Sources", theme),
        kv_line("  Loaded from", &config_sources_str, theme.text_primary, theme),
        Line::from(""),
        section_header("TUI", theme),
        kv_line("  Theme", theme_label, theme.text_primary, theme),
        kv_line("  Last refresh", &last_refresh, theme.text_primary, theme),
        kv_line("  Last error", &err_display, err_color, theme),
        Line::from(""),
        section_header("Keybindings", theme),
        kv_line("  F1-F8 / 1-8", "Switch tabs", theme.text_primary, theme),
        kv_line("  j / k", "Scroll up / down", theme.text_primary, theme),
        kv_line("  Enter", "Select / drill-down", theme.text_primary, theme),
        kv_line("  a / d", "Approve / deny effect", theme.text_primary, theme),
        kv_line("  Del", "Delete memory entry", theme.text_primary, theme),
        kv_line("  g / G", "Jump to top / bottom", theme.text_primary, theme),
        kv_line("  f", "Cycle filter (audit)", theme.text_primary, theme),
        kv_line("  r", "Refresh data", theme.text_primary, theme),
        kv_line("  q / Ctrl+C", "Quit", theme.text_primary, theme),
    ];

    // Environment overrides section.
    let env_vars = [
        "POLKAGENT_DB_PATH",
        "POLKAGENT_MEMORY_DB_PATH",
        "NO_COLOR",
    ];
    let set_vars: Vec<String> = env_vars
        .iter()
        .filter(|v| std::env::var_os(v).is_some())
        .map(|v| v.to_string())
        .collect();

    if !set_vars.is_empty() {
        lines.push(Line::from(""));
        lines.push(section_header("Env Overrides", theme));
        for v in &set_vars {
            let val = std::env::var(v).unwrap_or_default();
            let display = if val.len() > 30 { format!("{}…", &val[..29]) } else { val };
            lines.push(kv_line(&format!("  {v}"), &display, theme.bone, theme));
        }
    }

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
            format!("{key:<24}"),
            Style::default().fg(theme.text_dim),
        ),
        Span::styled(value.to_owned(), Style::default().fg(value_color)),
    ])
}

/// Format a byte count for compact display.
fn format_bytes(n: u64) -> String {
    if n >= 1_073_741_824 {
        format!("{:.1} GiB", n as f64 / 1_073_741_824.0)
    } else if n >= 1_048_576 {
        format!("{:.1} MiB", n as f64 / 1_048_576.0)
    } else if n >= 1024 {
        format!("{:.1} KiB", n as f64 / 1024.0)
    } else {
        format!("{n} B")
    }
}

/// Attempt to read the process RSS from the OS.
/// Returns `None` when unavailable (non-Linux, or permission error).
fn process_rss_kb() -> Option<u64> {
    // /proc/self/status is Linux-specific.
    #[cfg(target_os = "linux")]
    {
        let status = std::fs::read_to_string("/proc/self/status").ok()?;
        for line in status.lines() {
            if let Some(rest) = line.strip_prefix("VmRSS:") {
                let kb: u64 = rest
                    .split_whitespace()
                    .next()?
                    .parse()
                    .ok()?;
                return Some(kb);
            }
        }
    }
    None
}

/// Return a list of config file paths that exist on disk.
fn detect_config_sources() -> Vec<String> {
    let mut sources = Vec::new();

    let home = std::env::var("HOME").unwrap_or_default();
    let candidates = [
        format!("{home}/.config/polkagent/polkagent.toml"),
        "/etc/polkagent/polkagent.toml".to_owned(),
        "polkagent.toml".to_owned(),
    ];

    for path in &candidates {
        if std::path::Path::new(path).exists() {
            sources.push(path.clone());
        }
    }

    // Also check POLKAGENT_CONFIG env var.
    if let Ok(p) = std::env::var("POLKAGENT_CONFIG") {
        if !sources.contains(&p) {
            sources.push(p);
        }
    }

    sources
}
