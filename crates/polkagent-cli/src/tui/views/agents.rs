//! Agents tab view (F2 — Screen 2.1 Agent List).
//!
//! Shows a table of agents with status glyphs and a detail panel for the
//! selected agent when enough horizontal space is available.

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Cell, Paragraph, Row, Table},
    Frame,
};

use crate::tui::state::{AgentSummary, ScrollState, TuiState};
use crate::tui::theme::Theme;

// ---------------------------------------------------------------------------
// Public render entry point
// ---------------------------------------------------------------------------

/// Render the agents list (and optionally a detail panel) into `area`.
pub fn render(frame: &mut Frame, area: Rect, state: &TuiState, theme: &Theme) {
    if area.width >= 100 && state.agents_scroll.selected.is_some() {
        // Wide: list on the left, detail on the right.
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
            .split(area);

        render_list(frame, cols[0], &state.agents, &state.agents_scroll, theme);
        if let Some(sel) = state.agents_scroll.selected {
            if let Some(agent) = state.agents.get(sel) {
                render_detail(frame, cols[1], agent, theme);
            }
        }
    } else {
        render_list(frame, area, &state.agents, &state.agents_scroll, theme);
    }
}

// ---------------------------------------------------------------------------
// Agent list table
// ---------------------------------------------------------------------------

fn render_list(
    frame: &mut Frame,
    area: Rect,
    agents: &[AgentSummary],
    scroll: &ScrollState,
    theme: &Theme,
) {
    let block = Block::default()
        .title(Span::styled(
            " AGENTS ",
            Style::default().fg(theme.rose).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.border))
        .style(Style::default().bg(theme.bg_raised));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if agents.is_empty() {
        let msg = Paragraph::new(Span::styled(
            "\n  No agents found. Run `polkagent agent create <NAME>` to create one.",
            Style::default().fg(theme.text_dim),
        ));
        frame.render_widget(msg, inner);
        return;
    }

    // Header row.
    let header = Row::new(vec![
        Cell::from(Span::styled("  ST", Style::default().fg(theme.text_dim))),
        Cell::from(Span::styled("Name", Style::default().fg(theme.text_dim))),
        Cell::from(Span::styled("State", Style::default().fg(theme.text_dim))),
        Cell::from(Span::styled("Runs", Style::default().fg(theme.text_dim))),
        Cell::from(Span::styled("Model", Style::default().fg(theme.text_dim))),
    ])
    .height(1)
    .style(Style::default().add_modifier(Modifier::UNDERLINED));

    // Data rows.
    let selected_idx = scroll.selected.unwrap_or(usize::MAX);
    let rows: Vec<Row> = agents
        .iter()
        .skip(scroll.offset)
        .take(inner.height.saturating_sub(1) as usize)
        .enumerate()
        .map(|(vis_idx, a)| {
            let abs_idx = vis_idx + scroll.offset;
            let is_selected = abs_idx == selected_idx;

            let row_style = if is_selected {
                Style::default().bg(theme.bg_highlight).fg(theme.rose_bright)
            } else {
                Style::default().fg(theme.text_primary)
            };

            let glyph_color = match a.state.as_str() {
                "active"    => theme.rose,
                "paused"    => theme.warning,
                "configured" | "created" => theme.text_dim,
                _ => theme.danger,
            };

            // Truncate model to 20 chars.
            let model = if a.model.len() > 20 {
                format!("{}…", &a.model[..19])
            } else {
                a.model.clone()
            };

            Row::new(vec![
                Cell::from(Span::styled(
                    format!("  {}", a.glyph()),
                    Style::default().fg(glyph_color),
                )),
                Cell::from(a.name.clone()),
                Cell::from(Span::styled(
                    a.status_label(),
                    Style::default().fg(theme.status_color(&a.state)),
                )),
                Cell::from(a.total_runs.to_string()),
                Cell::from(Span::styled(model, Style::default().fg(theme.text_dim))),
            ])
            .height(1)
            .style(row_style)
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(5),
            Constraint::Min(14),
            Constraint::Length(12),
            Constraint::Length(6),
            Constraint::Min(12),
        ],
    )
    .header(header);

    frame.render_widget(table, inner);

    // Summary footer line.
    let footer_y = area.y + area.height.saturating_sub(1);
    if footer_y < area.y + area.height {
        let active_count = agents.iter().filter(|a| a.state == "active").count();
        let idle_count = agents.iter().filter(|a| matches!(a.state.as_str(), "configured" | "created")).count();
        let err_count = agents.iter().filter(|a| a.active_runs == 0 && !matches!(a.state.as_str(), "active" | "configured" | "created" | "archived")).count();

        let summary = format!(
            " {} agents  ·  {} active  ·  {} idle  ·  {} error",
            agents.len(), active_count, idle_count, err_count
        );
        let footer_area = Rect {
            y: footer_y,
            height: 1,
            x: area.x + 1,
            width: area.width.saturating_sub(2),
        };
        frame.render_widget(
            Paragraph::new(Span::styled(summary, Style::default().fg(theme.text_dim))),
            footer_area,
        );
    }
}

// ---------------------------------------------------------------------------
// Agent detail panel
// ---------------------------------------------------------------------------

fn render_detail(frame: &mut Frame, area: Rect, agent: &AgentSummary, theme: &Theme) {
    let block = Block::default()
        .title(Span::styled(
            format!(" {} ", agent.name),
            Style::default().fg(theme.bone).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.border_active))
        .style(Style::default().bg(theme.bg_raised));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let glyph_color = match agent.state.as_str() {
        "active" => theme.rose,
        "paused" => theme.warning,
        "configured" | "created" => theme.text_dim,
        _ => theme.danger,
    };

    let updated = agent.updated_at.format("%Y-%m-%d %H:%M UTC").to_string();
    let total_runs_str = agent.total_runs.to_string();
    let active_runs_str = agent.active_runs.to_string();
    let short_id = &agent.id[..8.min(agent.id.len())];

    let lines: Vec<Line> = vec![
        Line::from(vec![
            Span::styled(agent.glyph(), Style::default().fg(glyph_color)),
            Span::raw("  "),
            Span::styled(
                agent.name.clone(),
                Style::default().fg(theme.bone).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
        detail_row("ID", short_id, theme),
        detail_row("State", agent.status_label(), theme),
        detail_row("Model", &agent.model, theme),
        detail_row("Total runs", &total_runs_str, theme),
        detail_row("Active runs", &active_runs_str, theme),
        detail_row("Updated", &updated, theme),
    ];

    frame.render_widget(Paragraph::new(lines), inner);
}

fn detail_row(label: &str, value: &str, theme: &Theme) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("  {label:<12}"),
            Style::default().fg(theme.text_dim),
        ),
        Span::styled(value.to_owned(), Style::default().fg(theme.text_primary)),
    ])
}
