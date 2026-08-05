//! Memory Browser view — browse and search memory entries.
//!
//! Lists memory entries with type and relevance columns. A search bar at the
//! top accepts FTS (full-text-search) queries. Selecting an entry shows its
//! full content in a detail pane. Pressing Delete removes the selected entry.
//!
//! Color conventions (ROSEDUST):
//! - Memory type "episodic" — bone
//! - Memory type "semantic"  — dream (violet)
//! - Memory type "working"   — rose
//! - High relevance (>= 0.8) — success (jade)
//! - Medium relevance        — warning (amber)
//! - Low relevance           — danger (crimson)
//!
//! Keyboard: j/k navigate, Enter to view detail, Del to delete, '/' to search,
//! Esc to cancel search or go back.

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Cell, Paragraph, Row, Table, Wrap},
    Frame,
};

use crate::tui::state::{MemoryEntry, ScrollState, TuiState};
use crate::tui::theme::Theme;

// ---------------------------------------------------------------------------
// Public render entry point
// ---------------------------------------------------------------------------

/// Render the memory browser into `area`.
pub fn render(frame: &mut Frame, area: Rect, state: &TuiState, theme: &Theme) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(4)])
        .split(area);

    render_search_bar(frame, rows[0], &state.memory_search_query, theme);

    let list_area = rows[1];

    if area.width >= 100 && state.memory_scroll.selected.is_some() {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
            .split(list_area);

        render_list(
            frame,
            cols[0],
            &state.memory_entries,
            &state.memory_scroll,
            theme,
        );
        if let Some(sel) = state.memory_scroll.selected {
            if let Some(entry) = state.memory_entries.get(sel) {
                render_detail(frame, cols[1], entry, theme);
            }
        }
    } else {
        render_list(
            frame,
            list_area,
            &state.memory_entries,
            &state.memory_scroll,
            theme,
        );
    }
}

// ---------------------------------------------------------------------------
// Search bar
// ---------------------------------------------------------------------------

fn render_search_bar(frame: &mut Frame, area: Rect, query: &str, theme: &Theme) {
    let display = if query.is_empty() {
        " / Search memory… ".to_string()
    } else {
        format!(" / {query} ")
    };

    let hint_color = if query.is_empty() {
        theme.text_dim
    } else {
        theme.bone
    };

    let block = Block::default()
        .title(Span::styled(
            " SEARCH ",
            Style::default()
                .fg(theme.dream)
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(if query.is_empty() {
            theme.border
        } else {
            theme.border_dream
        }))
        .style(Style::default().bg(theme.bg_raised));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    frame.render_widget(
        Paragraph::new(Span::styled(display, Style::default().fg(hint_color))),
        inner,
    );
}

// ---------------------------------------------------------------------------
// Memory entry list
// ---------------------------------------------------------------------------

fn render_list(
    frame: &mut Frame,
    area: Rect,
    entries: &[MemoryEntry],
    scroll: &ScrollState,
    theme: &Theme,
) {
    let title = format!(" MEMORY BROWSER ({} entries) ", entries.len());

    let block = Block::default()
        .title(Span::styled(
            title,
            Style::default()
                .fg(theme.dream)
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.border_dream))
        .style(Style::default().bg(theme.bg_raised));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if entries.is_empty() {
        let msg = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(
                "  No memory entries found.",
                Style::default().fg(theme.text_dim),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "  Use '/' to search for specific entries.",
                Style::default().fg(theme.text_dim),
            )),
        ]);
        frame.render_widget(msg, inner);

        // Footer.
        render_list_footer(frame, area, theme);
        return;
    }

    // Header row.
    let header = Row::new(vec![
        Cell::from(Span::styled("Type", Style::default().fg(theme.text_dim))),
        Cell::from(Span::styled(
            "Relevance",
            Style::default().fg(theme.text_dim),
        )),
        Cell::from(Span::styled("Agent", Style::default().fg(theme.text_dim))),
        Cell::from(Span::styled(
            "Content (preview)",
            Style::default().fg(theme.text_dim),
        )),
    ])
    .height(1)
    .style(Style::default().add_modifier(Modifier::UNDERLINED));

    let selected_idx = scroll.selected.unwrap_or(usize::MAX);
    let visible = inner.height.saturating_sub(1) as usize;

    let rows: Vec<Row> = entries
        .iter()
        .skip(scroll.offset)
        .take(visible)
        .enumerate()
        .map(|(vis_idx, entry)| {
            let abs_idx = vis_idx + scroll.offset;
            let is_selected = abs_idx == selected_idx;

            let row_style = if is_selected {
                Style::default()
                    .bg(theme.bg_highlight)
                    .fg(theme.rose_bright)
            } else {
                Style::default().fg(theme.text_primary)
            };

            let type_color = memory_type_color(&entry.memory_type, theme);
            let relevance_color = relevance_color(entry.relevance_score, theme);
            let relevance_str = format!("{:.2}", entry.relevance_score);

            // Truncate agent name to 14 chars (char-boundary safe).
            let agent = if entry.agent_name.chars().count() > 14 {
                let split_byte = entry
                    .agent_name
                    .char_indices()
                    .nth(13)
                    .map_or(entry.agent_name.len(), |(i, _)| i);
                format!("{}…", &entry.agent_name[..split_byte])
            } else {
                entry.agent_name.clone()
            };

            // Preview: first 40 chars of content, single line.
            let preview_max = (inner.width as usize).saturating_sub(50).min(60);
            let preview = entry
                .content
                .lines()
                .next()
                .unwrap_or("")
                .chars()
                .take(preview_max.max(10))
                .collect::<String>();
            let preview = if entry.content.len() > preview_max {
                format!("{preview}…")
            } else {
                preview
            };

            Row::new(vec![
                Cell::from(Span::styled(
                    entry.memory_type.clone(),
                    Style::default().fg(type_color),
                )),
                Cell::from(Span::styled(
                    relevance_str,
                    Style::default()
                        .fg(relevance_color)
                        .add_modifier(Modifier::BOLD),
                )),
                Cell::from(agent),
                Cell::from(Span::styled(
                    preview,
                    Style::default().fg(theme.text_primary),
                )),
            ])
            .height(1)
            .style(row_style)
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(12),
            Constraint::Length(10),
            Constraint::Length(16),
            Constraint::Min(20),
        ],
    )
    .header(header);

    frame.render_widget(table, inner);
    render_list_footer(frame, area, theme);
}

fn render_list_footer(frame: &mut Frame, area: Rect, theme: &Theme) {
    let footer_y = area.y + area.height.saturating_sub(1);
    if footer_y < area.y + area.height {
        let footer_area = Rect {
            y: footer_y,
            height: 1,
            x: area.x + 1,
            width: area.width.saturating_sub(2),
        };
        frame.render_widget(
            Paragraph::new(Span::styled(
                " /: search  Del: delete  j/k: navigate  Esc: back",
                Style::default().fg(theme.text_dim),
            )),
            footer_area,
        );
    }
}

// ---------------------------------------------------------------------------
// Memory entry detail panel
// ---------------------------------------------------------------------------

fn render_detail(frame: &mut Frame, area: Rect, entry: &MemoryEntry, theme: &Theme) {
    let type_color = memory_type_color(&entry.memory_type, theme);
    let relevance_color = relevance_color(entry.relevance_score, theme);

    let block = Block::default()
        .title(Span::styled(
            format!(" Memory: {} ", entry.memory_type),
            Style::default().fg(theme.bone).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.border_dream))
        .style(Style::default().bg(theme.bg_raised));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let created = entry.created_at.format("%Y-%m-%d %H:%M:%S UTC").to_string();
    let short_id = &entry.id[..8.min(entry.id.len())];
    let relevance_str = format!("{:.4}", entry.relevance_score);

    let mut lines: Vec<Line> = vec![
        Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled(
                entry.memory_type.clone(),
                Style::default().fg(type_color).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
        kv_line("  ID", short_id, theme.text_dim, theme),
        kv_line("  Agent", &entry.agent_name, theme.text_primary, theme),
        kv_line("  Relevance", &relevance_str, relevance_color, theme),
        kv_line("  Created", &created, theme.text_primary, theme),
        Line::from(""),
        Line::from(Span::styled(
            "  Content:",
            Style::default().fg(theme.bone).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
    ];

    // Content — word-wrap to fit available width.
    let content_width = inner.width.saturating_sub(4) as usize;
    let used_lines = u16::try_from(lines.len()).unwrap_or(u16::MAX);
    let content_lines_available = usize::from(inner.height.saturating_sub(used_lines));
    for line in entry.content.lines().take(content_lines_available.max(1)) {
        // Split each line at content_width using char boundaries (not byte indices)
        // to avoid panics on multi-byte UTF-8 characters.
        let mut remaining = line;
        loop {
            let char_count = remaining.chars().count();
            if char_count <= content_width {
                lines.push(Line::from(Span::styled(
                    format!("  {remaining}"),
                    Style::default().fg(theme.text_primary),
                )));
                break;
            }
            // Find the byte offset of the char_count-th character boundary.
            let split_byte = remaining
                .char_indices()
                .nth(content_width)
                .map_or(remaining.len(), |(i, _)| i);
            let chunk = &remaining[..split_byte];
            lines.push(Line::from(Span::styled(
                format!("  {chunk}"),
                Style::default().fg(theme.text_primary),
            )));
            remaining = &remaining[split_byte..];
        }
    }

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Map a memory entry type to its ROSEDUST color.
fn memory_type_color(memory_type: &str, theme: &Theme) -> ratatui::style::Color {
    match memory_type {
        "episodic" => theme.bone,
        "semantic" => theme.dream,
        "working" => theme.rose,
        "procedural" => theme.success,
        _ => theme.text_primary,
    }
}

/// Map a relevance score to a semantic color.
fn relevance_color(score: f64, theme: &Theme) -> ratatui::style::Color {
    if score >= 0.8 {
        theme.success
    } else if score >= 0.5 {
        theme.warning
    } else {
        theme.danger
    }
}

fn kv_line(
    key: &str,
    value: &str,
    value_color: ratatui::style::Color,
    theme: &Theme,
) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{key:<14}"), Style::default().fg(theme.text_dim)),
        Span::styled(value.to_owned(), Style::default().fg(value_color)),
    ])
}
