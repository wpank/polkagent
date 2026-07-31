//! Chain connection status widget.
//!
//! Renders a compact panel showing the state of a Substrate chain
//! connection:
//! - Chain name and network identifier
//! - Best block number and finalized block number
//! - Connection indicator: `◉` connected (jade) / `○` disconnected (crimson)
//! - Runtime metadata version and freshness

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::tui::theme::Theme;

// ---------------------------------------------------------------------------
// Data
// ---------------------------------------------------------------------------

/// Input data for the chain status widget.
pub struct ChainStatusData<'a> {
    /// Human-readable chain name (e.g. "Polkadot", "Westend").
    pub chain_name: &'a str,
    /// Whether the RPC connection is alive.
    pub connected: bool,
    /// Best (head) block number.
    pub best_block: u64,
    /// Last finalized block number.
    pub finalized_block: u64,
    /// Runtime metadata version (e.g. 14, 15).
    pub metadata_version: u32,
    /// Human-readable age of the cached metadata (e.g. "2m ago", "fresh").
    pub metadata_freshness: &'a str,
}

// ---------------------------------------------------------------------------
// Render
// ---------------------------------------------------------------------------

/// Render the chain status panel into `area`.
///
/// Minimum useful size is roughly 30 columns x 5 rows.
pub fn render(frame: &mut Frame, area: Rect, data: &ChainStatusData<'_>, theme: &Theme) {
    if area.width < 8 || area.height < 3 {
        return;
    }

    // Connection indicator.
    let (conn_glyph, conn_color) = if data.connected {
        ("\u{25C9}", theme.success) // ◉ jade
    } else {
        ("\u{25CB}", theme.danger)  // ○ crimson
    };

    let conn_label = if data.connected {
        "connected"
    } else {
        "disconnected"
    };

    // Outer block with chain name as title.
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border))
        .title(Span::styled(
            format!(" {} ", data.chain_name),
            Style::default()
                .fg(theme.bone)
                .add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(theme.bg_raised));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Two-column layout: left = blocks, right = metadata.
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(inner);

    // ── Left column: connection + blocks ─────────────────────────────────
    let left_lines = vec![
        Line::from(vec![
            Span::styled(
                format!("{conn_glyph} "),
                Style::default().fg(conn_color),
            ),
            Span::styled(
                conn_label,
                Style::default()
                    .fg(conn_color)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::styled("  Best:  ", Style::default().fg(theme.text_dim)),
            Span::styled(
                format!("#{}", format_block_number(data.best_block)),
                Style::default().fg(theme.bone),
            ),
        ]),
        Line::from(vec![
            Span::styled("  Final: ", Style::default().fg(theme.text_dim)),
            Span::styled(
                format!("#{}", format_block_number(data.finalized_block)),
                Style::default().fg(theme.finalized_teal),
            ),
        ]),
    ];
    frame.render_widget(Paragraph::new(left_lines), cols[0]);

    // ── Right column: metadata ───────────────────────────────────────────
    let right_lines = vec![
        Line::from(vec![
            Span::styled("Metadata v", Style::default().fg(theme.text_dim)),
            Span::styled(
                data.metadata_version.to_string(),
                Style::default().fg(theme.bone),
            ),
        ]),
        Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled(
                data.metadata_freshness,
                Style::default().fg(theme.text_dim),
            ),
        ]),
    ];
    frame.render_widget(Paragraph::new(right_lines), cols[1]);
}

/// Format a block number with thousands separators for readability.
fn format_block_number(n: u64) -> String {
    let s = n.to_string();
    let mut result = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(ch);
    }
    result.chars().rev().collect()
}
