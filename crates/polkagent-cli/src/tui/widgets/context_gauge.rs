//! Context window gauge widget.
//!
//! Renders a horizontal fill bar showing how much of the context window
//! has been consumed, with a "used / total" label centred inside the bar.
//!
//! Colour thresholds:
//! - `< 50%`  → jade  (success)
//! - `50–80%` → amber (warning)
//! - `> 80%`  → crimson (danger)

use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::tui::theme::Theme;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Format a token count as a compact human-readable string.
///
/// - Values < 1_000 → as-is (e.g. "842")
/// - Values < 1_000_000 → e.g. "128.0k"
/// - Values >= 1_000_000 → e.g. "1.2M"
fn format_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

// ---------------------------------------------------------------------------
// Render
// ---------------------------------------------------------------------------

/// Render a context-window gauge into `area` (should be 1 row tall).
///
/// `used` and `total` are token counts. If `total` is 0 the gauge is
/// rendered as empty.
pub fn render(frame: &mut Frame, area: Rect, used: u64, total: u64, theme: &Theme) {
    let width = area.width as usize;
    if width == 0 || area.height == 0 {
        return;
    }

    let ratio = if total > 0 {
        (used as f64 / total as f64).clamp(0.0, 1.0)
    } else {
        0.0
    };

    // Choose colour by threshold.
    let fill_color = if ratio > 0.80 {
        theme.danger   // crimson
    } else if ratio >= 0.50 {
        theme.warning  // amber
    } else {
        theme.success  // jade
    };

    // Build the label.
    let label = format!("{} / {}", format_tokens(used), format_tokens(total));
    let label_len = label.len();

    // Determine how many cells are filled vs empty.
    let filled_count = ((ratio * width as f64).round() as usize).min(width);
    let empty_count = width.saturating_sub(filled_count);

    // Build the bar as a string, overlaying the label in the centre.
    let bar_filled = "\u{2588}".repeat(filled_count);
    let bar_empty = "░".repeat(empty_count);
    let bar_str = format!("{bar_filled}{bar_empty}");

    // We render the bar in two colour spans, with the label centred.
    // If the label fits, overlay it; otherwise just render the bar.
    if label_len + 2 <= width {
        let label_start = (width - label_len) / 2;
        let label_end = label_start + label_len;

        let mut spans: Vec<Span<'_>> = Vec::new();

        // Portion before the label.
        let pre = &bar_str[..label_start.min(bar_str.len())];
        if !pre.is_empty() {
            let pre_fill_end = filled_count.min(label_start);
            let pre_fill = &bar_str[..pre_fill_end];
            let pre_empty = &bar_str[pre_fill_end..label_start.min(bar_str.len())];
            if !pre_fill.is_empty() {
                spans.push(Span::styled(
                    pre_fill.to_string(),
                    Style::default().fg(fill_color),
                ));
            }
            if !pre_empty.is_empty() {
                spans.push(Span::styled(
                    pre_empty.to_string(),
                    Style::default().fg(theme.text_phantom),
                ));
            }
        }

        // The label itself — render with bone on bg_raised for readability.
        spans.push(Span::styled(
            label.clone(),
            Style::default().fg(theme.bone).bg(theme.bg_raised),
        ));

        // Portion after the label.
        if label_end < width {
            let post_fill_start = filled_count.max(label_end);
            let post_fill_end = filled_count.max(label_end);
            // Between label_end and filled_count: still filled.
            if filled_count > label_end {
                let seg = "\u{2588}".repeat(filled_count - label_end);
                spans.push(Span::styled(seg, Style::default().fg(fill_color)));
            }
            // Empty remainder.
            let remaining_empty = width.saturating_sub(post_fill_start.max(label_end));
            if remaining_empty > 0 {
                spans.push(Span::styled(
                    "░".repeat(remaining_empty),
                    Style::default().fg(theme.text_phantom),
                ));
            }
            let _ = post_fill_end; // suppress unused
        }

        frame.render_widget(Paragraph::new(Line::from(spans)), area);
    } else {
        // Label doesn't fit — just render the bar without it.
        let mut spans: Vec<Span<'_>> = Vec::new();
        if filled_count > 0 {
            spans.push(Span::styled(
                "\u{2588}".repeat(filled_count),
                Style::default().fg(fill_color),
            ));
        }
        if empty_count > 0 {
            spans.push(Span::styled(
                "░".repeat(empty_count),
                Style::default().fg(theme.text_phantom),
            ));
        }
        frame.render_widget(Paragraph::new(Line::from(spans)), area);
    }
}
