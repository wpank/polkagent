//! Run Detail view — drills into a single run.
//!
//! Shows full run info (ID, agent, state, timestamps), token usage summary,
//! effect count breakdown, and a scrollable turn list.
//!
//! Keyboard: Esc to go back, Tab to switch between info and turns panels.

use chrono::Utc;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
    Frame,
};

use crate::tui::state::{RunDetail, TuiState};
use crate::tui::theme::Theme;

// ---------------------------------------------------------------------------
// Public render entry point
// ---------------------------------------------------------------------------

/// Render the run detail view into `area`.
///
/// If no run is selected (`tui_state.run_detail` is `None`), shows a
/// placeholder message directing the user back to the runs list.
pub fn render(frame: &mut Frame, area: Rect, state: &TuiState, theme: &Theme) {
    let Some(detail) = &state.run_detail else {
        render_no_selection(frame, area, theme);
        return;
    };

    // Layout: left info panel (50%) | right turns panel (50%)
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    render_info_panel(frame, cols[0], detail, state.detail_panel_index == 0, theme);
    render_turns_panel(frame, cols[1], detail, state.detail_panel_index == 1, theme);
}

// ---------------------------------------------------------------------------
// No selection placeholder
// ---------------------------------------------------------------------------

fn render_no_selection(frame: &mut Frame, area: Rect, theme: &Theme) {
    let block = styled_block(" RUN DETAIL ", false, theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let msg = Paragraph::new(vec![
        Line::from(""),
        Line::from(Span::styled(
            "  No run selected.",
            Style::default().fg(theme.text_dim),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  Go to Runs (F3) and press Enter on a run to view details.",
            Style::default().fg(theme.text_dim),
        )),
    ]);
    frame.render_widget(msg, inner);
}

// ---------------------------------------------------------------------------
// Info panel (left)
// ---------------------------------------------------------------------------

fn render_info_panel(
    frame: &mut Frame,
    area: Rect,
    detail: &RunDetail,
    focused: bool,
    theme: &Theme,
) {
    let title = format!(" {} Run {} ", detail.state_glyph(), detail.short_id);
    let block = styled_block(&title, focused, theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let now = Utc::now();
    let state_color = theme.status_color(&detail.state);
    let created = detail.created_at.format("%Y-%m-%d %H:%M:%S UTC").to_string();
    let updated = detail.updated_at.format("%Y-%m-%d %H:%M:%S UTC").to_string();
    let completed = detail
        .completed_at
        .map(|t| t.format("%Y-%m-%d %H:%M:%S UTC").to_string())
        .unwrap_or_else(|| "--".into());
    let duration = detail.duration_display(now);
    let total_tokens = detail.input_tokens + detail.output_tokens;

    let mut lines: Vec<Line> = vec![
        // State header.
        Line::from(vec![
            Span::styled(
                detail.state_glyph(),
                Style::default().fg(state_color),
            ),
            Span::raw("  "),
            Span::styled(
                detail.state.clone(),
                Style::default().fg(state_color).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
        // Identity.
        section_header("Identity", theme),
        kv_line("  Run ID", &detail.id, theme.text_primary, theme),
        kv_line("  Agent", &detail.agent_name, theme.text_primary, theme),
        Line::from(""),
        // Timestamps.
        section_header("Timestamps", theme),
        kv_line("  Created", &created, theme.text_primary, theme),
        kv_line("  Updated", &updated, theme.text_primary, theme),
        kv_line("  Completed", &completed, theme.text_primary, theme),
        kv_line("  Duration", &duration, theme.text_primary, theme),
        Line::from(""),
        // Token usage.
        section_header("Token Usage", theme),
        kv_line("  Input", &format_tokens(detail.input_tokens), theme.text_primary, theme),
        kv_line("  Output", &format_tokens(detail.output_tokens), theme.text_primary, theme),
        kv_line("  Total", &format_tokens(total_tokens), theme.rose, theme),
        Line::from(""),
    ];

    // Effect counts (only if there are effects).
    if detail.effect_count > 0 {
        let eff_total = detail.effect_count.to_string();
        let eff_ok = detail.effects_succeeded.to_string();
        let eff_fail = detail.effects_failed.to_string();
        let eff_pend = detail.effects_pending.to_string();

        lines.push(section_header("Effects", theme));
        lines.push(kv_line("  Total", &eff_total, theme.text_primary, theme));
        lines.push(kv_line("  Succeeded", &eff_ok, theme.success, theme));
        lines.push(kv_line("  Failed", &eff_fail, theme.danger, theme));
        lines.push(kv_line("  Pending", &eff_pend, theme.warning, theme));
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

// ---------------------------------------------------------------------------
// Turns panel (right)
// ---------------------------------------------------------------------------

fn render_turns_panel(
    frame: &mut Frame,
    area: Rect,
    detail: &RunDetail,
    focused: bool,
    theme: &Theme,
) {
    let turn_label = format!(" TURNS ({}) ", detail.turn_count);
    let block = styled_block(&turn_label, focused, theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if detail.turns.is_empty() {
        let msg = Paragraph::new(Span::styled(
            "  No turns recorded.",
            Style::default().fg(theme.text_dim),
        ));
        frame.render_widget(msg, inner);
        return;
    }

    // Header line.
    let mut lines: Vec<Line> = vec![
        Line::from(vec![
            Span::styled(
                format!("  {:<4} {:<10} {:<8} {:<8} {:>8}",
                    "#", "Role", "In tok", "Out tok", "Time"),
                Style::default().fg(theme.text_dim).add_modifier(Modifier::UNDERLINED),
            ),
        ]),
    ];

    // Turn rows.
    let visible = inner.height.saturating_sub(1) as usize;
    for turn in detail.turns.iter().take(visible) {
        let role_color = match turn.role.as_str() {
            "assistant" => theme.rose,
            "user"      => theme.bone,
            "system"    => theme.dream,
            "tool"      => theme.success,
            _           => theme.text_primary,
        };

        let elapsed = turn
            .completed_at
            .map(|end| {
                let secs = (end - turn.started_at).num_milliseconds().max(0) as f64 / 1000.0;
                format!("{secs:.1}s")
            })
            .unwrap_or_else(|| "...".into());

        let in_tok = format_tokens(turn.input_tokens);
        let out_tok = format_tokens(turn.output_tokens);

        lines.push(Line::from(vec![
            Span::styled(
                format!("  {:<4}", turn.sequence),
                Style::default().fg(theme.text_dim),
            ),
            Span::styled(
                format!("{:<10}", turn.role),
                Style::default().fg(role_color),
            ),
            Span::styled(
                format!(" {:<8}", in_tok),
                Style::default().fg(theme.text_primary),
            ),
            Span::styled(
                format!("{:<8}", out_tok),
                Style::default().fg(theme.text_primary),
            ),
            Span::styled(
                format!("{:>8}", elapsed),
                Style::default().fg(theme.text_dim),
            ),
        ]));
    }

    // "More" indicator if truncated.
    if detail.turns.len() > visible {
        lines.push(Line::from(Span::styled(
            format!("  ... {} more turns", detail.turns.len() - visible),
            Style::default().fg(theme.text_dim),
        )));
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn styled_block<'a>(title: &'a str, focused: bool, theme: &'a Theme) -> Block<'a> {
    let border_color = if focused { theme.border_active } else { theme.border };
    Block::default()
        .title(Span::styled(
            title,
            Style::default()
                .fg(if focused { theme.bone } else { theme.rose })
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border_color))
        .style(Style::default().bg(theme.bg_raised))
}

fn section_header(title: &str, theme: &Theme) -> Line<'static> {
    Line::from(Span::styled(
        format!("  {title}"),
        Style::default()
            .fg(theme.bone)
            .add_modifier(Modifier::BOLD),
    ))
}

fn kv_line(
    key: &str,
    value: &str,
    value_color: ratatui::style::Color,
    theme: &Theme,
) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{key:<14}"),
            Style::default().fg(theme.text_dim),
        ),
        Span::styled(value.to_owned(), Style::default().fg(value_color)),
    ])
}

/// Format a token count for compact display.
fn format_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}
