//! Runs tab view (F3 — Screen 3.1 Run List).
//!
//! Shows a sortable list of runs with state, duration, and token usage.
//! Selecting a run shows a compact detail view on the right when width >= 100.

use chrono::Utc;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Cell, Paragraph, Row, Table},
    Frame,
};

use crate::tui::state::{RunSummary, ScrollState, TuiState};
use crate::tui::theme::Theme;

// ---------------------------------------------------------------------------
// Public render entry point
// ---------------------------------------------------------------------------

pub fn render(frame: &mut Frame, area: Rect, state: &TuiState, theme: &Theme) {
    if area.width >= 100 && state.runs_scroll.selected.is_some() {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
            .split(area);

        render_list(frame, cols[0], &state.runs, &state.runs_scroll, theme);
        if let Some(sel) = state.runs_scroll.selected {
            if let Some(run) = state.runs.get(sel) {
                render_detail(frame, cols[1], run, theme);
            }
        }
    } else {
        render_list(frame, area, &state.runs, &state.runs_scroll, theme);
    }
}

// ---------------------------------------------------------------------------
// Run list table
// ---------------------------------------------------------------------------

fn render_list(
    frame: &mut Frame,
    area: Rect,
    runs: &[RunSummary],
    scroll: &ScrollState,
    theme: &Theme,
) {
    let block = Block::default()
        .title(Span::styled(
            " RUNS ",
            Style::default().fg(theme.rose).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.border))
        .style(Style::default().bg(theme.bg_raised));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if runs.is_empty() {
        let msg = Paragraph::new(Span::styled(
            "\n  No runs found. Use `polkagent run --agent-id <ID> --prompt <TEXT>`.",
            Style::default().fg(theme.text_dim),
        ));
        frame.render_widget(msg, inner);
        return;
    }

    let header = Row::new(vec![
        Cell::from(Span::styled(" ST", Style::default().fg(theme.text_dim))),
        Cell::from(Span::styled("Run", Style::default().fg(theme.text_dim))),
        Cell::from(Span::styled("Agent", Style::default().fg(theme.text_dim))),
        Cell::from(Span::styled("State", Style::default().fg(theme.text_dim))),
        Cell::from(Span::styled("Dur", Style::default().fg(theme.text_dim))),
        Cell::from(Span::styled("Tokens", Style::default().fg(theme.text_dim))),
    ])
    .height(1)
    .style(Style::default().add_modifier(Modifier::UNDERLINED));

    let now = Utc::now();
    let selected_idx = scroll.selected.unwrap_or(usize::MAX);

    let rows: Vec<Row> = runs
        .iter()
        .skip(scroll.offset)
        .take(inner.height.saturating_sub(1) as usize)
        .enumerate()
        .map(|(vis_idx, r)| {
            let abs_idx = vis_idx + scroll.offset;
            let is_selected = abs_idx == selected_idx;

            let row_style = if is_selected {
                Style::default().bg(theme.bg_highlight).fg(theme.rose_bright)
            } else {
                Style::default().fg(theme.text_primary)
            };

            let state_color = theme.status_color(&r.state);
            let total_tokens = r.input_tokens + r.output_tokens;
            let token_str = format_tokens(total_tokens);
            let duration = r.duration_display(now);

            // Truncate agent name.
            let agent = if r.agent_name.len() > 14 {
                format!("{}…", &r.agent_name[..13])
            } else {
                r.agent_name.clone()
            };

            Row::new(vec![
                Cell::from(Span::styled(
                    format!(" {}", r.state_glyph()),
                    Style::default().fg(state_color),
                )),
                Cell::from(r.short_id.clone()),
                Cell::from(agent),
                Cell::from(Span::styled(r.state.clone(), Style::default().fg(state_color))),
                Cell::from(duration),
                Cell::from(Span::styled(token_str, Style::default().fg(theme.text_dim))),
            ])
            .height(1)
            .style(row_style)
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(4),
            Constraint::Length(10),
            Constraint::Min(12),
            Constraint::Length(12),
            Constraint::Length(7),
            Constraint::Length(8),
        ],
    )
    .header(header);

    frame.render_widget(table, inner);
}

// ---------------------------------------------------------------------------
// Run detail panel
// ---------------------------------------------------------------------------

fn render_detail(frame: &mut Frame, area: Rect, run: &RunSummary, theme: &Theme) {
    let state_color = theme.status_color(&run.state);

    let block = Block::default()
        .title(Span::styled(
            format!(" Run {} ", run.short_id),
            Style::default().fg(theme.bone).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.border_active))
        .style(Style::default().bg(theme.bg_raised));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let now = Utc::now();
    let started = run.created_at.format("%Y-%m-%d %H:%M:%S UTC").to_string();
    let total_tokens = run.input_tokens + run.output_tokens;
    let duration_str = run.duration_display(now);
    let turns_str = run.turn_count.to_string();
    let in_tok_str = format_tokens(run.input_tokens);
    let out_tok_str = format_tokens(run.output_tokens);
    let total_tok_str = format_tokens(total_tokens);

    let lines: Vec<Line> = vec![
        Line::from(vec![
            Span::styled(run.state_glyph(), Style::default().fg(state_color)),
            Span::raw("  "),
            Span::styled(
                run.state.clone(),
                Style::default().fg(state_color).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
        detail_row("Run ID", &run.id, theme),
        detail_row("Agent", &run.agent_name, theme),
        detail_row("Started", &started, theme),
        detail_row("Duration", &duration_str, theme),
        detail_row("Turns", &turns_str, theme),
        detail_row("In tokens", &in_tok_str, theme),
        detail_row("Out tokens", &out_tok_str, theme),
        detail_row("Total tokens", &total_tok_str, theme),
    ];

    frame.render_widget(Paragraph::new(lines), inner);
}

fn detail_row(label: &str, value: &str, theme: &Theme) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("  {label:<13}"),
            Style::default().fg(theme.text_dim),
        ),
        Span::styled(value.to_owned(), Style::default().fg(theme.text_primary)),
    ])
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

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
