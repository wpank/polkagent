//! Animated wave-style progress bar widget.
//!
//! Uses Unicode block characters (`▏▎▍▌▋▊▉█`) to render a smooth
//! horizontal progress bar. The fill colour transitions from jade at
//! the start, through amber at the midpoint, to rose when near
//! completion.

use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::tui::theme::Theme;

// ---------------------------------------------------------------------------
// Block characters (eighth-width increments)
// ---------------------------------------------------------------------------

/// Unicode block elements from 1/8 to full block.
const BLOCKS: [char; 8] = [
    '\u{258F}', // ▏  1/8
    '\u{258E}', // ▎  2/8
    '\u{258D}', // ▍  3/8
    '\u{258C}', // ▌  4/8
    '\u{258B}', // ▋  5/8
    '\u{258A}', // ▊  6/8
    '\u{2589}', // ▉  7/8
    '\u{2588}', // █  8/8
];

// ---------------------------------------------------------------------------
// Render
// ---------------------------------------------------------------------------

/// Render a progress bar into `area` (should be 1 row tall).
///
/// `ratio` is clamped to `0.0..=1.0`. The fill colour transitions:
///
/// - `ratio < 0.4`  → jade (success)
/// - `0.4 <= ratio < 0.8` → amber (warning)
/// - `ratio >= 0.8` → rose (rose_bright)
pub fn render(frame: &mut Frame, area: Rect, ratio: f64, theme: &Theme) {
    let width = area.width as usize;
    if width == 0 || area.height == 0 {
        return;
    }

    let ratio = ratio.clamp(0.0, 1.0);

    // Choose colour based on progress.
    let fill_color = if ratio >= 0.8 {
        theme.rose_bright
    } else if ratio >= 0.4 {
        theme.warning
    } else {
        theme.success
    };

    let fill_style = Style::default().fg(fill_color);
    let empty_style = Style::default().fg(theme.text_phantom);

    // Total sub-cell units (8 per cell).
    let total_units = width * 8;
    let filled_units = (ratio * total_units as f64).round() as usize;

    let full_cells = filled_units / 8;
    let partial_idx = filled_units % 8;

    let mut spans: Vec<Span<'_>> = Vec::with_capacity(width + 1);

    // Full filled cells.
    if full_cells > 0 {
        spans.push(Span::styled(
            BLOCKS[7].to_string().repeat(full_cells),
            fill_style,
        ));
    }

    // Partial cell.
    if partial_idx > 0 && full_cells < width {
        spans.push(Span::styled(
            BLOCKS[partial_idx - 1].to_string(),
            fill_style,
        ));
    }

    // Empty remainder.
    let filled_chars = full_cells + if partial_idx > 0 { 1 } else { 0 };
    let empty_chars = width.saturating_sub(filled_chars);
    if empty_chars > 0 {
        spans.push(Span::styled("░".repeat(empty_chars), empty_style));
    }

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}
