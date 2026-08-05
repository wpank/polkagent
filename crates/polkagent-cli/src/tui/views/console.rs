//! Interactive console view (F9).

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap},
    Frame,
};

use crate::tui::input::InputMode;
use crate::tui::interaction::{
    ConsoleCommandStatus, ConsoleRunStatus, ConsoleSessionPicker, SessionPickerStatus,
    SlashCommandMenu,
};
use crate::tui::state::TuiState;
use crate::tui::theme::Theme;

pub fn render(
    frame: &mut Frame,
    area: Rect,
    state: &TuiState,
    input_mode: InputMode,
    theme: &Theme,
) {
    let composer_height = if input_mode == InputMode::Prompt {
        if state.interaction.slash_command_menu().is_some() {
            11
        } else {
            7
        }
    } else {
        3
    };
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(5),
            Constraint::Length(composer_height),
        ])
        .split(area);

    render_session(frame, rows[0], state, theme);
    render_transcript(frame, rows[1], state, theme);
    render_composer(frame, rows[2], state, input_mode, theme);
    if let Some(picker) = &state.interaction.session_picker {
        render_session_picker(frame, area, picker, theme);
    }
}

fn render_session(frame: &mut Frame, area: Rect, state: &TuiState, theme: &Theme) {
    let agent = state
        .interaction
        .agent_name
        .as_deref()
        .unwrap_or("no active agent");
    let run_status = state.interaction.run.as_ref().map(|run| {
        let color = match run.status {
            ConsoleRunStatus::Completed => theme.success,
            ConsoleRunStatus::Failed | ConsoleRunStatus::TimedOut => theme.danger,
            ConsoleRunStatus::Cancelled => theme.warning,
            _ => theme.rose_bright,
        };
        (run.status.label(), color)
    });
    let command_status = state.interaction.command_result.as_ref().map(|command| {
        let color = match command.status {
            ConsoleCommandStatus::Running => theme.rose_bright,
            ConsoleCommandStatus::Completed => theme.success,
            ConsoleCommandStatus::Failed => theme.danger,
        };
        (command.status.label(), color)
    });
    let active_command = state
        .interaction
        .command_result
        .as_ref()
        .filter(|command| command.status == ConsoleCommandStatus::Running)
        .and(command_status);
    let active_run = state
        .interaction
        .run
        .as_ref()
        .filter(|run| !run.status.is_terminal())
        .and(run_status);
    let selected_status = active_command
        .map(|status| ("Action", status))
        .or_else(|| active_run.map(|status| ("Turn", status)))
        .or_else(|| command_status.map(|status| ("Action", status)))
        .or_else(|| run_status.map(|status| ("Turn", status)));
    let (status_kind, status, color) = selected_status.map_or(
        ("State", "idle", theme.text_dim),
        |(kind, (status, color))| (kind, status, color),
    );
    let conversation_id = short_id(state.interaction.conversation_id.as_deref());
    let model = state
        .interaction
        .selected_model
        .as_deref()
        .unwrap_or("runtime/agent default");
    let turn_id = short_id(
        state
            .interaction
            .run
            .as_ref()
            .and_then(|run| run.turn_id.as_deref()),
    );
    let run_id = short_id(
        state
            .interaction
            .run
            .as_ref()
            .and_then(|run| run.run_id.as_deref()),
    );

    let block = Block::default()
        .title(Span::styled(
            " SESSION ",
            Style::default().fg(theme.rose).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.border_active))
        .style(Style::default().bg(theme.bg_raised));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(format!(" Agent  {agent}"), Style::default().fg(theme.bone)),
            Span::styled(
                format!("   {status_kind}  "),
                Style::default().fg(theme.text_dim),
            ),
            Span::styled(
                status,
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
            Span::styled("   Chat  ", Style::default().fg(theme.text_dim)),
            Span::styled(conversation_id, Style::default().fg(theme.text_primary)),
            Span::styled("   Model  ", Style::default().fg(theme.text_dim)),
            Span::styled(model, Style::default().fg(theme.text_primary)),
            Span::styled("   Turn  ", Style::default().fg(theme.text_dim)),
            Span::styled(turn_id, Style::default().fg(theme.text_primary)),
            Span::styled("   Run  ", Style::default().fg(theme.text_dim)),
            Span::styled(run_id, Style::default().fg(theme.text_primary)),
        ])),
        inner,
    );
}

fn render_transcript(frame: &mut Frame, area: Rect, state: &TuiState, theme: &Theme) {
    let block = Block::default()
        .title(Span::styled(
            " LIVE OUTPUT ",
            Style::default().fg(theme.rose).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.border))
        .style(Style::default().bg(theme.bg_raised));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines: Vec<Line<'static>> = Vec::new();
    let turns = state
        .interaction
        .transcript
        .iter()
        .chain(state.interaction.run.iter())
        .collect::<Vec<_>>();
    if turns.is_empty() && state.interaction.command_result.is_none() {
        lines.push(Line::from(Span::styled(
            " Choose an active agent in F2, then press p to compose a prompt.",
            Style::default().fg(theme.text_dim),
        )));
        lines.push(Line::from(Span::styled(
            " You can also press p here to use the first active agent.",
            Style::default().fg(theme.text_dim),
        )));
    } else if !turns.is_empty() {
        for (index, run) in turns.iter().enumerate() {
            if index > 0 {
                lines.push(Line::from(""));
            }
            lines.push(Line::from(vec![
                Span::styled(" YOU  ", Style::default().fg(theme.rose_bright)),
                Span::styled(run.prompt.clone(), Style::default().fg(theme.text_primary)),
            ]));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                " AGENT",
                Style::default().fg(theme.bone).add_modifier(Modifier::BOLD),
            )));
            if run.output.is_empty() {
                lines.push(Line::from(Span::styled(
                    format!("   {}", run.detail),
                    Style::default().fg(theme.text_dim),
                )));
            } else {
                lines.extend(run.output.lines().map(|line| {
                    Line::from(Span::styled(
                        format!(" {line}"),
                        Style::default().fg(theme.text_primary),
                    ))
                }));
            }
            let usage = if run.status == ConsoleRunStatus::Completed {
                format!(
                    " {} · {} input / {} output tokens · turn {} · run {}",
                    run.detail,
                    run.input_tokens,
                    run.output_tokens,
                    short_id(run.turn_id.as_deref()),
                    short_id(run.run_id.as_deref())
                )
            } else {
                format!(
                    " {} · turn {} · run {}",
                    run.detail,
                    short_id(run.turn_id.as_deref()),
                    short_id(run.run_id.as_deref())
                )
            };
            lines.push(Line::from(Span::styled(
                usage,
                Style::default().fg(theme.text_dim),
            )));
        }
    }

    if let Some(command) = &state.interaction.command_result {
        if !lines.is_empty() {
            lines.push(Line::from(""));
        }
        let color = match command.status {
            ConsoleCommandStatus::Running => theme.rose_bright,
            ConsoleCommandStatus::Completed => theme.success,
            ConsoleCommandStatus::Failed => theme.danger,
        };
        lines.push(Line::from(vec![
            Span::styled(" COMMAND  ", Style::default().fg(theme.rose_bright)),
            Span::styled(
                command.line.clone(),
                Style::default().fg(theme.text_primary),
            ),
        ]));
        lines.push(Line::from(vec![
            Span::styled(
                format!(" {} ", command.status.label()),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
            Span::styled(command.title.clone(), Style::default().fg(theme.bone)),
        ]));
        lines.extend(command.lines.iter().map(|line| {
            Line::from(Span::styled(
                format!("   {line}"),
                Style::default().fg(theme.text_primary),
            ))
        }));
    }

    let height = usize::from(inner.height);
    let scroll = u16::try_from(lines.len().saturating_sub(height)).unwrap_or(u16::MAX);
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0)),
        inner,
    );
}

fn short_id(id: Option<&str>) -> &str {
    id.map_or("--------", |id| &id[..8.min(id.len())])
}

fn render_composer(
    frame: &mut Frame,
    area: Rect,
    state: &TuiState,
    input_mode: InputMode,
    theme: &Theme,
) {
    let composing = input_mode == InputMode::Prompt;
    let slash_menu = if composing {
        state.interaction.slash_command_menu()
    } else {
        None
    };
    let title = if slash_menu.is_some() {
        " SLASH HELP · ↑/↓ select · Tab complete · Esc dismiss "
    } else if composing {
        " PROMPT / COMMAND · Enter submit · Shift+Enter newline · ↑/↓ line/history · Esc cancel "
    } else if state
        .interaction
        .run
        .as_ref()
        .is_some_and(|run| !run.status.is_terminal())
    {
        " RUNNING · x cancel "
    } else {
        " p compose · s sessions · F3 runs · F5 timeline "
    };
    let border = if composing {
        theme.rose_bright
    } else {
        theme.border
    };
    let block = Block::default()
        .title(Span::styled(title, Style::default().fg(theme.text_dim)))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border))
        .style(Style::default().bg(theme.bg_raised));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if !composing {
        frame.render_widget(
            Paragraph::new(Span::styled(
                "> press p to write the next prompt or Console command",
                Style::default().fg(theme.text_primary),
            )),
            inner,
        );
        return;
    }

    let editor_area = if let Some(menu) = slash_menu.as_ref() {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(1)])
            .split(inner);
        render_slash_menu(frame, rows[1], menu, theme);
        rows[0]
    } else {
        inner
    };

    let buffer = &state.interaction.prompt_buffer;
    let cursor = state.interaction.cursor();
    let before_cursor = &buffer[..cursor];
    let cursor_line = before_cursor.bytes().filter(|byte| *byte == b'\n').count();
    let line_start = before_cursor.rfind('\n').map_or(0, |index| index + 1);
    let cursor_column = 2 + Span::raw(&buffer[line_start..cursor]).width();

    let lines = buffer.split('\n').enumerate().map(|(index, line)| {
        let prefix = if index == 0 { "> " } else { "  " };
        Line::from(vec![
            Span::styled(prefix, Style::default().fg(theme.rose_bright)),
            Span::styled(line.to_owned(), Style::default().fg(theme.text_primary)),
        ])
    });
    let visible_height = usize::from(editor_area.height.max(1));
    let visible_width = usize::from(editor_area.width.max(1));
    let vertical_scroll = cursor_line.saturating_sub(visible_height.saturating_sub(1));
    let horizontal_scroll = cursor_column.saturating_sub(visible_width.saturating_sub(1));
    frame.render_widget(
        Paragraph::new(lines.collect::<Text>()).scroll((
            u16::try_from(vertical_scroll).unwrap_or(u16::MAX),
            u16::try_from(horizontal_scroll).unwrap_or(u16::MAX),
        )),
        editor_area,
    );

    if editor_area.width > 0 && editor_area.height > 0 {
        let cursor_x = editor_area
            .x
            .saturating_add(
                u16::try_from(cursor_column.saturating_sub(horizontal_scroll)).unwrap_or(u16::MAX),
            )
            .min(
                editor_area
                    .x
                    .saturating_add(editor_area.width.saturating_sub(1)),
            );
        let cursor_y = editor_area
            .y
            .saturating_add(
                u16::try_from(cursor_line.saturating_sub(vertical_scroll)).unwrap_or(u16::MAX),
            )
            .min(
                editor_area
                    .y
                    .saturating_add(editor_area.height.saturating_sub(1)),
            );
        frame.set_cursor_position((cursor_x, cursor_y));
    }
}

fn render_session_picker(
    frame: &mut Frame,
    area: Rect,
    picker: &ConsoleSessionPicker,
    theme: &Theme,
) {
    let width = area.width.saturating_sub(4).min(110);
    let desired_height = u16::try_from(picker.sessions.len().min(12) + 9).unwrap_or(u16::MAX);
    let height = area.height.saturating_sub(2).min(desired_height);
    if width < 12 || height < 6 {
        return;
    }
    let overlay = Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, overlay);
    let block = Block::default()
        .title(Span::styled(
            " DURABLE SESSIONS ",
            Style::default()
                .fg(theme.rose_bright)
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.border_active))
        .style(Style::default().bg(theme.bg_mid));
    let inner = block.inner(overlay);
    frame.render_widget(block, overlay);

    let status = match picker.status {
        SessionPickerStatus::LoadingList => "loading recent same-agent sessions…",
        SessionPickerStatus::Ready => "ready",
        SessionPickerStatus::LoadingSelection => "loading selected transcript…",
        SessionPickerStatus::Failed => "service request failed",
    };
    let mut lines = vec![
        Line::from(vec![
            Span::styled(" Agent  ", Style::default().fg(theme.text_dim)),
            Span::styled(
                short_id(Some(&picker.agent_id)),
                Style::default().fg(theme.bone),
            ),
            Span::styled("   State  ", Style::default().fg(theme.text_dim)),
            Span::styled(status, Style::default().fg(theme.rose_bright)),
        ]),
        Line::from(""),
    ];

    if picker.sessions.is_empty() && picker.status == SessionPickerStatus::Ready {
        lines.push(Line::from(Span::styled(
            " No durable single-agent sessions found. Use /new [title] in the composer.",
            Style::default().fg(theme.text_primary),
        )));
    } else if !picker.sessions.is_empty() {
        lines.push(Line::from(Span::styled(
            "     TITLE · ID       STATE      TURNS   UPDATED",
            Style::default().fg(theme.text_dim),
        )));
        let visible = usize::from(inner.height).saturating_sub(6).max(1);
        let start = picker
            .selected
            .saturating_sub(visible.saturating_sub(1))
            .min(picker.sessions.len().saturating_sub(visible));
        for (index, session) in picker.sessions.iter().enumerate().skip(start).take(visible) {
            let selected = index == picker.selected;
            let style = if selected {
                Style::default().fg(theme.bone).bg(theme.bg_highlight)
            } else {
                Style::default().fg(theme.text_primary)
            };
            lines.push(Line::from(Span::styled(
                format!(
                    " {} {} · {}   {:<10} {:>4}   {}",
                    if selected { "›" } else { " " },
                    session.title,
                    short_id(Some(&session.conversation_id)),
                    session.state,
                    session.turn_count,
                    session.updated_at
                ),
                style,
            )));
        }
    }

    if let Some(error) = &picker.error {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!(" ! {error}"),
            Style::default().fg(theme.danger),
        )));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        " ↑/↓ or j/k navigate · Enter resume · Esc close · /new [title] creates",
        Style::default().fg(theme.text_dim),
    )));
    frame.render_widget(
        Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false }),
        inner,
    );
}

fn render_slash_menu(frame: &mut Frame, area: Rect, menu: &SlashCommandMenu, theme: &Theme) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let visible_entries = usize::from(area.height.saturating_sub(1));
    let window_start = menu
        .selected
        .saturating_sub(visible_entries.saturating_sub(1))
        .min(menu.candidates.len().saturating_sub(visible_entries));
    let mut lines = menu
        .candidates
        .iter()
        .enumerate()
        .skip(window_start)
        .take(visible_entries)
        .map(|(index, candidate)| {
            let selected = index == menu.selected;
            let marker = if selected { "▸ " } else { "  " };
            let command_style = if selected {
                Style::default()
                    .fg(theme.rose_bright)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.bone)
            };
            let aliases = if selected && !candidate.aliases.is_empty() {
                format!(" · aliases: /{}", candidate.aliases.join(", /"))
            } else {
                String::new()
            };
            Line::from(vec![
                Span::styled(marker, command_style),
                Span::styled(candidate.usage(), command_style),
                Span::styled(
                    format!(" — {}{aliases}", candidate.description),
                    Style::default().fg(theme.text_dim),
                ),
            ])
        })
        .collect::<Vec<_>>();
    lines.push(Line::from(Span::styled(
        format!(
            "  Executable Console commands · {}/{} · x cancels the active turn",
            menu.selected + 1,
            menu.candidates.len()
        ),
        Style::default().fg(theme.success),
    )));
    frame.render_widget(Paragraph::new(lines), area);
}
