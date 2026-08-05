//! Bottom status bar widget.
//!
//! Renders a single-row bar showing:
//! - Left:   key-binding hints relevant to the current tab
//! - Centre: error message (if any) or last-refresh time
//! - Right:  current UTC time and input mode indicator

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Block, Paragraph},
    Frame,
};

use crate::tui::app::Tab;
use crate::tui::input::InputMode;
use crate::tui::theme::Theme;

/// Render the status bar into `area` (should be 1 row tall).
pub fn render(
    frame: &mut Frame,
    area: Rect,
    active_tab: Tab,
    mode: InputMode,
    last_error: Option<&str>,
    theme: &Theme,
) {
    let bg_block = Block::default().style(Style::default().bg(theme.bg_raised));
    frame.render_widget(bg_block, area);

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(50),
            Constraint::Percentage(30),
            Constraint::Percentage(20),
        ])
        .split(area);

    // ── Left: key hints ───────────────────────────────────────────────────
    let hints = key_hints(active_tab);
    let left = Paragraph::new(Span::styled(
        format!(" {hints}"),
        Style::default().fg(theme.text_dim),
    ));
    frame.render_widget(left, cols[0]);

    // ── Centre: error / status ────────────────────────────────────────────
    let centre_text = if let Some(err) = last_error {
        Span::styled(format!(" ! {err}"), Style::default().fg(theme.danger))
    } else {
        Span::styled(" Ready", Style::default().fg(theme.text_dim))
    };
    let centre = Paragraph::new(centre_text).alignment(Alignment::Center);
    frame.render_widget(centre, cols[1]);

    // ── Right: time and mode ──────────────────────────────────────────────
    let now = chrono::Utc::now().format("%H:%M:%S UTC");
    let mode_label = match mode {
        InputMode::Normal => "NRM",
        InputMode::Insert => "INS",
        InputMode::Prompt => "PRM",
        InputMode::SessionPicker => "SES",
        InputMode::Command => "CMD",
    };
    let right = Paragraph::new(Line::from(vec![
        Span::styled(
            format!("{mode_label} "),
            Style::default().fg(theme.rose_dim),
        ),
        Span::styled(format!("{now} "), Style::default().fg(theme.text_dim)),
    ]))
    .alignment(Alignment::Right);
    frame.render_widget(right, cols[2]);
}

/// Returns the key-binding hint string for each tab.
fn key_hints(tab: Tab) -> &'static str {
    match tab {
        Tab::Dashboard => "F1-F8:tabs  q:quit  r:refresh  j/k:scroll",
        Tab::Agents => "j/k:select  p:prompt  Enter:detail  r:refresh  q:quit",
        Tab::Runs => "j/k:select  Enter:detail  r:refresh  q:quit",
        Tab::System => "r:refresh  q:quit",
        Tab::RunDetail => "Esc:back  Tab:panel  F5:timeline  r:refresh  q:quit",
        Tab::Timeline => "j/k:scroll  Enter:detail  Esc:back  r:refresh  q:quit",
        Tab::Approvals => "j/k:select  a:approve  d:deny  r:refresh  q:quit",
        Tab::Memory => "j/k:select  /:search  Del:forget  r:refresh  q:quit",
        Tab::Audit => "j/k:scroll  r:refresh  q:quit",
        Tab::Console => "p:prompt  s:sessions  x:cancel  F3:runs  F5:timeline  q:quit",
    }
}
