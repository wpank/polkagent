//! Event Timeline view (F5).
//!
//! Chronological list of events for the selected run. Each event shows its
//! timestamp, event type, and a brief description. Events are color-coded:
//!
//! - Lifecycle events (RunCreated, TurnStarted, ...) — rose
//! - Effect events (EffectIntentCreated, ...) — jade (success)
//! - Error events (RunFailed, EffectFailed, ...) — crimson (danger)
//!
//! Scrollable with j/k or arrow keys. Selecting an event shows its raw
//! JSON payload in a side panel when width >= 100.

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph, Wrap},
    Frame,
};

use crate::tui::state::{EventSummary, ScrollState, TuiState};
use crate::tui::theme::Theme;

// ---------------------------------------------------------------------------
// Public render entry point
// ---------------------------------------------------------------------------

/// Render the event timeline into `area`.
pub fn render(frame: &mut Frame, area: Rect, state: &TuiState, theme: &Theme) {
    if state.run_events.is_empty() && state.selected_run.is_none() {
        render_no_selection(frame, area, theme);
        return;
    }

    if area.width >= 100 && state.timeline_scroll.selected.is_some() {
        // Wide: event list on the left, payload detail on the right.
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
            .split(area);

        render_event_list(
            frame,
            cols[0],
            &state.run_events,
            &state.timeline_scroll,
            theme,
        );
        if let Some(sel) = state.timeline_scroll.selected {
            if let Some(event) = state.run_events.get(sel) {
                render_event_detail(frame, cols[1], event, theme);
            }
        }
    } else {
        render_event_list(
            frame,
            area,
            &state.run_events,
            &state.timeline_scroll,
            theme,
        );
    }
}

// ---------------------------------------------------------------------------
// No selection placeholder
// ---------------------------------------------------------------------------

fn render_no_selection(frame: &mut Frame, area: Rect, theme: &Theme) {
    let block = styled_block(" EVENT TIMELINE ", theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let msg = Paragraph::new(vec![
        Line::from(""),
        Line::from(Span::styled(
            "  No run selected.",
            Style::default().fg(theme.text_dim),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  Select a run from the Runs tab (F3) first, then switch here.",
            Style::default().fg(theme.text_dim),
        )),
    ]);
    frame.render_widget(msg, inner);
}

// ---------------------------------------------------------------------------
// Event list
// ---------------------------------------------------------------------------

fn render_event_list(
    frame: &mut Frame,
    area: Rect,
    events: &[EventSummary],
    scroll: &ScrollState,
    theme: &Theme,
) {
    let run_hint = if events.is_empty() { "" } else { "" };
    let title = format!(
        " EVENTS ({count}){hint} ",
        count = events.len(),
        hint = run_hint,
    );
    let block = styled_block(&title, theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if events.is_empty() {
        let msg = Paragraph::new(Span::styled(
            "  No events found for this run.",
            Style::default().fg(theme.text_dim),
        ));
        frame.render_widget(msg, inner);
        return;
    }

    // Header row.
    let mut lines: Vec<Line> = vec![Line::from(Span::styled(
        format!("  {:<12} {:<28} {}", "Time", "Type", "Description"),
        Style::default()
            .fg(theme.text_dim)
            .add_modifier(Modifier::UNDERLINED),
    ))];

    let selected_idx = scroll.selected.unwrap_or(usize::MAX);
    let visible = inner.height.saturating_sub(1) as usize;

    for (vis_idx, event) in events.iter().skip(scroll.offset).take(visible).enumerate() {
        let abs_idx = vis_idx + scroll.offset;
        let is_selected = abs_idx == selected_idx;

        let type_color = event_type_color(&event.event_type, theme);
        let time_str = event.timestamp.format("%H:%M:%S").to_string();

        // Truncate type and description to fit.
        let type_display = truncate(&event.event_type, 26);
        let desc_max = (inner.width as usize).saturating_sub(44);
        let desc_display = truncate(&event.description, desc_max);

        let row_bg = if is_selected {
            Style::default().bg(theme.bg_highlight)
        } else {
            Style::default()
        };

        let glyph = event_glyph(&event.event_type);
        let glyph_color = type_color;

        lines.push(Line::from(vec![
            Span::styled(
                format!("  {glyph} "),
                Style::default().fg(glyph_color).patch(row_bg),
            ),
            Span::styled(
                format!("{time_str:<10}"),
                Style::default().fg(theme.text_dim).patch(row_bg),
            ),
            Span::styled(
                format!("{type_display:<28}"),
                Style::default().fg(type_color).patch(row_bg),
            ),
            Span::styled(
                desc_display,
                Style::default().fg(theme.text_primary).patch(row_bg),
            ),
        ]));
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

// ---------------------------------------------------------------------------
// Event detail panel (side panel)
// ---------------------------------------------------------------------------

fn render_event_detail(frame: &mut Frame, area: Rect, event: &EventSummary, theme: &Theme) {
    let type_color = event_type_color(&event.event_type, theme);

    let block = Block::default()
        .title(Span::styled(
            format!(" {} ", event.event_type),
            Style::default().fg(theme.bone).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.border_active))
        .style(Style::default().bg(theme.bg_raised));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let ts = event
        .timestamp
        .format("%Y-%m-%d %H:%M:%S.%3f UTC")
        .to_string();

    let mut lines: Vec<Line> = vec![
        Line::from(vec![
            Span::styled(
                event_glyph(&event.event_type),
                Style::default().fg(type_color),
            ),
            Span::raw("  "),
            Span::styled(
                event.event_type.clone(),
                Style::default().fg(type_color).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
        kv_line("  Timestamp", &ts, theme.text_primary, theme),
        kv_line("  Event ID", &event.id, theme.text_dim, theme),
        Line::from(""),
        Line::from(Span::styled(
            "  Payload:",
            Style::default().fg(theme.bone).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
    ];

    // Pretty-print payload (basic: split on commas for readability).
    let payload_lines = format_payload(&event.payload, inner.width.saturating_sub(4) as usize);
    for pl in payload_lines {
        lines.push(Line::from(Span::styled(
            format!("  {pl}"),
            Style::default().fg(theme.text_primary),
        )));
    }

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

// ---------------------------------------------------------------------------
// Helpers
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

/// Map an event type to its ROSEDUST colour category.
fn event_type_color(event_type: &str, theme: &Theme) -> ratatui::style::Color {
    let lower = event_type.to_lowercase();
    if lower.contains("fail") || lower.contains("error") || lower.contains("timeout") {
        theme.danger
    } else if lower.contains("effect") || lower.contains("outcome") {
        theme.success
    } else {
        // Lifecycle events: rose family.
        theme.rose
    }
}

/// Glyph for an event type.
fn event_glyph(event_type: &str) -> &'static str {
    let lower = event_type.to_lowercase();
    if lower.contains("fail") || lower.contains("error") {
        "✗"
    } else if lower.contains("complete") || lower.contains("success") {
        "✓"
    } else if lower.contains("start") || lower.contains("created") {
        "▶"
    } else if lower.contains("effect") {
        "◆"
    } else {
        "·"
    }
}

fn kv_line(
    key: &str,
    value: &str,
    value_color: ratatui::style::Color,
    theme: &Theme,
) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{key:<14}"), Style::default().fg(theme.text_dim)),
        Span::styled(value.to_owned(), Style::default().fg(value_color)),
    ])
}

/// Truncate a string to `max` characters, appending ellipsis if needed.
fn truncate(s: &str, max: usize) -> String {
    if s.len() > max && max > 1 {
        format!("{}…", &s[..max.saturating_sub(1)])
    } else {
        s.to_owned()
    }
}

/// Split a JSON payload into lines for display.
fn format_payload(json: &str, max_width: usize) -> Vec<String> {
    // Simple approach: wrap raw JSON at max_width boundaries.
    let trimmed = json.trim();
    if trimmed.is_empty() || trimmed == "{}" {
        return vec!["(empty)".into()];
    }

    let mut lines = Vec::new();
    let mut remaining = trimmed;
    while !remaining.is_empty() {
        if remaining.len() <= max_width {
            lines.push(remaining.to_owned());
            break;
        }
        let split_at = max_width.min(remaining.len());
        lines.push(remaining[..split_at].to_owned());
        remaining = &remaining[split_at..];
    }
    lines
}
