//! Horizontal tab bar widget.
//!
//! Renders a single-row bar of F-key-labelled tabs. The active tab is
//! highlighted in rose; inactive tabs use `text_dim`. The full set of
//! seven conceptual tabs is rendered even though only four are wired up
//! to views today — the remainder appear dimmed as placeholders.
// Public widget API — will be wired to the header bar.
#![allow(dead_code)]

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph},
    Frame,
};

use crate::tui::theme::Theme;

/// A single tab descriptor.
struct TabDef {
    /// F-key shortcut label (e.g. "F1").
    fkey: &'static str,
    /// Human-readable tab name.
    label: &'static str,
}

/// The canonical ordered set of tabs.
const TABS: [TabDef; 7] = [
    TabDef { fkey: "F1", label: "Dashboard" },
    TabDef { fkey: "F2", label: "Agents" },
    TabDef { fkey: "F3", label: "Runs" },
    TabDef { fkey: "F4", label: "Timeline" },
    TabDef { fkey: "F5", label: "Approvals" },
    TabDef { fkey: "F6", label: "Knowledge" },
    TabDef { fkey: "F7", label: "System" },
];

/// Render a horizontal tab bar into `area`.
///
/// `active_index` is the 0-based index of the currently selected tab in
/// [`TABS`]. Values outside `0..7` result in no tab being highlighted.
///
/// The bar is always one row tall; callers must constrain `area` to
/// `Constraint::Length(1)`.
pub fn render(frame: &mut Frame, area: Rect, active_index: usize, theme: &Theme) {
    // Background fill.
    let bg = Block::default().style(Style::default().bg(theme.bg_raised));
    frame.render_widget(bg, area);

    let mut spans: Vec<Span<'_>> = Vec::with_capacity(TABS.len() * 3);

    for (i, tab) in TABS.iter().enumerate() {
        let is_active = i == active_index;

        let fkey_style = if is_active {
            Style::default().fg(theme.rose_bright)
        } else {
            Style::default().fg(theme.text_dim)
        };

        let label_style = if is_active {
            Style::default()
                .fg(theme.rose_bright)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.text_dim)
        };

        spans.push(Span::styled(format!(" {}:", tab.fkey), fkey_style));
        spans.push(Span::styled(tab.label, label_style));
        spans.push(Span::raw(" "));
    }

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}
