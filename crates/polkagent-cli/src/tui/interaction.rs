//! Headless interaction state and asynchronous run controller for the TUI.
//!
//! Terminal input/rendering stays in `input` and `views::console`; this module
//! owns the deterministic reducer and the bridge to the same production run
//! bootstrap used by `polkagent run`.

use std::sync::mpsc;

use polkagent_config::Config;
use polkagent_core::event::EventKind;
use polkagent_store_sqlite::SqlitePool;

/// Keep a runaway streaming response from growing the terminal process
/// forever. Durable lifecycle/events remain available through the run views.
const MAX_OUTPUT_BYTES: usize = 128 * 1024;
/// Bound in-memory Console history until durable conversations own it.
const MAX_PROMPT_HISTORY: usize = 100;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptRequest {
    pub agent_id: String,
    pub agent_name: String,
    pub prompt: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConsoleRunStatus {
    Starting,
    Running,
    Cancelling,
    Completed,
    Failed,
    Cancelled,
    TimedOut,
}

impl ConsoleRunStatus {
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Cancelling => "cancelling",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::TimedOut => "timed out",
        }
    }

    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Cancelled | Self::TimedOut
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsoleRun {
    pub run_id: Option<String>,
    pub prompt: String,
    pub output: String,
    pub status: ConsoleRunStatus,
    pub detail: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

#[derive(Debug, Clone, Default)]
pub struct InteractionState {
    pub agent_id: Option<String>,
    pub agent_name: Option<String>,
    pub prompt_buffer: String,
    prompt_cursor: usize,
    prompt_history: Vec<String>,
    prompt_history_index: Option<usize>,
    prompt_history_draft: Option<String>,
    pub run: Option<ConsoleRun>,
}

impl InteractionState {
    pub fn select_agent(&mut self, agent_id: impl Into<String>, agent_name: impl Into<String>) {
        self.agent_id = Some(agent_id.into());
        self.agent_name = Some(agent_name.into());
        self.clear_prompt();
    }

    pub fn push_char(&mut self, c: char) {
        self.prepare_edit();
        self.prompt_buffer.insert(self.prompt_cursor, c);
        self.prompt_cursor += c.len_utf8();
    }

    pub fn insert_newline(&mut self) {
        self.push_char('\n');
    }

    pub fn backspace(&mut self) {
        self.prepare_edit();
        let previous = previous_char_boundary(&self.prompt_buffer, self.prompt_cursor);
        if previous < self.prompt_cursor {
            self.prompt_buffer.drain(previous..self.prompt_cursor);
            self.prompt_cursor = previous;
        }
    }

    pub fn delete(&mut self) {
        self.prepare_edit();
        let next = next_char_boundary(&self.prompt_buffer, self.prompt_cursor);
        if next > self.prompt_cursor {
            self.prompt_buffer.drain(self.prompt_cursor..next);
        }
    }

    pub fn move_left(&mut self) {
        self.prompt_cursor = previous_char_boundary(&self.prompt_buffer, self.cursor());
    }

    pub fn move_right(&mut self) {
        self.prompt_cursor = next_char_boundary(&self.prompt_buffer, self.cursor());
    }

    pub fn move_home(&mut self) {
        let cursor = self.cursor();
        self.prompt_cursor = self.prompt_buffer[..cursor]
            .rfind('\n')
            .map_or(0, |index| index + 1);
    }

    pub fn move_end(&mut self) {
        let cursor = self.cursor();
        self.prompt_cursor = self.prompt_buffer[cursor..]
            .find('\n')
            .map_or(self.prompt_buffer.len(), |offset| cursor + offset);
    }

    /// Move vertically within multiline input, falling back to older history
    /// only when the cursor is already on the first line.
    pub fn move_up(&mut self) {
        let cursor = self.cursor();
        let line_start = self.prompt_buffer[..cursor]
            .rfind('\n')
            .map_or(0, |index| index + 1);
        if line_start == 0 {
            self.history_previous();
            return;
        }

        let column = self.prompt_buffer[line_start..cursor].chars().count();
        let previous_end = line_start - 1;
        let previous_start = self.prompt_buffer[..previous_end]
            .rfind('\n')
            .map_or(0, |index| index + 1);
        self.prompt_cursor = previous_start
            + char_column_offset(&self.prompt_buffer[previous_start..previous_end], column);
    }

    /// Move vertically within multiline input, falling back to newer history
    /// only when the cursor is already on the last line.
    pub fn move_down(&mut self) {
        let cursor = self.cursor();
        let line_start = self.prompt_buffer[..cursor]
            .rfind('\n')
            .map_or(0, |index| index + 1);
        let Some(line_end_offset) = self.prompt_buffer[cursor..].find('\n') else {
            self.history_next();
            return;
        };
        let line_end = cursor + line_end_offset;
        let column = self.prompt_buffer[line_start..cursor].chars().count();
        let next_start = line_end + 1;
        let next_end = self.prompt_buffer[next_start..]
            .find('\n')
            .map_or(self.prompt_buffer.len(), |offset| next_start + offset);
        self.prompt_cursor =
            next_start + char_column_offset(&self.prompt_buffer[next_start..next_end], column);
    }

    #[must_use]
    pub fn cursor(&self) -> usize {
        clamp_char_boundary(&self.prompt_buffer, self.prompt_cursor)
    }

    pub fn clear_prompt(&mut self) {
        self.prompt_buffer.clear();
        self.prompt_cursor = 0;
        self.prompt_history_index = None;
        self.prompt_history_draft = None;
    }

    pub fn submit(&mut self) -> Result<PromptRequest, &'static str> {
        let prompt = self.prompt_buffer.trim().to_owned();
        if prompt.is_empty() {
            return Err("prompt cannot be empty");
        }
        let Some(agent_id) = self.agent_id.clone() else {
            return Err("select an active agent before prompting");
        };
        let agent_name = self.agent_name.clone().unwrap_or_else(|| agent_id.clone());
        self.record_history(&prompt);
        self.clear_prompt();
        self.run = Some(ConsoleRun {
            run_id: None,
            prompt: prompt.clone(),
            output: String::new(),
            status: ConsoleRunStatus::Starting,
            detail: "resolving provider and harness".to_owned(),
            input_tokens: 0,
            output_tokens: 0,
        });
        Ok(PromptRequest {
            agent_id,
            agent_name,
            prompt,
        })
    }

    fn prepare_edit(&mut self) {
        self.prompt_cursor = self.cursor();
        self.leave_history_navigation();
    }

    fn leave_history_navigation(&mut self) {
        self.prompt_history_index = None;
        self.prompt_history_draft = None;
    }

    fn record_history(&mut self, prompt: &str) {
        if self
            .prompt_history
            .last()
            .is_none_or(|entry| entry != prompt)
        {
            self.prompt_history.push(prompt.to_owned());
            let overflow = self.prompt_history.len().saturating_sub(MAX_PROMPT_HISTORY);
            if overflow > 0 {
                self.prompt_history.drain(..overflow);
            }
        }
    }

    fn history_previous(&mut self) {
        if self.prompt_history.is_empty() {
            return;
        }

        let index = if let Some(index) = self.prompt_history_index {
            index.saturating_sub(1)
        } else {
            self.prompt_history_draft = Some(self.prompt_buffer.clone());
            self.prompt_history.len() - 1
        };
        self.load_history(index);
    }

    fn history_next(&mut self) {
        let Some(index) = self.prompt_history_index else {
            return;
        };
        if index + 1 < self.prompt_history.len() {
            self.load_history(index + 1);
        } else {
            self.prompt_buffer = self.prompt_history_draft.take().unwrap_or_default();
            self.prompt_cursor = self.prompt_buffer.len();
            self.prompt_history_index = None;
        }
    }

    fn load_history(&mut self, index: usize) {
        self.prompt_buffer.clone_from(&self.prompt_history[index]);
        self.prompt_cursor = self.prompt_buffer.len();
        self.prompt_history_index = Some(index);
    }

    pub fn mark_cancelling(&mut self) {
        if let Some(run) = &mut self.run {
            if !run.status.is_terminal() {
                run.status = ConsoleRunStatus::Cancelling;
                "cancellation requested".clone_into(&mut run.detail);
            }
        }
    }

    pub fn apply(&mut self, event: ControllerEvent) {
        let Some(run) = &mut self.run else {
            return;
        };
        match event {
            ControllerEvent::Started {
                run_id,
                agent_name,
                notes,
            } => {
                run.run_id = Some(run_id);
                run.status = ConsoleRunStatus::Running;
                self.agent_name = Some(agent_name);
                run.detail = if notes.is_empty() {
                    "run started".to_owned()
                } else {
                    notes.join(" ")
                };
            }
            ControllerEvent::Output(text) => {
                run.output.push_str(&text);
                if run.output.len() > MAX_OUTPUT_BYTES {
                    let overflow = run.output.len() - MAX_OUTPUT_BYTES;
                    let boundary = run
                        .output
                        .char_indices()
                        .find_map(|(index, _)| (index >= overflow).then_some(index))
                        .unwrap_or(overflow);
                    run.output.drain(..boundary);
                }
            }
            ControllerEvent::Progress(detail) => run.detail = detail,
            ControllerEvent::Completed {
                input_tokens,
                output_tokens,
            } => {
                run.status = ConsoleRunStatus::Completed;
                "run completed".clone_into(&mut run.detail);
                run.input_tokens = input_tokens;
                run.output_tokens = output_tokens;
            }
            ControllerEvent::Failed(reason) => {
                run.status = ConsoleRunStatus::Failed;
                run.detail = reason;
            }
            ControllerEvent::Cancelled(reason) => {
                run.status = ConsoleRunStatus::Cancelled;
                run.detail = reason;
            }
            ControllerEvent::TimedOut => {
                run.status = ConsoleRunStatus::TimedOut;
                "run timed out".clone_into(&mut run.detail);
            }
        }
    }
}

fn clamp_char_boundary(text: &str, cursor: usize) -> usize {
    let mut cursor = cursor.min(text.len());
    while !text.is_char_boundary(cursor) {
        cursor -= 1;
    }
    cursor
}

fn previous_char_boundary(text: &str, cursor: usize) -> usize {
    let cursor = clamp_char_boundary(text, cursor);
    text[..cursor]
        .char_indices()
        .next_back()
        .map_or(cursor, |(index, _)| index)
}

fn next_char_boundary(text: &str, cursor: usize) -> usize {
    let cursor = clamp_char_boundary(text, cursor);
    text[cursor..]
        .chars()
        .next()
        .map_or(cursor, |c| cursor + c.len_utf8())
}

fn char_column_offset(line: &str, column: usize) -> usize {
    line.char_indices()
        .nth(column)
        .map_or(line.len(), |(index, _)| index)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControllerEvent {
    Started {
        run_id: String,
        agent_name: String,
        notes: Vec<String>,
    },
    Output(String),
    Progress(String),
    Completed {
        input_tokens: u64,
        output_tokens: u64,
    },
    Failed(String),
    Cancelled(String),
    TimedOut,
}

impl ControllerEvent {
    fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Completed { .. } | Self::Failed(_) | Self::Cancelled(_) | Self::TimedOut
        )
    }
}

/// Runtime bridge owned by `App`. It can be driven without blocking the
/// Crossterm event loop and exposes plain events for deterministic reduction.
pub struct RunController {
    runtime: Option<tokio::runtime::Handle>,
    pool: SqlitePool,
    config: Config,
    event_tx: mpsc::Sender<ControllerEvent>,
    event_rx: mpsc::Receiver<ControllerEvent>,
    cancel: Option<tokio::sync::oneshot::Sender<()>>,
    active: bool,
}

impl RunController {
    #[must_use]
    pub fn new(pool: SqlitePool, config: Config) -> Self {
        let (event_tx, event_rx) = mpsc::channel();
        Self {
            runtime: tokio::runtime::Handle::try_current().ok(),
            pool,
            config,
            event_tx,
            event_rx,
            cancel: None,
            active: false,
        }
    }

    pub fn start(&mut self, request: PromptRequest) -> Result<(), &'static str> {
        if self.active {
            return Err("a run is already active; cancel it before starting another");
        }
        let Some(runtime) = self.runtime.clone() else {
            return Err("interactive run runtime is unavailable");
        };

        let pool = self.pool.clone();
        let config = self.config.clone();
        let event_tx = self.event_tx.clone();
        let (cancel_tx, mut cancel_rx) = tokio::sync::oneshot::channel();
        self.cancel = Some(cancel_tx);
        self.active = true;

        runtime.spawn(async move {
            let start = crate::commands::run::start_interactive_run(
                &pool,
                &request.agent_id,
                &request.prompt,
                config,
            );
            tokio::pin!(start);

            let started = tokio::select! {
                result = &mut start => match result {
                    Ok(started) => started,
                    Err(error) => {
                        let _ = event_tx.send(ControllerEvent::Failed(format!("{error:#}")));
                        return;
                    }
                },
                _ = &mut cancel_rx => {
                    let _ = event_tx.send(ControllerEvent::Cancelled(
                        "cancelled before the run started".to_owned(),
                    ));
                    return;
                }
            };

            let run_id = started.run_id;
            let service = started.service;
            let mut events = started.events;
            let _ = event_tx.send(ControllerEvent::Started {
                run_id: run_id.to_string(),
                agent_name: started.agent_name,
                notes: started.notes,
            });

            let mut cancellation_requested = false;
            loop {
                let event = tokio::select! {
                    _ = &mut cancel_rx, if !cancellation_requested => {
                        cancellation_requested = true;
                        match service.cancel_run(run_id).await {
                            Ok(()) => {}
                            Err(error) => {
                                let _ = event_tx.send(ControllerEvent::Progress(format!(
                                    "cancelling run: {error}"
                                )));
                            }
                        }
                        continue;
                    }
                    result = events.recv() => match result {
                        Ok(event) => event,
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(count)) => {
                            let _ = event_tx.send(ControllerEvent::Progress(format!(
                                "live stream lagged; {count} event(s) skipped — durable timeline remains available"
                            )));
                            continue;
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            let _ = event_tx.send(ControllerEvent::Failed(
                                "run event stream closed before a terminal event".to_owned(),
                            ));
                            return;
                        }
                    }
                };

                if event.run_id != run_id {
                    continue;
                }
                let projected = match event.kind {
                    EventKind::StreamingToken { text } => Some(ControllerEvent::Output(text)),
                    EventKind::RunCreated => Some(ControllerEvent::Progress("run created".to_owned())),
                    EventKind::RunQueued => Some(ControllerEvent::Progress("run queued".to_owned())),
                    EventKind::RunStarted => Some(ControllerEvent::Progress("agent working".to_owned())),
                    EventKind::ProgressUpdate { message, percentage } => {
                        let suffix = percentage.map_or_else(String::new, |p| format!(" ({p:.0}%)"));
                        Some(ControllerEvent::Progress(format!("{message}{suffix}")))
                    }
                    EventKind::ToolCallStarted { tool_name } => Some(ControllerEvent::Progress(
                        format!("tool {tool_name} started"),
                    )),
                    EventKind::ToolCallCompleted { tool_name } => Some(ControllerEvent::Progress(
                        format!("tool {tool_name} completed"),
                    )),
                    EventKind::ApprovalRequested { request_id } => Some(ControllerEvent::Progress(
                        format!("approval required: {request_id}"),
                    )),
                    EventKind::RunCompleted {
                        input_tokens,
                        output_tokens,
                        ..
                    } => Some(ControllerEvent::Completed {
                        input_tokens,
                        output_tokens,
                    }),
                    EventKind::RunFailed { reason } => Some(ControllerEvent::Failed(reason)),
                    EventKind::RunCancelled { reason } => Some(ControllerEvent::Cancelled(reason)),
                    EventKind::RunTimedOut => Some(ControllerEvent::TimedOut),
                    _ => None,
                };

                if let Some(projected) = projected {
                    let terminal = projected.is_terminal();
                    if event_tx.send(projected).is_err() || terminal {
                        return;
                    }
                }
            }
        });

        Ok(())
    }

    pub fn cancel(&mut self) -> bool {
        self.cancel
            .take()
            .is_some_and(|cancel| cancel.send(()).is_ok())
    }

    pub fn try_recv(&mut self) -> Option<ControllerEvent> {
        match self.event_rx.try_recv() {
            Ok(event) => {
                if event.is_terminal() {
                    self.cancel = None;
                    self.active = false;
                }
                Some(event)
            }
            Err(mpsc::TryRecvError::Empty | mpsc::TryRecvError::Disconnected) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn type_prompt(state: &mut InteractionState, prompt: &str) {
        for c in prompt.chars() {
            state.push_char(c);
        }
    }

    #[test]
    fn reducer_rejects_empty_prompt_without_destroying_buffer() {
        let mut state = InteractionState::default();
        state.select_agent("agent-id", "Alice");
        state.prompt_buffer = "   ".to_owned();
        assert_eq!(state.submit(), Err("prompt cannot be empty"));
        assert_eq!(state.prompt_buffer, "   ");
        assert!(state.run.is_none());
    }

    #[test]
    fn reducer_tracks_start_output_and_completion() {
        let mut state = InteractionState::default();
        state.select_agent("agent-id", "Alice");
        for c in "Summarize treasury activity".chars() {
            state.push_char(c);
        }
        let request = state.submit().expect("valid prompt");
        assert_eq!(request.agent_id, "agent-id");
        assert_eq!(request.prompt, "Summarize treasury activity");
        assert_eq!(
            state.run.as_ref().unwrap().status,
            ConsoleRunStatus::Starting
        );

        state.apply(ControllerEvent::Started {
            run_id: "run-id".to_owned(),
            agent_name: "Alice".to_owned(),
            notes: vec!["simulated provider".to_owned()],
        });
        state.apply(ControllerEvent::Output("hello ".to_owned()));
        state.apply(ControllerEvent::Output("world".to_owned()));
        state.apply(ControllerEvent::Completed {
            input_tokens: 9,
            output_tokens: 2,
        });

        let run = state.run.as_ref().unwrap();
        assert_eq!(run.run_id.as_deref(), Some("run-id"));
        assert_eq!(run.output, "hello world");
        assert_eq!(run.status, ConsoleRunStatus::Completed);
        assert_eq!((run.input_tokens, run.output_tokens), (9, 2));
    }

    #[test]
    fn reducer_projects_failure_and_cancellation() {
        let mut state = InteractionState::default();
        state.select_agent("agent-id", "Alice");
        state.prompt_buffer = "go".to_owned();
        state.submit().unwrap();
        state.mark_cancelling();
        assert_eq!(
            state.run.as_ref().unwrap().status,
            ConsoleRunStatus::Cancelling
        );
        state.apply(ControllerEvent::Cancelled("operator cancelled".to_owned()));
        assert_eq!(
            state.run.as_ref().unwrap().status,
            ConsoleRunStatus::Cancelled
        );
        assert_eq!(state.run.as_ref().unwrap().detail, "operator cancelled");
    }

    #[test]
    fn prompt_editor_inserts_and_deletes_on_utf8_boundaries() {
        let mut state = InteractionState::default();
        type_prompt(&mut state, "a🙂界");
        assert_eq!(state.cursor(), state.prompt_buffer.len());

        state.move_left();
        state.push_char('é');
        assert_eq!(state.prompt_buffer, "a🙂é界");
        assert!(state.prompt_buffer.is_char_boundary(state.cursor()));

        state.backspace();
        assert_eq!(state.prompt_buffer, "a🙂界");
        assert!(state.prompt_buffer.is_char_boundary(state.cursor()));

        state.delete();
        assert_eq!(state.prompt_buffer, "a🙂");
        state.move_left();
        state.delete();
        assert_eq!(state.prompt_buffer, "a");
        assert!(state.prompt_buffer.is_char_boundary(state.cursor()));
    }

    #[test]
    fn prompt_editor_moves_across_multiline_unicode_input() {
        let mut state = InteractionState::default();
        state.select_agent("agent-id", "Alice");
        type_prompt(&mut state, "ab\n世🙂\nz");

        state.move_up();
        assert_eq!(&state.prompt_buffer[..state.cursor()], "ab\n世");
        state.move_up();
        assert_eq!(&state.prompt_buffer[..state.cursor()], "a");
        state.move_down();
        assert_eq!(&state.prompt_buffer[..state.cursor()], "ab\n世");

        state.move_end();
        assert_eq!(&state.prompt_buffer[..state.cursor()], "ab\n世🙂");
        state.move_home();
        assert_eq!(&state.prompt_buffer[..state.cursor()], "ab\n");

        let request = state.submit().expect("valid multiline prompt");
        assert_eq!(request.prompt, "ab\n世🙂\nz");
    }

    #[test]
    fn prompt_history_restores_the_unsent_draft() {
        let mut state = InteractionState::default();
        state.select_agent("agent-id", "Alice");

        type_prompt(&mut state, "first prompt");
        state.submit().expect("first prompt");
        type_prompt(&mut state, "second prompt");
        state.submit().expect("second prompt");
        type_prompt(&mut state, "draft");

        state.move_up();
        assert_eq!(state.prompt_buffer, "second prompt");
        state.move_up();
        assert_eq!(state.prompt_buffer, "first prompt");
        state.move_down();
        assert_eq!(state.prompt_buffer, "second prompt");
        state.move_down();
        assert_eq!(state.prompt_buffer, "draft");
        assert_eq!(state.cursor(), state.prompt_buffer.len());
    }

    #[test]
    fn prompt_history_is_bounded_and_deduplicates_consecutive_entries() {
        let mut state = InteractionState::default();
        state.select_agent("agent-id", "Alice");

        for index in 0..=MAX_PROMPT_HISTORY {
            type_prompt(&mut state, &format!("prompt {index}"));
            state.submit().expect("valid prompt");
        }
        type_prompt(&mut state, "prompt 100");
        state.submit().expect("duplicate prompt remains valid");

        assert_eq!(state.prompt_history.len(), MAX_PROMPT_HISTORY);
        assert_eq!(
            state.prompt_history.first().map(String::as_str),
            Some("prompt 1")
        );
        assert_eq!(
            state.prompt_history.last().map(String::as_str),
            Some("prompt 100")
        );
    }
}
