//! System tab view (F4 — Screen 6.1 System Health / 6.2 Configuration).
//!
//! Displays:
//! - Database health, size, connection count
//! - Config source files loaded
//! - Loaded providers and their status
//! - Skill/tool count
//! - Memory usage statistics
//! - TUI configuration and keybindings
//!
//! ## Widget integration
//!
//! - `chain_status`    — shows configured chain connection information
//! - `balance_display` — shows budget remaining as a pseudo-balance panel

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
    Frame,
};

use crate::tui::state::TuiState;
use crate::tui::theme::Theme;
use crate::tui::widgets::balance_display::{self, AssetBalance, BalanceDisplayData, Denomination};
use crate::tui::widgets::chain_status::{self, ChainStatusData};

// ---------------------------------------------------------------------------
// Public render entry point
// ---------------------------------------------------------------------------

pub fn render(frame: &mut Frame, area: Rect, state: &TuiState, theme: &Theme) {
    // Wide layout: two columns for health+stats vs config+chain.
    if area.width >= 100 {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(area);

        let left_rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(cols[0]);

        let right_rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Percentage(40),
                Constraint::Percentage(30),
                Constraint::Percentage(30),
            ])
            .split(cols[1]);

        render_health(frame, left_rows[0], state, theme);
        render_stats(frame, left_rows[1], state, theme);
        render_config(frame, right_rows[0], state, theme);
        render_chain_status_panel(frame, right_rows[1], state, theme);
        render_balance_panel(frame, right_rows[2], state, theme);
    } else {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Percentage(30),
                Constraint::Percentage(20),
                Constraint::Percentage(20),
                Constraint::Percentage(15),
                Constraint::Percentage(15),
            ])
            .split(area);

        render_health(frame, rows[0], state, theme);
        render_stats(frame, rows[1], state, theme);
        render_config(frame, rows[2], state, theme);
        render_chain_status_panel(frame, rows[3], state, theme);
        render_balance_panel(frame, rows[4], state, theme);
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

    // DB size — avoid filesystem I/O on the render path.
    let db_size: String = if state.health.db_ok {
        "see health".into()
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

    // Budget remaining.
    let budget_pct = format!("{:.1}%", state.budget_remaining * 100.0);
    let budget_color = if state.budget_remaining > 0.5 {
        theme.success
    } else if state.budget_remaining > 0.2 {
        theme.warning
    } else {
        theme.danger
    };

    let lines = vec![
        section_header("Database", theme),
        kv_line("  Size on disk", &db_size, theme.text_primary, theme),
        kv_line(
            "  Pending effects",
            &pending_count,
            if state.pending_approvals.is_empty() {
                theme.success
            } else {
                theme.warning
            },
            theme,
        ),
        Line::from(""),
        section_header("Memory Store", theme),
        kv_line("  Loaded entries", &memory_count, theme.text_primary, theme),
        Line::from(""),
        section_header("Process", theme),
        kv_line("  RSS (approx)", &rss, theme.text_primary, theme),
        Line::from(""),
        section_header("Budget", theme),
        kv_line("  Remaining", &budget_pct, budget_color, theme),
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
    let err_display = state.last_error.clone().unwrap_or_else(|| "none".into());
    let err_color = if err_display == "none" {
        theme.success
    } else {
        theme.danger
    };
    let theme_label = if std::env::var_os("NO_COLOR").is_some() {
        "no_color"
    } else {
        "dark (ROSEDUST)"
    };

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
        kv_line(
            "  Loaded from",
            &config_sources_str,
            theme.text_primary,
            theme,
        ),
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
        kv_line(
            "  a / d",
            "Approve / deny effect",
            theme.text_primary,
            theme,
        ),
        kv_line("  Del", "Delete memory entry", theme.text_primary, theme),
        kv_line("  g / G", "Jump to top / bottom", theme.text_primary, theme),
        kv_line("  f", "Cycle filter (audit)", theme.text_primary, theme),
        kv_line("  r", "Refresh data", theme.text_primary, theme),
        kv_line("  q / Ctrl+C", "Quit", theme.text_primary, theme),
    ];

    // Environment overrides section.
    let env_vars = ["POLKAGENT_DB_PATH", "POLKAGENT_MEMORY_DB_PATH", "NO_COLOR"];
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
            let display = if val.len() > 30 {
                format!("{}…", &val[..29])
            } else {
                val
            };
            lines.push(kv_line(&format!("  {v}"), &display, theme.bone, theme));
        }
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

// ---------------------------------------------------------------------------
// Chain status widget panel
// ---------------------------------------------------------------------------

/// Render the chain status widget showing live chain information.
///
/// Chain data is populated by the [`crate::tui::db::ChainPoller`] which
/// periodically queries the configured RPC endpoint. When no RPC URL is
/// set the panel gracefully shows "Not connected" with zero block numbers.
fn render_chain_status_panel(frame: &mut Frame, area: Rect, state: &TuiState, theme: &Theme) {
    // Use live chain data from state (populated by ChainPoller).
    // Fall back to environment heuristic if the chain name has not been set yet.
    let chain_name = if state.chain_name.is_empty() {
        detect_chain_name()
    } else {
        state.chain_name.clone()
    };

    let data = ChainStatusData {
        chain_name: &chain_name,
        connected: state.chain_connected,
        best_block: state.best_block,
        finalized_block: state.finalized_block,
        node_version: if state.node_version.is_empty() {
            None
        } else {
            Some(state.node_version.as_str())
        },
        metadata_version: 14,
        metadata_freshness: if state.chain_connected {
            "fresh"
        } else {
            "stale"
        },
    };

    chain_status::render(frame, area, &data, theme);
}

// ---------------------------------------------------------------------------
// Balance display widget panel
// ---------------------------------------------------------------------------

/// Render the balance display widget showing budget remaining as a DOT balance.
///
/// The budget remaining fraction from `TuiState` is converted into a
/// pseudo-planck value with DOT denomination for a realistic display.
fn render_balance_panel(frame: &mut Frame, area: Rect, state: &TuiState, theme: &Theme) {
    // Represent the budget as a DOT balance: 1 DOT = 10^10 plancks.
    // We use a notional "10 DOT budget" and scale by the remaining fraction.
    let total_plancks: u128 = 10 * 10_000_000_000; // 10 DOT in plancks
    let free_plancks = (total_plancks as f64 * state.budget_remaining) as u128;
    let spent_plancks = total_plancks.saturating_sub(free_plancks);

    let assets = vec![AssetBalance {
        label: "Budget (DOT equiv)".to_owned(),
        free: free_plancks,
        reserved: spent_plancks,
        frozen: 0,
        denomination: Denomination::Dot,
    }];

    let data = BalanceDisplayData {
        assets: &assets,
        existential_deposit: 10_000_000_000, // 1 DOT
        ed_denomination: Denomination::Dot,
    };

    balance_display::render(frame, area, &data, theme);
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn section_header(title: &str, theme: &Theme) -> Line<'static> {
    Line::from(Span::styled(
        format!("  {title}"),
        Style::default().fg(theme.bone).add_modifier(Modifier::BOLD),
    ))
}

fn kv_line(
    key: &str,
    value: &str,
    value_color: ratatui::style::Color,
    theme: &Theme,
) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{key:<24}"), Style::default().fg(theme.text_dim)),
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
/// Returns `None` when unavailable or on permission error.
fn process_rss_kb() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let status = std::fs::read_to_string("/proc/self/status").ok()?;
        for line in status.lines() {
            if let Some(rest) = line.strip_prefix("VmRSS:") {
                let kb: u64 = rest.split_whitespace().next()?.parse().ok()?;
                return Some(kb);
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        // Use `ps` to read RSS for the current process (in KiB).
        let pid = std::process::id();
        let output = std::process::Command::new("ps")
            .args(["-o", "rss=", "-p", &pid.to_string()])
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&output.stdout);
        let kb: u64 = text.trim().parse().ok()?;
        return Some(kb);
    }

    #[allow(unreachable_code)]
    None
}

/// Return a list of config file paths that exist on disk.
///
/// Matches the paths checked by `ConfigLoader`:
///   1. Global: `dirs::config_dir()/polkagent/polkagent.toml`
///   2. Project: `.polkagent/polkagent.toml` (walks up from CWD)
fn detect_config_sources() -> Vec<String> {
    let mut sources = Vec::new();

    if let Some(global) = polkagent_config::loader::global_config_path() {
        if global.exists() {
            sources.push(global.display().to_string());
        }
    }

    if let Some(project) = polkagent_config::loader::find_project_config() {
        sources.push(project.display().to_string());
    }

    sources
}

/// Detect the configured chain name from environment variables.
fn detect_chain_name() -> String {
    if let Ok(url) = std::env::var("POLKAGENT_RPC_URL") {
        if url.contains("westend") {
            return "Westend".to_owned();
        } else if url.contains("kusama") {
            return "Kusama".to_owned();
        } else if url.contains("polkadot") {
            return "Polkadot".to_owned();
        }
    }
    // Default: read from POLKAGENT_CHAIN or fall back.
    std::env::var("POLKAGENT_CHAIN").unwrap_or_else(|_| "Polkadot".to_owned())
}
