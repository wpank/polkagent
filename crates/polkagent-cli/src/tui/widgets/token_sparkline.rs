//! Token usage sparkline widget.
//!
//! Renders a single-line Braille-character sparkline showing token usage
//! over a series of data points. Each Braille character encodes two
//! vertical data points (the Braille block is 2 wide x 4 tall, but we
//! use the single-column 1x8 mapping for maximum horizontal density).
//!
//! Colour is `bone` for normal usage and `amber` when values approach
//! the configured limit.

use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::tui::theme::Theme;

// ---------------------------------------------------------------------------
// Braille mapping
// ---------------------------------------------------------------------------

/// Braille base codepoint (U+2800).
const BRAILLE_BASE: u32 = 0x2800;

/// Map a value in `0..=7` to a Braille dot pattern in a single column.
///
/// We use dots 1-2-3-7 (bits 0,1,2,6) for the left column only, giving
/// us 8 vertical levels (0 = blank, 7 = full).
///
/// Level mapping:
///   0 → ⠀ (blank)
///   1 → ⡀ (dot-7)
///   2 → ⡠ (dot-6,7)
///   3 → ⡰ (dot-3,6,7)  ... etc.
///
/// For simplicity we use a lookup table of pre-computed offsets.
const BRAILLE_LEVELS: [u32; 8] = [
    0x00, // 0: empty
    0x40, // 1: dot 7
    0x44, // 2: dot 7,3
    0x46, // 3: dot 7,3,2
    0x47, // 4: dot 7,3,2,1
    0x4F, // 5: dot 7,3,2,1,4  (right col dot 1)
    0x5F, // 6: dot 7,3,2,1,4,5
    0x7F, // 7: dot 7,3,2,1,4,5,6
];

/// Convert a value and non-zero limit to a single Braille character.
fn value_to_braille(value: u64, limit: u64) -> char {
    let scaled = u128::from(value.min(limit))
        .saturating_mul(7)
        .saturating_add(u128::from(limit) / 2);
    let level = usize::try_from(scaled / u128::from(limit)).unwrap_or(7);
    let cp = BRAILLE_BASE + BRAILLE_LEVELS[level];
    char::from_u32(cp).unwrap_or(' ')
}

// ---------------------------------------------------------------------------
// Render
// ---------------------------------------------------------------------------

/// Render a token-usage sparkline into `area` (should be 1 row tall).
///
/// `values` contains raw token counts for successive time buckets.
/// `limit` is the maximum expected value (used to normalise heights).
/// `warn_threshold` is a ratio (`0.0..=1.0`) above which the sparkline
/// switches from `bone` to `amber`.
#[allow(
    clippy::cast_precision_loss,
    reason = "the float ratio is used only to compare a display threshold; braille height is computed exactly"
)]
pub fn render(
    frame: &mut Frame,
    area: Rect,
    values: &[u64],
    limit: u64,
    warn_threshold: f64,
    theme: &Theme,
) {
    if area.width == 0 || area.height == 0 || limit == 0 {
        return;
    }

    let width = usize::from(area.width);

    // Take the last `width` data points (or pad with zeros on the left).
    let start = values.len().saturating_sub(width);
    let visible = &values[start..];

    let mut spans: Vec<Span<'_>> = Vec::with_capacity(width);

    // Leading padding if we have fewer points than the width.
    let pad_count = width.saturating_sub(visible.len());
    if pad_count > 0 {
        spans.push(Span::styled(
            " ".repeat(pad_count),
            Style::default().fg(theme.bone),
        ));
    }

    for &v in visible {
        let ratio = v as f64 / limit as f64;
        let ch = value_to_braille(v, limit);
        let color = if ratio >= warn_threshold {
            theme.warning // amber
        } else {
            theme.bone
        };
        spans.push(Span::styled(ch.to_string(), Style::default().fg(color)));
    }

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}
