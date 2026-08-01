//! Structured error digest widget.
//!
//! Renders a multi-line error display with:
//! - Error type in a crimson header
//! - Error message in bone
//! - Optional recovery suggestion in jade
//! - Optional stack trace (collapsed by default, toggled via `show_trace`)

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};

use crate::tui::theme::Theme;

// ---------------------------------------------------------------------------
// Data
// ---------------------------------------------------------------------------

/// Input data for the error digest widget.
// Public widget API — constructed when the error view is wired to live error state.
#[allow(dead_code)]
pub struct ErrorData<'a> {
    /// Short error type or category (e.g. "RuntimeError", "RpcError").
    pub error_type: &'a str,
    /// Human-readable error message.
    pub message: &'a str,
    /// Optional recovery suggestion shown in jade.
    pub recovery: Option<&'a str>,
    /// Optional stack trace lines (shown only when `show_trace` is true).
    pub trace: Option<&'a [&'a str]>,
    /// Whether to expand the stack trace section.
    pub show_trace: bool,
}

// ---------------------------------------------------------------------------
// Render
// ---------------------------------------------------------------------------

/// Render a structured error digest into `area`.
///
/// The widget draws its own border and fills the provided area.
// Public widget API — called by the error view once wired.
#[allow(dead_code)]
pub fn render(frame: &mut Frame, area: Rect, data: &ErrorData<'_>, theme: &Theme) {
    if area.width < 4 || area.height < 3 {
        return;
    }

    // Outer block.
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.danger))
        .title(Span::styled(
            format!(" {} ", data.error_type),
            Style::default()
                .fg(theme.danger)
                .add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(theme.bg_raised));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines: Vec<Line<'_>> = Vec::new();

    // ── Error message ────────────────────────────────────────────────────
    lines.push(Line::from(Span::styled(
        data.message,
        Style::default().fg(theme.bone),
    )));

    // ── Recovery suggestion ──────────────────────────────────────────────
    if let Some(recovery) = data.recovery {
        lines.push(Line::raw(""));
        lines.push(Line::from(vec![
            Span::styled(
                "Suggestion: ",
                Style::default()
                    .fg(theme.success)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(recovery, Style::default().fg(theme.success)),
        ]));
    }

    // ── Stack trace ──────────────────────────────────────────────────────
    if let Some(trace) = data.trace {
        lines.push(Line::raw(""));
        if data.show_trace {
            lines.push(Line::from(Span::styled(
                "Stack trace (press 't' to collapse):",
                Style::default()
                    .fg(theme.text_dim)
                    .add_modifier(Modifier::ITALIC),
            )));
            for line in trace.iter() {
                lines.push(Line::from(Span::styled(
                    format!("  {line}"),
                    Style::default().fg(theme.text_dim),
                )));
            }
        } else {
            lines.push(Line::from(Span::styled(
                "Stack trace collapsed (press 't' to expand)",
                Style::default()
                    .fg(theme.text_ghost)
                    .add_modifier(Modifier::ITALIC),
            )));
        }
    }

    let para = Paragraph::new(lines).wrap(Wrap { trim: false });
    frame.render_widget(para, inner);
}
