//! Action card widget.
//!
//! Renders a structured card for a single on-chain action with two visually
//! distinct sections:
//!
//! - **Canonical** — metadata-derived facts (pallet, call, parameters) shown
//!   in jade/bone.
//! - **Narrative** — model-generated human-readable summary, rendered in a
//!   separate visual box with an "AI Generated" badge.
//!
//! A risk indicator and truncated call hash are displayed at the bottom.

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};

use crate::tui::theme::Theme;

// ---------------------------------------------------------------------------
// Risk level
// ---------------------------------------------------------------------------

/// Risk classification for an action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RiskLevel {
    Low,
    Medium,
    High,
}

impl RiskLevel {
    /// Short label for display.
    pub fn label(self) -> &'static str {
        match self {
            Self::Low    => "LOW",
            Self::Medium => "MEDIUM",
            Self::High   => "HIGH",
        }
    }

    /// The glyph shown before the risk label.
    pub fn glyph(self) -> &'static str {
        match self {
            Self::Low    => "●",
            Self::Medium => "▲",
            Self::High   => "◆",
        }
    }

    /// Map risk level to the appropriate theme colour.
    fn color(self, theme: &Theme) -> ratatui::style::Color {
        match self {
            Self::Low    => theme.success,  // jade
            Self::Medium => theme.warning,  // amber
            Self::High   => theme.danger,   // crimson
        }
    }
}

// ---------------------------------------------------------------------------
// Data
// ---------------------------------------------------------------------------

/// Input data for a single action card.
pub struct ActionCardData<'a> {
    /// Pallet name (e.g. "Balances").
    pub pallet: &'a str,
    /// Call name (e.g. "transfer_keep_alive").
    pub call: &'a str,
    /// Key-value parameter pairs rendered in the canonical section.
    pub params: &'a [(&'a str, &'a str)],
    /// Model-generated narrative summary.
    pub narrative: &'a str,
    /// Risk classification.
    pub risk: RiskLevel,
    /// Hex-encoded call hash (will be truncated visually).
    pub hash: &'a str,
}

// ---------------------------------------------------------------------------
// Render
// ---------------------------------------------------------------------------

/// Render an action card into `area`.
///
/// The card occupies the full `area` and draws its own border. Minimum
/// useful height is around 8 rows.
pub fn render(frame: &mut Frame, area: Rect, data: &ActionCardData<'_>, theme: &Theme) {
    // Outer card border.
    let card_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border))
        .style(Style::default().bg(theme.bg_raised));
    let inner = card_block.inner(area);
    frame.render_widget(card_block, area);

    // Vertical layout: canonical | narrative | risk + hash.
    // Reserve 1 row for the risk/hash footer.
    let param_lines = data.params.len().max(1) as u16;
    // Title (pallet::call) = 1, blank = 1, params, blank = 1.
    let canonical_height = 1 + 1 + param_lines + 1;

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(canonical_height), // canonical section
            Constraint::Min(3),                   // narrative section
            Constraint::Length(1),                 // risk + hash footer
        ])
        .split(inner);

    // ── Canonical section ────────────────────────────────────────────────
    render_canonical(frame, rows[0], data, theme);

    // ── Narrative section ────────────────────────────────────────────────
    render_narrative(frame, rows[1], data.narrative, theme);

    // ── Footer: risk + hash ──────────────────────────────────────────────
    render_footer(frame, rows[2], data, theme);
}

/// Render the canonical metadata section.
fn render_canonical(frame: &mut Frame, area: Rect, data: &ActionCardData<'_>, theme: &Theme) {
    let mut lines: Vec<Line<'_>> = Vec::new();

    // Title: Pallet::Call
    lines.push(Line::from(vec![
        Span::styled(
            format!("{}::{}", data.pallet, data.call),
            Style::default()
                .fg(theme.bone)
                .add_modifier(Modifier::BOLD),
        ),
    ]));

    // Blank separator.
    lines.push(Line::raw(""));

    // Parameters.
    for (key, val) in data.params {
        lines.push(Line::from(vec![
            Span::styled(
                format!("  {key}: "),
                Style::default().fg(theme.success), // jade
            ),
            Span::styled(*val, Style::default().fg(theme.bone)),
        ]));
    }

    let para = Paragraph::new(lines).wrap(Wrap { trim: false });
    frame.render_widget(para, area);
}

/// Render the narrative section inside a bordered box with an "AI Generated"
/// label.
fn render_narrative(frame: &mut Frame, area: Rect, narrative: &str, theme: &Theme) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.dream))
        .title(Span::styled(
            " AI Generated ",
            Style::default()
                .fg(theme.dream)
                .add_modifier(Modifier::ITALIC),
        ))
        .style(Style::default().bg(theme.bg_mid));

    let para = Paragraph::new(Span::styled(
        narrative,
        Style::default().fg(theme.text_primary),
    ))
    .block(block)
    .wrap(Wrap { trim: false });

    frame.render_widget(para, area);
}

/// Render the risk indicator and call hash on a single line.
fn render_footer(frame: &mut Frame, area: Rect, data: &ActionCardData<'_>, theme: &Theme) {
    let risk_color = data.risk.color(theme);

    // Truncate hash to 16 characters for display.
    let hash_display = if data.hash.len() > 16 {
        format!("{}...", &data.hash[..16])
    } else {
        data.hash.to_string()
    };

    let line = Line::from(vec![
        Span::styled(
            format!(" {} Risk: {} ", data.risk.glyph(), data.risk.label()),
            Style::default()
                .fg(risk_color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("  ", Style::default()),
        Span::styled(
            format!("Hash: {hash_display}"),
            Style::default().fg(theme.text_dim),
        ),
    ]);

    frame.render_widget(Paragraph::new(line), area);
}
