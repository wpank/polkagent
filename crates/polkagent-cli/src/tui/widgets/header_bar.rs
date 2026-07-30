//! Top header bar widget.
//!
//! Renders a single-row bar showing:
//! - Left:   "POLKAGENT" product name with rose accent
//! - Centre: active tab name with F-key indicator
//! - Right:  binary version string

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph},
    Frame,
};

use crate::tui::app::Tab;
use crate::tui::theme::Theme;

/// Render the header bar into `area`.
///
/// The bar is always one row tall; callers are responsible for constraining
/// `area` to a height of 1.
pub fn render(frame: &mut Frame, area: Rect, active_tab: Tab, theme: &Theme) {
    // Background fill.
    let bg_block = Block::default()
        .style(Style::default().bg(theme.bg_raised));
    frame.render_widget(bg_block, area);

    // Divide area into [left | centre | right].
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(18),
            Constraint::Min(10),
            Constraint::Length(20),
        ])
        .split(area);

    // ── Left: brand ──────────────────────────────────────────────────────
    let brand = Paragraph::new(Line::from(vec![
        Span::styled(
            " POLKAGENT",
            Style::default()
                .fg(theme.rose_bright)
                .add_modifier(Modifier::BOLD),
        ),
    ]));
    frame.render_widget(brand, cols[0]);

    // ── Centre: active tab ───────────────────────────────────────────────
    let tab_label = active_tab.label();
    let tab_fkey  = active_tab.fkey_label();
    let centre = Paragraph::new(Line::from(vec![
        Span::styled(tab_fkey, Style::default().fg(theme.rose_dim)),
        Span::raw(" "),
        Span::styled(
            tab_label,
            Style::default()
                .fg(theme.bone)
                .add_modifier(Modifier::BOLD),
        ),
    ]))
    .alignment(Alignment::Center);
    frame.render_widget(centre, cols[1]);

    // ── Right: version ───────────────────────────────────────────────────
    let version_str = format!("v{} ", env!("CARGO_PKG_VERSION"));
    let right = Paragraph::new(Span::styled(
        version_str,
        Style::default().fg(theme.text_dim),
    ))
    .alignment(Alignment::Right);
    frame.render_widget(right, cols[2]);
}
