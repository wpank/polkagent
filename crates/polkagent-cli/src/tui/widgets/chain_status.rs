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
    /// Node implementation version string (e.g. "Parity Polkadot/v1.7.0").
    /// Shown when available; omitted when `None`.
    pub node_version: Option<&'a str>,
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
        ("\u{25CB}", theme.danger) // ○ crimson
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
            Style::default().fg(theme.bone).add_modifier(Modifier::BOLD),
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
            Span::styled(format!("{conn_glyph} "), Style::default().fg(conn_color)),
            Span::styled(
                conn_label,
                Style::default().fg(conn_color).add_modifier(Modifier::BOLD),
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

    // ── Right column: metadata + node version ─────────────────────────────
    let mut right_lines = vec![
        Line::from(vec![
            Span::styled("Metadata v", Style::default().fg(theme.text_dim)),
            Span::styled(
                data.metadata_version.to_string(),
                Style::default().fg(theme.bone),
            ),
        ]),
        Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled(data.metadata_freshness, Style::default().fg(theme.text_dim)),
        ]),
    ];

    // Show node version when available.
    if let Some(version) = data.node_version {
        right_lines.push(Line::from(vec![
            Span::styled("Node: ", Style::default().fg(theme.text_dim)),
            Span::styled(version.to_owned(), Style::default().fg(theme.bone)),
        ]));
    }

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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{backend::TestBackend, Terminal};

    fn test_theme() -> Theme {
        Theme::dark()
    }

    #[test]
    fn test_format_block_number_zero() {
        assert_eq!(format_block_number(0), "0");
    }

    #[test]
    fn test_format_block_number_thousands() {
        assert_eq!(format_block_number(1_000), "1,000");
        assert_eq!(format_block_number(22_500_000), "22,500,000");
    }

    #[test]
    fn test_widget_renders_without_panic_on_empty_state() {
        let backend = TestBackend::new(80, 10);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        let theme = test_theme();

        let data = ChainStatusData {
            chain_name: "",
            connected: false,
            best_block: 0,
            finalized_block: 0,
            node_version: None,
            metadata_version: 14,
            metadata_freshness: "stale",
        };

        terminal
            .draw(|frame| {
                let area = frame.area();
                render(frame, area, &data, &theme);
            })
            .expect("render should not panic on empty state");
    }

    #[test]
    fn test_widget_renders_with_real_data() {
        let backend = TestBackend::new(80, 10);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        let theme = test_theme();

        let data = ChainStatusData {
            chain_name: "Polkadot",
            connected: true,
            best_block: 22_500_000,
            finalized_block: 22_499_990,
            node_version: Some("Parity Polkadot/v1.7.0"),
            metadata_version: 14,
            metadata_freshness: "fresh",
        };

        terminal
            .draw(|frame| {
                let area = frame.area();
                render(frame, area, &data, &theme);
            })
            .expect("render should not panic with real data");
    }

    #[test]
    fn test_widget_skips_render_on_tiny_area() {
        let backend = TestBackend::new(5, 2);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        let theme = test_theme();

        let data = ChainStatusData {
            chain_name: "Polkadot",
            connected: true,
            best_block: 100,
            finalized_block: 99,
            node_version: None,
            metadata_version: 14,
            metadata_freshness: "fresh",
        };

        // Should not panic even with area too small.
        terminal
            .draw(|frame| {
                let area = frame.area();
                render(frame, area, &data, &theme);
            })
            .expect("render should gracefully skip on tiny area");
    }

    #[test]
    fn test_widget_renders_not_connected_label() {
        let backend = TestBackend::new(80, 10);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        let theme = test_theme();

        let data = ChainStatusData {
            chain_name: "Not connected",
            connected: false,
            best_block: 0,
            finalized_block: 0,
            node_version: None,
            metadata_version: 14,
            metadata_freshness: "stale",
        };

        terminal
            .draw(|frame| {
                let area = frame.area();
                render(frame, area, &data, &theme);
            })
            .expect("render should display 'Not connected' gracefully");
    }
}
