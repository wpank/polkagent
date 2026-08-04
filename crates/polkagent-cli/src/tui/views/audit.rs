//! Audit Log view — scrollable event log with filtering.
//!
//! Shows all audit events across all agents/runs. Events are color-coded by
//! severity:
//!
//! - info  — bone (`#C8B890`)
//! - warn  — amber/warning (`#AA8855`)
//! - error — rose/danger (`#AA5060`)
//!
//! Keyboard:
//! - j/k — navigate
//! - G   — jump to latest (bottom)
//! - g   — jump to top
//! - f   — cycle filter (kind → agent → run → none)
//! - Esc — back
//!
//! Filters are applied client-side over the loaded `audit_log` slice.

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph, Wrap},
    Frame,
};

use crate::tui::state::{AuditEvent, TuiState};
use crate::tui::theme::Theme;

// ---------------------------------------------------------------------------
// Public render entry point
// ---------------------------------------------------------------------------

/// Render the audit log view into `area`.
pub fn render(frame: &mut Frame, area: Rect, state: &TuiState, theme: &Theme) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(4)])
        .split(area);

    render_filter_bar(frame, rows[0], &state.audit_filter, theme);
    render_log(frame, rows[1], state, theme);
}

// ---------------------------------------------------------------------------
// Filter bar
// ---------------------------------------------------------------------------

fn render_filter_bar(frame: &mut Frame, area: Rect, filter: &AuditFilter, theme: &Theme) {
    let filter_text = match filter {
        AuditFilter::None => " Filter: ALL  (f: cycle filter) ".to_owned(),
        AuditFilter::BySeverity(sev) => format!(" Filter: severity={sev}  (f: cycle) "),
        AuditFilter::ByAgent(agent) => format!(" Filter: agent={agent}  (f: cycle) "),
        AuditFilter::ByKind(kind) => format!(" Filter: kind={kind}  (f: cycle) "),
    };

    let filter_color = match filter {
        AuditFilter::None => theme.text_dim,
        AuditFilter::BySeverity(_) => theme.warning,
        AuditFilter::ByAgent(_) => theme.rose,
        AuditFilter::ByKind(_) => theme.dream,
    };

    frame.render_widget(
        Paragraph::new(Span::styled(filter_text, Style::default().fg(filter_color))),
        area,
    );
}

// ---------------------------------------------------------------------------
// Log panel
// ---------------------------------------------------------------------------

fn render_log(frame: &mut Frame, area: Rect, state: &TuiState, theme: &Theme) {
    let filtered = apply_filter(&state.audit_log, &state.audit_filter);

    let title = format!(" AUDIT LOG ({} events) ", filtered.len());

    let block = Block::default()
        .title(Span::styled(
            title,
            Style::default().fg(theme.rose).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.border))
        .style(Style::default().bg(theme.bg_raised));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if filtered.is_empty() {
        let msg = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(
                "  No audit events found.",
                Style::default().fg(theme.text_dim),
            )),
        ]);
        frame.render_widget(msg, inner);
        render_log_footer(frame, area, theme);
        return;
    }

    // Header row.
    let mut lines: Vec<Line> = vec![Line::from(Span::styled(
        format!(
            "  {:<12} {:<8} {:<20} {:<16} {}",
            "Time", "Sev", "Kind", "Agent", "Message"
        ),
        Style::default()
            .fg(theme.text_dim)
            .add_modifier(Modifier::UNDERLINED),
    ))];

    let selected_idx = state.audit_scroll.selected.unwrap_or(usize::MAX);
    let visible = inner.height.saturating_sub(1) as usize;

    for (vis_idx, event) in filtered
        .iter()
        .skip(state.audit_scroll.offset)
        .take(visible)
        .enumerate()
    {
        let abs_idx = vis_idx + state.audit_scroll.offset;
        let is_selected = abs_idx == selected_idx;

        let sev_color = severity_color(&event.severity, theme);
        let row_bg = if is_selected {
            Style::default().bg(theme.bg_highlight)
        } else {
            Style::default()
        };

        let time_str = event.timestamp.format("%H:%M:%S").to_string();
        let sev_glyph = severity_glyph(&event.severity);

        // Truncate agent name to 14 chars.
        let agent = if event.agent_name.len() > 14 {
            format!("{}…", &event.agent_name[..13])
        } else {
            event.agent_name.clone()
        };

        // Truncate kind to 18 chars.
        let kind = if event.kind.len() > 18 {
            format!("{}…", &event.kind[..17])
        } else {
            event.kind.clone()
        };

        // Message: remaining width.
        let msg_max = (inner.width as usize).saturating_sub(62);
        let msg = if event.message.len() > msg_max && msg_max > 1 {
            format!("{}…", &event.message[..msg_max.saturating_sub(1)])
        } else {
            event.message.clone()
        };

        lines.push(Line::from(vec![
            Span::styled(
                format!("  {time_str:<10}"),
                Style::default().fg(theme.text_dim).patch(row_bg),
            ),
            Span::styled(
                format!("{sev_glyph} {:<6}", event.severity),
                Style::default()
                    .fg(sev_color)
                    .add_modifier(Modifier::BOLD)
                    .patch(row_bg),
            ),
            Span::styled(
                format!("{kind:<20}"),
                Style::default().fg(theme.text_primary).patch(row_bg),
            ),
            Span::styled(
                format!("{agent:<16}"),
                Style::default().fg(theme.text_dim).patch(row_bg),
            ),
            Span::styled(msg, Style::default().fg(sev_color).patch(row_bg)),
        ]));
    }

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);

    render_log_footer(frame, area, theme);
}

fn render_log_footer(frame: &mut Frame, area: Rect, theme: &Theme) {
    let footer_y = area.y + area.height.saturating_sub(1);
    if footer_y < area.y + area.height {
        let footer_area = Rect {
            y: footer_y,
            height: 1,
            x: area.x + 1,
            width: area.width.saturating_sub(2),
        };
        frame.render_widget(
            Paragraph::new(Span::styled(
                " g: top  G: latest  f: filter  j/k: navigate  Esc: back",
                Style::default().fg(theme.text_dim),
            )),
            footer_area,
        );
    }
}

// ---------------------------------------------------------------------------
// Filter helpers
// ---------------------------------------------------------------------------

/// Active audit log filter mode.
#[allow(dead_code)]
#[derive(Debug, Clone, Default, PartialEq)]
pub enum AuditFilter {
    /// No filter — show all events.
    #[default]
    None,
    /// Filter by severity string (info, warn, error).
    BySeverity(String),
    /// Filter by agent name (substring match).
    ByAgent(String),
    /// Filter by event kind (substring match).
    ByKind(String),
}

/// Apply the current filter to the audit log, returning a borrowed slice view.
fn apply_filter<'a>(events: &'a [AuditEvent], filter: &AuditFilter) -> Vec<&'a AuditEvent> {
    events
        .iter()
        .filter(|e| match filter {
            AuditFilter::None => true,
            AuditFilter::BySeverity(sev) => e.severity.eq_ignore_ascii_case(sev),
            AuditFilter::ByAgent(agent) => {
                e.agent_name.to_lowercase().contains(&agent.to_lowercase())
            }
            AuditFilter::ByKind(kind) => e.kind.to_lowercase().contains(&kind.to_lowercase()),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Severity helpers
// ---------------------------------------------------------------------------

/// ROSEDUST color for a severity level.
fn severity_color(severity: &str, theme: &Theme) -> ratatui::style::Color {
    match severity.to_lowercase().as_str() {
        "info" => theme.bone,
        "warn" | "warning" => theme.warning,
        "error" | "err" => theme.danger,
        "debug" => theme.text_dim,
        _ => theme.text_primary,
    }
}

/// Glyph for a severity level.
fn severity_glyph(severity: &str) -> &'static str {
    match severity.to_lowercase().as_str() {
        "info" => "·",
        "warn" | "warning" => "▲",
        "error" | "err" => "✗",
        "debug" => "○",
        _ => "?",
    }
}
