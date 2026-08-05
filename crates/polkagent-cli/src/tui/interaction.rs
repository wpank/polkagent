//! Headless interaction state and asynchronous run controller for the TUI.
//!
//! Terminal input/rendering stays in `input` and `views::console`; this module
//! owns the deterministic reducer and the bridge to the same process-wide
//! production runtime used by `polkagent run`.

use std::path::Path;
use std::sync::{mpsc, Arc, OnceLock};

use async_trait::async_trait;
use polkagent_core::{AgentId, ApprovalId, ConversationId, RunId};
use polkagent_interaction::{
    AgentTargetView, ClientContext, CommandContext, CommandExecutor, CommandInvocation,
    CommandName, CommandOutput, CommandRegistry, CommandRequest, CreateInteractionRequest,
    InteractionCommand, InteractionCommandRuntime, InteractionConfig, InteractionContent,
    InteractionError, InteractionErrorCode, InteractionEvent, InteractionOverrides,
    InteractionService, InteractionState as DurableInteractionState, InteractionSummary,
    InteractionTarget, InteractionTurnId, ListInteractionsRequest, ParsedLine,
    PromptRequest as InteractionPromptRequest, RunDetailView, RunSummaryView,
    ServiceCommandExecutor, StreamError, SubscriptionRequest, TranscriptRequest, TurnHandle,
    TurnState,
};
use polkagent_runtime::{
    AdapterPolicy, ComponentState, PolkagentRuntime, RuntimeOptions, RuntimeReadiness, WarningCode,
};
use polkagent_store_sqlite::SqlitePool;

/// Keep a runaway streaming response from growing the terminal process
/// forever. Durable lifecycle/events remain available through the run views.
const MAX_OUTPUT_BYTES: usize = 128 * 1024;
/// Bound composer recall even when a durable interaction has a longer transcript.
const MAX_PROMPT_HISTORY: usize = 100;
const INTERACTION_STREAM_CAPACITY: usize = 256;
const TUI_INTERACTION_TITLE_PREFIX: &str = "TUI Console";
const SUPPORTED_CONSOLE_COMMANDS: [CommandName; 4] = [
    CommandName::Help,
    CommandName::Status,
    CommandName::New,
    CommandName::Resume,
];

/// One canonical entry in the Console's shared slash-command picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlashCommandCandidate {
    /// Canonical command name without the leading slash.
    pub name: String,
    /// Alternative spellings accepted by the shared parser.
    pub aliases: Vec<String>,
    /// Human-readable command purpose from the shared registry.
    pub description: String,
    /// Optional argument syntax from the shared registry.
    pub input_hint: Option<String>,
}

impl SlashCommandCandidate {
    /// Render the canonical command and its argument hint.
    #[must_use]
    pub fn usage(&self) -> String {
        self.input_hint.as_ref().map_or_else(
            || format!("/{}", self.name),
            |hint| format!("/{} {hint}", self.name),
        )
    }
}

/// Current registry-derived slash-command picker projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlashCommandMenu {
    /// Canonical candidates matching the typed name or alias prefix.
    pub candidates: Vec<SlashCommandCandidate>,
    /// Highlighted candidate, always clamped to `candidates`.
    pub selected: usize,
}

impl SlashCommandMenu {
    /// Return the highlighted candidate.
    #[must_use]
    pub fn selected_candidate(&self) -> Option<&SlashCommandCandidate> {
        self.candidates.get(self.selected)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptRequest {
    pub agent_id: String,
    pub agent_name: String,
    pub conversation_id: Option<String>,
    pub prompt: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsoleCommandRequest {
    pub request_id: String,
    pub agent_id: String,
    pub agent_name: String,
    pub conversation_id: Option<String>,
    pub line: String,
    pub invocation: CommandInvocation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsoleCommandStatus {
    Running,
    Completed,
    Failed,
}

impl ConsoleCommandStatus {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsoleCommandResult {
    pub request_id: String,
    pub line: String,
    pub status: ConsoleCommandStatus,
    pub title: String,
    pub lines: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConsoleCommandSubmission {
    Execute(ConsoleCommandRequest),
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsoleConversationSelection {
    pub conversation_id: String,
    pub turns: Vec<ConsoleRun>,
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
    pub conversation_id: Option<String>,
    pub turn_id: Option<String>,
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
    slash_completion_selection: usize,
    slash_completion_dismissed: bool,
    pub conversation_id: Option<String>,
    pub transcript: Vec<ConsoleRun>,
    pub run: Option<ConsoleRun>,
    pub command_result: Option<ConsoleCommandResult>,
}

impl InteractionState {
    pub fn select_agent(&mut self, agent_id: impl Into<String>, agent_name: impl Into<String>) {
        let agent_id = agent_id.into();
        if self.agent_id.as_deref() != Some(agent_id.as_str()) {
            self.conversation_id = None;
            self.transcript.clear();
            self.run = None;
            self.prompt_history.clear();
            self.command_result = None;
        }
        self.agent_id = Some(agent_id);
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
        if self.move_slash_completion_up() {
            return;
        }
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
        if self.move_slash_completion_down() {
            return;
        }
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
        self.slash_completion_selection = 0;
        self.slash_completion_dismissed = false;
    }

    /// Return shared slash-command candidates for the current composer text.
    #[must_use]
    pub fn slash_command_menu(&self) -> Option<SlashCommandMenu> {
        if self.slash_completion_dismissed {
            return None;
        }
        let command_line = self.prompt_buffer.trim_start().strip_prefix('/')?;
        let (typed_name, has_arguments) = command_line
            .find(char::is_whitespace)
            .map_or((command_line, false), |index| {
                (&command_line[..index], true)
            });
        let typed_name = typed_name.to_ascii_lowercase();
        let registry = command_registry();

        let candidates = if has_arguments || registry.resolve(&typed_name).is_some() {
            registry
                .resolve(&typed_name)
                .filter(|spec| SUPPORTED_CONSOLE_COMMANDS.contains(&spec.command))
                .into_iter()
                .map(slash_candidate)
                .collect()
        } else {
            registry
                .specs()
                .into_iter()
                .filter(|spec| SUPPORTED_CONSOLE_COMMANDS.contains(&spec.command))
                .filter(|spec| {
                    spec.name.starts_with(&typed_name)
                        || spec
                            .aliases
                            .iter()
                            .any(|alias| alias.starts_with(&typed_name))
                })
                .map(slash_candidate)
                .collect::<Vec<_>>()
        };
        if candidates.is_empty() {
            return None;
        }
        let selected = self
            .slash_completion_selection
            .min(candidates.len().saturating_sub(1));
        Some(SlashCommandMenu {
            candidates,
            selected,
        })
    }

    /// Replace the typed slash name with the highlighted canonical command.
    ///
    /// Returns whether a completion was available and accepted.
    pub fn accept_slash_completion(&mut self) -> bool {
        let Some(candidate) = self
            .slash_command_menu()
            .and_then(|menu| menu.selected_candidate().cloned())
        else {
            return false;
        };
        let leading_whitespace = self.prompt_buffer.len() - self.prompt_buffer.trim_start().len();
        let command_start = leading_whitespace + 1;
        let command_end = self.prompt_buffer[command_start..]
            .find(char::is_whitespace)
            .map_or(self.prompt_buffer.len(), |offset| command_start + offset);
        self.prompt_buffer
            .replace_range(command_start..command_end, &candidate.name);
        self.prompt_cursor = command_start + candidate.name.len();
        if candidate.input_hint.is_some()
            && self
                .prompt_buffer
                .as_bytes()
                .get(self.prompt_cursor)
                .is_none_or(u8::is_ascii_whitespace)
        {
            if self.prompt_buffer.as_bytes().get(self.prompt_cursor) != Some(&b' ') {
                self.prompt_buffer.insert(self.prompt_cursor, ' ');
            }
            self.prompt_cursor += 1;
        }
        self.slash_completion_selection = 0;
        self.slash_completion_dismissed = false;
        self.leave_history_navigation();
        true
    }

    /// Dismiss the visible slash-command picker.
    ///
    /// Returns `false` when no picker was visible, allowing Escape to fall
    /// through to the composer's existing cancel behavior.
    pub fn dismiss_slash_completion(&mut self) -> bool {
        if self.slash_command_menu().is_none() {
            return false;
        }
        self.slash_completion_dismissed = true;
        true
    }

    pub fn submit(&mut self) -> Result<PromptRequest, &'static str> {
        let prompt = self.prompt_buffer.trim().to_owned();
        if prompt.is_empty() {
            return Err("prompt cannot be empty");
        }
        if prompt.starts_with('/') {
            return Err("slash commands must use the Console command path");
        }
        let Some(agent_id) = self.agent_id.clone() else {
            return Err("select an active agent before prompting");
        };
        let agent_name = self.agent_name.clone().unwrap_or_else(|| agent_id.clone());
        self.record_history(&prompt);
        self.clear_prompt();
        self.command_result = None;
        if self
            .run
            .as_ref()
            .is_some_and(|run| run.status.is_terminal())
        {
            if let Some(previous) = self.run.take() {
                self.transcript.push(previous);
            }
        }
        self.run = Some(ConsoleRun {
            conversation_id: self.conversation_id.clone(),
            turn_id: None,
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
            conversation_id: self.conversation_id.clone(),
            prompt,
        })
    }

    #[must_use]
    pub fn is_command_input(&self) -> bool {
        self.prompt_buffer.trim_start().starts_with('/')
    }

    pub fn submit_command(&mut self) -> Result<ConsoleCommandSubmission, &'static str> {
        let line = self.prompt_buffer.trim().to_owned();
        if line.is_empty() {
            return Err("command cannot be empty");
        }
        if !line.starts_with('/') {
            return Err("Console command input must begin with '/'");
        }
        let Some(agent_id) = self.agent_id.clone() else {
            return Err("select an active agent before using Console commands");
        };
        let agent_name = self.agent_name.clone().unwrap_or_else(|| agent_id.clone());
        self.record_history(&line);
        self.clear_prompt();

        let invocation = match command_registry().parse(&line) {
            Ok(ParsedLine::Command(invocation)) => invocation,
            Ok(ParsedLine::Prompt(_)) => unreachable!("slash input parsed as a prompt"),
            Err(error) => {
                self.reject_command(&line, console_parse_error(&line, &error.to_string()));
                return Ok(ConsoleCommandSubmission::Rejected);
            }
        };
        if let Err(reason) = validate_console_command(&invocation.command) {
            self.reject_command(&line, reason);
            return Ok(ConsoleCommandSubmission::Rejected);
        }

        let request_id = uuid::Uuid::now_v7().to_string();
        self.command_result = Some(ConsoleCommandResult {
            request_id: request_id.clone(),
            line: line.clone(),
            status: ConsoleCommandStatus::Running,
            title: "Executing shared command".to_owned(),
            lines: vec!["Waiting for the durable interaction service.".to_owned()],
        });
        Ok(ConsoleCommandSubmission::Execute(ConsoleCommandRequest {
            request_id,
            agent_id,
            agent_name,
            conversation_id: self.conversation_id.clone(),
            line,
            invocation,
        }))
    }

    fn reject_command(&mut self, line: &str, reason: String) {
        self.command_result = Some(ConsoleCommandResult {
            request_id: uuid::Uuid::now_v7().to_string(),
            line: line.to_owned(),
            status: ConsoleCommandStatus::Failed,
            title: "Command unavailable".to_owned(),
            lines: vec![reason],
        });
    }

    pub fn fail_pending_command(&mut self, reason: impl Into<String>) {
        if let Some(result) = &mut self.command_result {
            if result.status == ConsoleCommandStatus::Running {
                result.status = ConsoleCommandStatus::Failed;
                "Command failed".clone_into(&mut result.title);
                result.lines = vec![reason.into()];
            }
        }
    }

    fn prepare_edit(&mut self) {
        self.prompt_cursor = self.cursor();
        self.leave_history_navigation();
        self.slash_completion_selection = 0;
        self.slash_completion_dismissed = false;
    }

    fn move_slash_completion_up(&mut self) -> bool {
        let Some(menu) = self.slash_command_menu() else {
            return false;
        };
        if menu.candidates.len() <= 1 {
            return true;
        }
        self.slash_completion_selection = if menu.selected == 0 {
            menu.candidates.len() - 1
        } else {
            menu.selected - 1
        };
        true
    }

    fn move_slash_completion_down(&mut self) -> bool {
        let Some(menu) = self.slash_command_menu() else {
            return false;
        };
        if menu.candidates.len() <= 1 {
            return true;
        }
        self.slash_completion_selection = (menu.selected + 1) % menu.candidates.len();
        true
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
        match event {
            ControllerEvent::HistoryLoaded {
                agent_id,
                conversation_id,
                turns,
            } => {
                if self.agent_id.as_deref() != Some(agent_id.as_str())
                    || self.run.is_some()
                    || self.command_result.is_some()
                {
                    return;
                }
                self.replace_conversation(conversation_id, turns);
            }
            ControllerEvent::HistoryFailed { .. } => {}
            ControllerEvent::CommandCompleted {
                agent_id,
                request_id,
                result,
                selection,
            } => {
                if self.agent_id.as_deref() != Some(agent_id.as_str())
                    || self
                        .command_result
                        .as_ref()
                        .map(|result| result.request_id.as_str())
                        != Some(request_id.as_str())
                {
                    return;
                }
                self.command_result = Some(result);
                if let Some(selection) = selection {
                    self.replace_conversation(Some(selection.conversation_id), selection.turns);
                }
            }
            ControllerEvent::CommandFailed {
                agent_id,
                request_id,
                result,
            } => {
                if self.agent_id.as_deref() == Some(agent_id.as_str())
                    && self
                        .command_result
                        .as_ref()
                        .map(|result| result.request_id.as_str())
                        == Some(request_id.as_str())
                {
                    self.command_result = Some(result);
                }
            }
            event => self.apply_run_event(event),
        }
    }

    fn replace_conversation(
        &mut self,
        conversation_id: Option<String>,
        mut turns: Vec<ConsoleRun>,
    ) {
        self.conversation_id = conversation_id;
        self.prompt_history = turns
            .iter()
            .map(|turn| turn.prompt.clone())
            .rev()
            .take(MAX_PROMPT_HISTORY)
            .collect::<Vec<_>>();
        self.prompt_history.reverse();
        self.run = turns.pop();
        self.transcript = turns;
    }

    fn apply_run_event(&mut self, event: ControllerEvent) {
        let Some(run) = &mut self.run else {
            return;
        };
        match event {
            ControllerEvent::Started {
                conversation_id,
                turn_id,
                run_id,
                agent_name,
                notes,
            } => {
                run.conversation_id = Some(conversation_id.clone());
                run.turn_id = Some(turn_id);
                run.run_id = Some(run_id);
                self.conversation_id = Some(conversation_id);
                run.status = ConsoleRunStatus::Running;
                self.agent_name = Some(agent_name);
                run.detail = if notes.is_empty() {
                    "run started".to_owned()
                } else {
                    notes.join(" ")
                };
            }
            ControllerEvent::Output(text) => {
                append_bounded_output(&mut run.output, &text);
            }
            ControllerEvent::Progress(detail) => run.detail = detail,
            ControllerEvent::UsageUpdated {
                input_tokens,
                output_tokens,
            } => {
                run.input_tokens = input_tokens;
                run.output_tokens = output_tokens;
            }
            ControllerEvent::Completed {
                text,
                input_tokens,
                output_tokens,
            } => {
                run.status = ConsoleRunStatus::Completed;
                "run completed".clone_into(&mut run.detail);
                replace_bounded_output(&mut run.output, &text);
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
            ControllerEvent::HistoryLoaded { .. }
            | ControllerEvent::HistoryFailed { .. }
            | ControllerEvent::CommandCompleted { .. }
            | ControllerEvent::CommandFailed { .. } => {}
        }
    }
}

fn append_bounded_output(output: &mut String, text: &str) {
    output.push_str(text);
    truncate_output(output);
}

fn truncate_output(output: &mut String) {
    if output.len() <= MAX_OUTPUT_BYTES {
        return;
    }
    let overflow = output.len() - MAX_OUTPUT_BYTES;
    let boundary = output
        .char_indices()
        .find_map(|(index, _)| (index >= overflow).then_some(index))
        .unwrap_or(overflow);
    output.drain(..boundary);
}

fn replace_bounded_output(output: &mut String, text: &str) {
    output.clear();
    append_bounded_output(output, text);
}

fn command_registry() -> &'static CommandRegistry {
    static REGISTRY: OnceLock<CommandRegistry> = OnceLock::new();
    REGISTRY.get_or_init(CommandRegistry::mvp)
}

fn validate_console_command(command: &InteractionCommand) -> Result<(), String> {
    match command {
        InteractionCommand::Help {
            command: Some(command),
        } if !SUPPORTED_CONSOLE_COMMANDS.contains(command) => Err(format!(
            "/{} is not supported in the Console",
            command.as_str()
        )),
        InteractionCommand::Agents | InteractionCommand::Agent { .. } => Err(
            "agent selection commands are unavailable in the Console; choose an active agent in F2"
                .to_owned(),
        ),
        InteractionCommand::Runs | InteractionCommand::Inspect { .. } => Err(
            "run inspection commands are unavailable in the Console; use F3 Runs and F5 Timeline"
                .to_owned(),
        ),
        InteractionCommand::Cancel { .. } => Err(
            "/cancel is unavailable because the composer is closed during an active turn; press x for exact current-turn cancellation"
                .to_owned(),
        ),
        InteractionCommand::Approve { .. } | InteractionCommand::Deny { .. } => Err(
            "approval commands are unavailable in the Console prompt path; use an authorized approval surface"
                .to_owned(),
        ),
        InteractionCommand::Model { .. } => Err(
            "model selection is unavailable in the Console; configure the selected agent outside this surface"
                .to_owned(),
        ),
        _ => Ok(()),
    }
}

fn console_parse_error(line: &str, shared_error: &str) -> String {
    let name = line
        .trim_start_matches('/')
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    match name.as_str() {
        "group" | "groups" => {
            "group orchestration is unavailable in the Console; select one active agent in F2"
                .to_owned()
        }
        "provider" => {
            "provider selection is unavailable in the Console; configure the runtime and restart"
                .to_owned()
        }
        "autonomy" => {
            "autonomy changes are unavailable in the Console; configure the agent outside this surface"
                .to_owned()
        }
        _ => shared_error.to_owned(),
    }
}

fn slash_candidate(spec: &polkagent_interaction::CommandSpec) -> SlashCommandCandidate {
    SlashCommandCandidate {
        name: spec.name.clone(),
        aliases: spec.aliases.clone(),
        description: spec.description.clone(),
        input_hint: spec.input_hint.clone(),
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
    HistoryLoaded {
        agent_id: String,
        conversation_id: Option<String>,
        turns: Vec<ConsoleRun>,
    },
    HistoryFailed {
        agent_id: String,
        reason: String,
    },
    CommandCompleted {
        agent_id: String,
        request_id: String,
        result: ConsoleCommandResult,
        selection: Option<ConsoleConversationSelection>,
    },
    CommandFailed {
        agent_id: String,
        request_id: String,
        result: ConsoleCommandResult,
    },
    Started {
        conversation_id: String,
        turn_id: String,
        run_id: String,
        agent_name: String,
        notes: Vec<String>,
    },
    Output(String),
    Progress(String),
    UsageUpdated {
        input_tokens: u64,
        output_tokens: u64,
    },
    Completed {
        text: String,
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
            Self::Completed { .. }
                | Self::Failed(_)
                | Self::Cancelled(_)
                | Self::TimedOut
                | Self::CommandCompleted { .. }
                | Self::CommandFailed { .. }
        )
    }
}

/// Runtime bridge owned by `App`. It can be driven without blocking the
/// Crossterm event loop and exposes plain events for deterministic reduction.
pub struct RunController {
    task_runtime: Option<tokio::runtime::Handle>,
    polkagent_runtime: PolkagentRuntime,
    event_tx: mpsc::Sender<ControllerEvent>,
    event_rx: mpsc::Receiver<ControllerEvent>,
    cancel: Option<tokio::sync::oneshot::Sender<()>>,
    active: bool,
}

impl RunController {
    #[must_use]
    pub fn new(polkagent_runtime: PolkagentRuntime) -> Self {
        let (event_tx, event_rx) = mpsc::channel();
        Self {
            task_runtime: tokio::runtime::Handle::try_current().ok(),
            polkagent_runtime,
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
        let Some(task_runtime) = self.task_runtime.clone() else {
            return Err("interactive run runtime is unavailable");
        };

        let polkagent_runtime = self.polkagent_runtime.clone();
        let event_tx = self.event_tx.clone();
        let (cancel_tx, mut cancel_rx) = tokio::sync::oneshot::channel();
        self.cancel = Some(cancel_tx);
        self.active = true;

        task_runtime.spawn(async move {
            let PromptRequest {
                agent_id,
                agent_name,
                conversation_id,
                prompt,
            } = request;
            let typed_agent_id = match parse_selected_agent_id(&agent_id) {
                Ok(agent_id) => agent_id,
                Err(reason) => {
                    let _ = event_tx.send(ControllerEvent::Failed(reason));
                    return;
                }
            };
            let service = Arc::clone(polkagent_runtime.interactions());
            let interaction = match resolve_prompt_interaction(
                service.as_ref(),
                &polkagent_runtime,
                typed_agent_id,
                &agent_name,
                conversation_id.as_deref(),
            )
            .await {
                Ok(interaction) => interaction,
                Err(error) => {
                    let _ = event_tx.send(ControllerEvent::Failed(error.to_string()));
                    return;
                }
            };
            let client_context = match tui_client_context(&polkagent_runtime) {
                Ok(context) => context,
                Err(error) => {
                    let _ = event_tx.send(ControllerEvent::Failed(error.to_string()));
                    return;
                }
            };
            let turn_id = InteractionTurnId::new();
            let prompt = service.prompt(InteractionPromptRequest {
                turn_id: Some(turn_id),
                conversation_id: interaction.conversation_id,
                content: vec![InteractionContent::Text {
                    text: prompt,
                }],
                config_overrides: InteractionOverrides::default(),
                client_context,
            });
            tokio::pin!(prompt);
            let mut cancellation_requested = false;
            let started = loop {
                tokio::select! {
                    biased;
                    _ = &mut cancel_rx, if !cancellation_requested => {
                        cancellation_requested = true;
                        let _ = event_tx.send(ControllerEvent::Progress(
                            "cancellation queued until the durable turn is ready".to_owned(),
                        ));
                    }
                    result = &mut prompt => match result {
                        Ok(started) => break started,
                        Err(error) => {
                            let _ = event_tx.send(ControllerEvent::Failed(error.to_string()));
                            return;
                        }
                    }
                }
            };
            let Some(run_id) = started.handle.run_ids.first().copied() else {
                let _ = event_tx.send(ControllerEvent::Failed(
                    "durable interaction turn has no linked run".to_owned(),
                ));
                return;
            };
            let mut events = started.events;
            let notes = runtime_notes(polkagent_runtime.readiness());

            let _ = event_tx.send(ControllerEvent::Started {
                conversation_id: interaction.conversation_id.to_string(),
                turn_id: turn_id.to_string(),
                run_id: run_id.to_string(),
                agent_name,
                notes,
            });

            if cancellation_requested {
                request_turn_cancellation(service.as_ref(), turn_id, &event_tx).await;
            }
            loop {
                let envelope = tokio::select! {
                    biased;
                    _ = &mut cancel_rx, if !cancellation_requested => {
                        cancellation_requested = true;
                        request_turn_cancellation(service.as_ref(), turn_id, &event_tx).await;
                        continue;
                    }
                    result = events.recv() => match result {
                        Ok(event) => event,
                        Err(StreamError::Lagged { last_seen_sequence, resume_after_sequence }) => {
                            let _ = event_tx.send(ControllerEvent::Progress(format!(
                                "interaction stream lagged at sequence {resume_after_sequence}; replaying durable events"
                            )));
                            match service
                                .subscribe(SubscriptionRequest {
                                    conversation_id: interaction.conversation_id,
                                    turn_id: Some(turn_id),
                                    after_sequence: last_seen_sequence,
                                    capacity: INTERACTION_STREAM_CAPACITY,
                                })
                                .await
                            {
                                Ok(replacement) => events = replacement,
                                Err(error) => {
                                    let _ = event_tx.send(ControllerEvent::Failed(format!(
                                        "resubscribe to durable interaction events: {error}"
                                    )));
                                    return;
                                }
                            }
                            continue;
                        }
                        Err(StreamError::Backend(error)) => {
                            let _ = event_tx.send(ControllerEvent::Failed(format!(
                                "interaction event stream failed: {error}"
                            )));
                            return;
                        }
                        Err(StreamError::Closed) => {
                            let _ = event_tx.send(ControllerEvent::Failed(
                                "interaction event stream closed before a terminal event".to_owned(),
                            ));
                            return;
                        }
                    }
                };

                let projected = project_interaction_event(envelope.event);
                let terminal = projected.is_terminal();
                if event_tx.send(projected).is_err() || terminal {
                    return;
                }
            }
        });

        Ok(())
    }

    pub fn execute_command(&mut self, request: ConsoleCommandRequest) -> Result<(), &'static str> {
        if self.active {
            return Err("a Console action is already active");
        }
        let Some(task_runtime) = self.task_runtime.clone() else {
            return Err("interactive command runtime is unavailable");
        };
        self.active = true;
        self.cancel = None;
        let polkagent_runtime = self.polkagent_runtime.clone();
        let event_tx = self.event_tx.clone();
        task_runtime.spawn(async move {
            let ConsoleCommandRequest {
                request_id,
                agent_id,
                agent_name,
                conversation_id,
                line,
                invocation,
            } = request;
            let typed_agent_id = match agent_id.parse::<AgentId>() {
                Ok(agent_id) => agent_id,
                Err(error) => {
                    send_command_failure(
                        &event_tx,
                        agent_id,
                        request_id,
                        line,
                        format!("invalid selected agent ID: {error}"),
                    );
                    return;
                }
            };
            let conversation_id = match conversation_id
                .map(|id| {
                    id.parse::<ConversationId>().map_err(|error| {
                        format!("invalid selected Console conversation ID '{id}': {error}")
                    })
                })
                .transpose()
            {
                Ok(conversation_id) => conversation_id,
                Err(error) => {
                    send_command_failure(&event_tx, agent_id, request_id, line, error);
                    return;
                }
            };
            let service: Arc<dyn InteractionService> = polkagent_runtime.interactions().clone();
            let command_runtime: Arc<dyn InteractionCommandRuntime> = Arc::new(TuiCommandRuntime {
                service: Arc::clone(&service),
                agent_id: typed_agent_id,
            });
            let executor = match ServiceCommandExecutor::new(
                command_registry().clone(),
                service,
                command_runtime,
            ) {
                Ok(executor) => executor,
                Err(error) => {
                    send_command_failure(&event_tx, agent_id, request_id, line, error.to_string());
                    return;
                }
            };
            let client_context = match tui_client_context(&polkagent_runtime) {
                Ok(context) => context,
                Err(error) => {
                    send_command_failure(&event_tx, agent_id, request_id, line, error.to_string());
                    return;
                }
            };
            let output = executor
                .execute(CommandRequest {
                    invocation,
                    context: CommandContext {
                        conversation_id,
                        has_active_turn: false,
                        pending_approval_count: 0,
                        can_mutate: true,
                    },
                    client_context,
                })
                .await;
            let outcome = match output {
                Ok(output) => {
                    project_console_command_output(
                        &polkagent_runtime,
                        typed_agent_id,
                        &agent_name,
                        output,
                    )
                    .await
                }
                Err(error) => Err(error.to_string()),
            };
            match outcome {
                Ok(outcome) => {
                    let result = ConsoleCommandResult {
                        request_id: request_id.clone(),
                        line,
                        status: ConsoleCommandStatus::Completed,
                        title: outcome.title,
                        lines: outcome.lines,
                    };
                    let _ = event_tx.send(ControllerEvent::CommandCompleted {
                        agent_id,
                        request_id,
                        result,
                        selection: outcome.selection,
                    });
                }
                Err(error) => {
                    send_command_failure(&event_tx, agent_id, request_id, line, error);
                }
            }
        });
        Ok(())
    }

    /// Load the durable TUI interaction for a selected agent without blocking
    /// input or rendering. A stale result is ignored by the reducer.
    pub fn load_history(&self, agent_id: String) -> Result<(), &'static str> {
        let Some(task_runtime) = self.task_runtime.clone() else {
            return Err("interactive run runtime is unavailable");
        };
        let polkagent_runtime = self.polkagent_runtime.clone();
        let event_tx = self.event_tx.clone();
        task_runtime.spawn(async move {
            let typed_agent_id = match agent_id.parse::<AgentId>() {
                Ok(agent_id) => agent_id,
                Err(error) => {
                    let _ = event_tx.send(ControllerEvent::HistoryFailed {
                        agent_id,
                        reason: format!("invalid selected agent ID: {error}"),
                    });
                    return;
                }
            };
            match load_console_history(&polkagent_runtime, typed_agent_id).await {
                Ok((conversation_id, turns)) => {
                    let _ = event_tx.send(ControllerEvent::HistoryLoaded {
                        agent_id,
                        conversation_id: conversation_id.map(|id| id.to_string()),
                        turns,
                    });
                }
                Err(error) => {
                    let _ = event_tx.send(ControllerEvent::HistoryFailed {
                        agent_id,
                        reason: error,
                    });
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

    #[must_use]
    pub const fn is_active(&self) -> bool {
        self.active
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

fn tui_interaction_title(agent_name: &str) -> String {
    format!("{TUI_INTERACTION_TITLE_PREFIX} · {agent_name}")
}

fn parse_selected_agent_id(agent_id: &str) -> Result<AgentId, String> {
    agent_id
        .parse::<AgentId>()
        .map_err(|error| format!("invalid selected agent ID '{agent_id}': {error}"))
}

fn tui_client_context(
    runtime: &PolkagentRuntime,
) -> Result<ClientContext, polkagent_interaction::InteractionError> {
    let mut context = ClientContext::new(runtime.workdir().to_path_buf())?;
    context.client_name = Some("tui".to_owned());
    Ok(context)
}

fn send_command_failure(
    event_tx: &mpsc::Sender<ControllerEvent>,
    agent_id: String,
    request_id: String,
    line: String,
    error: String,
) {
    let result = ConsoleCommandResult {
        request_id: request_id.clone(),
        line,
        status: ConsoleCommandStatus::Failed,
        title: "Command failed".to_owned(),
        lines: vec![error],
    };
    let _ = event_tx.send(ControllerEvent::CommandFailed {
        agent_id,
        request_id,
        result,
    });
}

struct ProjectedConsoleCommand {
    title: String,
    lines: Vec<String>,
    selection: Option<ConsoleConversationSelection>,
}

async fn project_console_command_output(
    runtime: &PolkagentRuntime,
    agent_id: AgentId,
    agent_name: &str,
    output: CommandOutput,
) -> Result<ProjectedConsoleCommand, String> {
    match output {
        CommandOutput::Help { commands } => {
            let mut commands = commands
                .into_iter()
                .filter(|spec| SUPPORTED_CONSOLE_COMMANDS.contains(&spec.command))
                .collect::<Vec<_>>();
            commands.sort_by_key(|spec| spec.command);
            let mut lines = commands
                .into_iter()
                .map(|spec| {
                    let hint = spec
                        .input_hint
                        .as_deref()
                        .map_or(String::new(), |hint| format!(" {hint}"));
                    format!("/{}{hint} — {}", spec.name, spec.description)
                })
                .collect::<Vec<_>>();
            lines.push("x — cancel the exact current turn (not a slash command)".to_owned());
            lines.push(
                "Agent/model/provider/autonomy, approval, group, and run-inspection commands are unavailable in Console."
                    .to_owned(),
            );
            Ok(ProjectedConsoleCommand {
                title: "Supported Console commands".to_owned(),
                lines,
                selection: None,
            })
        }
        CommandOutput::Status {
            interaction,
            active_turns,
            pending_approvals: _,
        } => Ok(ProjectedConsoleCommand {
            title: "Durable Console status".to_owned(),
            lines: vec![
                format!("conversation: {}", interaction.conversation_id),
                format!(
                    "target: {}",
                    display_interaction_target(&interaction.config.target)
                ),
                format!("state: {:?}", interaction.state),
                format!("turns: {}", interaction.turn_count),
                format!("active turns: {}", active_turns.len()),
                "pending approvals: visibility unavailable in Console".to_owned(),
                format!(
                    "model/provider: {}/{} (read-only; selection unsupported)",
                    interaction
                        .config
                        .model
                        .as_deref()
                        .unwrap_or("runtime default"),
                    interaction
                        .config
                        .provider
                        .as_deref()
                        .unwrap_or("runtime default")
                ),
            ],
            selection: None,
        }),
        CommandOutput::InteractionCreated { interaction } => {
            validate_console_target(&interaction, agent_id).map_err(|error| error.to_string())?;
            let (_, turns) = load_interaction_history(runtime, &interaction).await?;
            Ok(ProjectedConsoleCommand {
                title: "Created durable interaction".to_owned(),
                lines: vec![
                    format!("conversation: {}", interaction.conversation_id),
                    format!("agent: {agent_name}"),
                    format!(
                        "title: {}",
                        interaction.title.as_deref().unwrap_or("untitled")
                    ),
                ],
                selection: Some(ConsoleConversationSelection {
                    conversation_id: interaction.conversation_id.to_string(),
                    turns,
                }),
            })
        }
        CommandOutput::InteractionResumed { interaction } => {
            validate_console_target(&interaction, agent_id).map_err(|error| error.to_string())?;
            let (_, turns) = load_interaction_history(runtime, &interaction).await?;
            Ok(ProjectedConsoleCommand {
                title: "Resumed durable interaction".to_owned(),
                lines: vec![
                    format!("conversation: {}", interaction.conversation_id),
                    format!("agent: {agent_name}"),
                    format!("restored turns: {}", turns.len()),
                ],
                selection: Some(ConsoleConversationSelection {
                    conversation_id: interaction.conversation_id.to_string(),
                    turns,
                }),
            })
        }
        CommandOutput::Agents { .. }
        | CommandOutput::TargetChanged { .. }
        | CommandOutput::Runs { .. }
        | CommandOutput::RunInspected { .. }
        | CommandOutput::CancellationRequested { .. }
        | CommandOutput::ApprovalResolved { .. }
        | CommandOutput::Model { .. } => {
            Err("shared command returned an output unsupported by Console".to_owned())
        }
    }
}

fn display_interaction_target(target: &InteractionTarget) -> String {
    match target {
        InteractionTarget::Agent(id) => format!("agent {id}"),
        InteractionTarget::Group(id) => format!("group {id}"),
        InteractionTarget::Auto => "automatic".to_owned(),
    }
}

struct TuiCommandRuntime {
    service: Arc<dyn InteractionService>,
    agent_id: AgentId,
}

#[async_trait]
impl InteractionCommandRuntime for TuiCommandRuntime {
    async fn default_interaction_config(&self) -> Result<InteractionConfig, InteractionError> {
        Ok(InteractionConfig::new(InteractionTarget::Agent(
            self.agent_id,
        )))
    }

    async fn list_agents(&self) -> Result<Vec<AgentTargetView>, InteractionError> {
        Err(tui_command_unsupported("agent discovery"))
    }

    async fn resolve_agent(&self, _selector: &str) -> Result<AgentTargetView, InteractionError> {
        Err(tui_command_unsupported("agent selection"))
    }

    async fn list_runs(
        &self,
        _conversation_id: ConversationId,
    ) -> Result<Vec<RunSummaryView>, InteractionError> {
        Err(tui_command_unsupported("run listing"))
    }

    async fn inspect_run(&self, _run_id: RunId) -> Result<RunDetailView, InteractionError> {
        Err(tui_command_unsupported("run inspection"))
    }

    async fn cancel_run(&self, _run_id: RunId) -> Result<(), InteractionError> {
        Err(tui_command_unsupported("run-scoped cancellation"))
    }

    async fn active_turns(
        &self,
        conversation_id: ConversationId,
    ) -> Result<Vec<TurnHandle>, InteractionError> {
        Ok(self
            .service
            .list_turns(conversation_id)
            .await?
            .into_iter()
            .filter(|turn| !turn.state.is_terminal())
            .map(|turn| turn.handle)
            .collect())
    }

    async fn pending_approvals(
        &self,
        _conversation_id: ConversationId,
    ) -> Result<Vec<ApprovalId>, InteractionError> {
        Ok(Vec::new())
    }
}

fn tui_command_unsupported(capability: &str) -> InteractionError {
    InteractionError::new(
        InteractionErrorCode::Unsupported,
        format!("Console does not support {capability}"),
    )
}

async fn find_console_interaction(
    service: &impl InteractionService,
    agent_id: AgentId,
) -> Result<Option<InteractionSummary>, InteractionError> {
    let mut offset = 0_u32;
    loop {
        let interactions = service
            .list_interactions(ListInteractionsRequest {
                limit: 1_000,
                offset,
                state: Some(DurableInteractionState::Active),
            })
            .await?;
        let page_len = u32::try_from(interactions.len()).map_err(|_| {
            InteractionError::invalid_request("TUI interaction page exceeds supported size")
        })?;
        if let Some(interaction) = interactions.into_iter().find(|interaction| {
            interaction.title.as_deref().is_some_and(|title| {
                title == TUI_INTERACTION_TITLE_PREFIX
                    || title.starts_with(&format!("{TUI_INTERACTION_TITLE_PREFIX} · "))
            }) && interaction.config.target == InteractionTarget::Agent(agent_id)
        }) {
            return Ok(Some(interaction));
        }
        if page_len < 1_000 {
            return Ok(None);
        }
        offset = offset.checked_add(page_len).ok_or_else(|| {
            InteractionError::invalid_request("TUI interaction pagination overflowed")
        })?;
    }
}

async fn find_or_create_console_interaction(
    service: &impl InteractionService,
    runtime: &PolkagentRuntime,
    agent_id: AgentId,
    agent_name: &str,
) -> Result<InteractionSummary, InteractionError> {
    if let Some(interaction) = find_console_interaction(service, agent_id).await? {
        return Ok(interaction);
    }
    service
        .new_interaction(CreateInteractionRequest {
            title: Some(tui_interaction_title(agent_name)),
            config: InteractionConfig::new(InteractionTarget::Agent(agent_id)),
            client_context: tui_client_context(runtime)?,
        })
        .await
}

async fn resolve_prompt_interaction(
    service: &impl InteractionService,
    runtime: &PolkagentRuntime,
    agent_id: AgentId,
    agent_name: &str,
    conversation_id: Option<&str>,
) -> Result<InteractionSummary, InteractionError> {
    let Some(conversation_id) = conversation_id else {
        return find_or_create_console_interaction(service, runtime, agent_id, agent_name).await;
    };
    let conversation_id = conversation_id.parse::<ConversationId>().map_err(|error| {
        InteractionError::invalid_request(format!(
            "invalid selected Console conversation ID '{conversation_id}': {error}"
        ))
    })?;
    let interaction = service.load_interaction(conversation_id).await?;
    validate_console_target(&interaction, agent_id)?;
    Ok(interaction)
}

fn validate_console_target(
    interaction: &InteractionSummary,
    agent_id: AgentId,
) -> Result<(), InteractionError> {
    match interaction.config.target {
        InteractionTarget::Agent(target) if target == agent_id => Ok(()),
        InteractionTarget::Agent(target) => Err(InteractionError::new(
            InteractionErrorCode::Conflict,
            format!(
                "conversation {} targets agent {target}, not selected agent {agent_id}",
                interaction.conversation_id
            ),
        )),
        InteractionTarget::Group(_) => Err(InteractionError::new(
            InteractionErrorCode::Unsupported,
            "Console does not support group interactions",
        )),
        InteractionTarget::Auto => Err(InteractionError::new(
            InteractionErrorCode::Unsupported,
            "Console requires a durable single-agent interaction target",
        )),
    }
}

async fn load_console_history(
    runtime: &PolkagentRuntime,
    agent_id: AgentId,
) -> Result<(Option<ConversationId>, Vec<ConsoleRun>), String> {
    let service = runtime.interactions();
    let Some(interaction) = find_console_interaction(service.as_ref(), agent_id)
        .await
        .map_err(|error| error.to_string())?
    else {
        return Ok((None, Vec::new()));
    };
    load_interaction_history(runtime, &interaction).await
}

async fn load_interaction_history(
    runtime: &PolkagentRuntime,
    interaction: &InteractionSummary,
) -> Result<(Option<ConversationId>, Vec<ConsoleRun>), String> {
    let service = runtime.interactions();
    let capacity = usize::try_from(interaction.turn_count)
        .map_err(|_| "TUI transcript turn count exceeds this platform".to_owned())?;
    let mut transcript = Vec::with_capacity(capacity);
    let mut offset = 0_u32;
    while offset < interaction.turn_count {
        let limit = interaction.turn_count.saturating_sub(offset).min(1_000);
        let page = service
            .load_transcript(TranscriptRequest {
                conversation_id: interaction.conversation_id,
                limit,
                offset,
            })
            .await
            .map_err(|error| format!("load TUI interaction transcript: {error}"))?;
        if page.is_empty() {
            return Err("durable TUI transcript ended before its recorded turn count".to_owned());
        }
        let page_len = u32::try_from(page.len())
            .map_err(|_| "TUI transcript page exceeds supported size".to_owned())?;
        transcript.extend(page);
        offset = offset
            .checked_add(page_len)
            .ok_or_else(|| "TUI transcript pagination overflowed".to_owned())?;
    }

    let mut turns = Vec::with_capacity(transcript.len());
    for transcript_turn in transcript {
        let summary = transcript_turn.turn;
        let (mut output, output_tokens) = if summary.state == TurnState::Completed {
            (
                transcript_turn.assistant_text.ok_or_else(|| {
                    "completed durable TUI turn is missing its assistant transcript".to_owned()
                })?,
                transcript_turn.assistant_token_count,
            )
        } else {
            (String::new(), None)
        };
        truncate_output(&mut output);
        let status = console_status(summary.state);
        turns.push(ConsoleRun {
            conversation_id: Some(interaction.conversation_id.to_string()),
            turn_id: Some(summary.handle.turn_id.to_string()),
            run_id: summary.handle.run_ids.first().map(ToString::to_string),
            prompt: transcript_turn.user_text,
            output,
            detail: format!("restored durable {} turn", status.label()),
            status,
            input_tokens: u64::from(transcript_turn.user_token_count.unwrap_or(0)),
            output_tokens: u64::from(output_tokens.unwrap_or(0)),
        });
    }
    Ok((Some(interaction.conversation_id), turns))
}

fn console_status(state: TurnState) -> ConsoleRunStatus {
    match state {
        TurnState::Pending => ConsoleRunStatus::Starting,
        TurnState::Running | TurnState::AwaitingApproval => ConsoleRunStatus::Running,
        TurnState::Completed => ConsoleRunStatus::Completed,
        TurnState::Failed => ConsoleRunStatus::Failed,
        TurnState::Cancelled => ConsoleRunStatus::Cancelled,
        TurnState::TimedOut => ConsoleRunStatus::TimedOut,
    }
}

async fn request_turn_cancellation(
    service: &impl InteractionService,
    turn_id: InteractionTurnId,
    event_tx: &mpsc::Sender<ControllerEvent>,
) {
    let progress = match service.cancel_turn(turn_id).await {
        Ok(()) => format!("cancellation requested for turn {turn_id}"),
        Err(error) => format!("cancelling interaction turn {turn_id}: {error}"),
    };
    let _ = event_tx.send(ControllerEvent::Progress(progress));
}

fn project_interaction_event(event: InteractionEvent) -> ControllerEvent {
    match event {
        InteractionEvent::TurnStarted { .. } => {
            ControllerEvent::Progress("durable turn started".to_owned())
        }
        InteractionEvent::AgentMessageDelta { text, .. } => ControllerEvent::Output(text),
        InteractionEvent::ThoughtDelta { .. } => {
            ControllerEvent::Progress("agent reasoning".to_owned())
        }
        InteractionEvent::ToolCallStarted { call } | InteractionEvent::ToolCallUpdated { call } => {
            ControllerEvent::Progress(format!("tool {}: {:?}", call.title, call.status))
        }
        InteractionEvent::PlanUpdated { entries } => {
            ControllerEvent::Progress(format!("plan updated: {} step(s)", entries.len()))
        }
        InteractionEvent::ApprovalRequested { request } => ControllerEvent::Progress(format!(
            "approval {} is unavailable in Console; use an authorized approval surface",
            request.approval_id
        )),
        InteractionEvent::ApprovalResolved {
            approval_id,
            decision,
        } => ControllerEvent::Progress(format!("approval {approval_id} resolved: {decision:?}")),
        InteractionEvent::UsageUpdated { usage } => ControllerEvent::UsageUpdated {
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
        },
        InteractionEvent::RunStateChanged { state, .. } => {
            ControllerEvent::Progress(format!("run {state}"))
        }
        InteractionEvent::TurnCompleted { result } => ControllerEvent::Completed {
            text: result.text,
            input_tokens: result.usage.input_tokens,
            output_tokens: result.usage.output_tokens,
        },
        InteractionEvent::TurnFailed { error } => ControllerEvent::Failed(error.to_string()),
        InteractionEvent::TurnCancelled { reason } => ControllerEvent::Cancelled(
            reason.unwrap_or_else(|| "interaction turn cancelled".to_owned()),
        ),
        InteractionEvent::TurnTimedOut => ControllerEvent::TimedOut,
    }
}

/// Build runtime options for the TUI without losing root config/database
/// selection. The simulated-adapter policy preserves the Console's established
/// local-first fallback and is reported as degraded readiness by the runtime.
pub fn tui_runtime_options(
    pool: &SqlitePool,
    config_path: Option<&Path>,
) -> anyhow::Result<RuntimeOptions> {
    let workdir = std::env::current_dir()
        .map_err(|error| anyhow::anyhow!("resolving TUI working directory: {error}"))?;
    let mut options = RuntimeOptions::new(workdir);
    options.config_path = config_path.map(Path::to_path_buf);
    options.database_path = Some(pool.path().to_path_buf());
    options.adapter_policy = AdapterPolicy::AllowSimulated;
    Ok(options)
}

fn runtime_notes(readiness: &RuntimeReadiness) -> Vec<String> {
    let mut notes = Vec::new();
    if readiness.executor.state == ComponentState::Degraded
        && readiness
            .warnings
            .iter()
            .any(|warning| warning.code == WarningCode::SimulatedExecutor)
    {
        notes.push("No API key or local model configured. Using simulated responses.".to_owned());
    } else if readiness.executor.state == ComponentState::Ready {
        notes.push(readiness.executor.detail.clone());
    }

    match readiness.harness.state {
        ComponentState::Ready => notes.push(readiness.harness.detail.clone()),
        ComponentState::Disabled => {
            notes.push("No harness found. Running in executor-only mode.".to_owned());
        }
        ComponentState::Degraded | ComponentState::Unavailable => {
            if let Some(warning) = readiness
                .warnings
                .iter()
                .find(|warning| warning.code == WarningCode::HarnessUnavailable)
            {
                notes.push(warning.message.clone());
            }
        }
    }
    notes
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    use polkagent_core::RunId;
    use polkagent_runtime::{ConfigSource, RuntimeFactory};
    use polkagent_store_sqlite::{migrations, SqliteRunStore};
    use polkagent_store_trait::{RunStatus, RunStore};

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
            conversation_id: "conversation-id".to_owned(),
            turn_id: "turn-id".to_owned(),
            run_id: "run-id".to_owned(),
            agent_name: "Alice".to_owned(),
            notes: vec!["simulated provider".to_owned()],
        });
        state.apply(ControllerEvent::Output("hello ".to_owned()));
        state.apply(ControllerEvent::Output("world".to_owned()));
        state.apply(ControllerEvent::Completed {
            text: "hello world".to_owned(),
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
    fn reducer_bounds_terminal_output_on_utf8_boundaries() {
        let mut state = InteractionState::default();
        state.select_agent("agent-id", "Alice");
        type_prompt(&mut state, "produce a large answer");
        state.submit().expect("valid prompt");
        let final_text = format!("{}tail", "界".repeat(MAX_OUTPUT_BYTES));

        state.apply(ControllerEvent::Completed {
            text: final_text,
            input_tokens: 1,
            output_tokens: 2,
        });

        let output = &state.run.as_ref().expect("completed run").output;
        assert!(output.len() <= MAX_OUTPUT_BYTES);
        let body = output.strip_suffix("tail").expect("preserve newest output");
        assert!(body.chars().all(|character| character == '界'));
    }

    #[test]
    fn reducer_restores_durable_transcript_and_composer_history() {
        let mut state = InteractionState::default();
        state.select_agent("agent-id", "Alice");
        let restored = |ordinal: u8| ConsoleRun {
            conversation_id: Some("conversation-id".to_owned()),
            turn_id: Some(format!("turn-{ordinal}")),
            run_id: Some(format!("run-{ordinal}")),
            prompt: format!("prompt {ordinal}"),
            output: format!("answer {ordinal}"),
            status: ConsoleRunStatus::Completed,
            detail: "restored durable completed turn".to_owned(),
            input_tokens: 0,
            output_tokens: 0,
        };
        state.apply(ControllerEvent::HistoryLoaded {
            agent_id: "agent-id".to_owned(),
            conversation_id: Some("conversation-id".to_owned()),
            turns: vec![restored(1), restored(2)],
        });

        assert_eq!(state.conversation_id.as_deref(), Some("conversation-id"));
        assert_eq!(state.transcript.len(), 1);
        assert_eq!(
            state.run.as_ref().map(|run| run.prompt.as_str()),
            Some("prompt 2")
        );
        state.move_up();
        assert_eq!(state.prompt_buffer, "prompt 2");
        state.move_up();
        assert_eq!(state.prompt_buffer, "prompt 1");
    }

    #[test]
    fn reducer_ignores_history_that_arrives_after_a_prompt_finishes() {
        let mut state = InteractionState::default();
        state.select_agent("agent-id", "Alice");
        type_prompt(&mut state, "new prompt");
        state.submit().expect("valid prompt");
        state.apply(ControllerEvent::Completed {
            text: "new answer".to_owned(),
            input_tokens: 1,
            output_tokens: 2,
        });

        state.apply(ControllerEvent::HistoryLoaded {
            agent_id: "agent-id".to_owned(),
            conversation_id: Some("stale-conversation".to_owned()),
            turns: Vec::new(),
        });

        assert_eq!(
            state.run.as_ref().map(|run| run.output.as_str()),
            Some("new answer")
        );
        assert_ne!(state.conversation_id.as_deref(), Some("stale-conversation"));
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

    #[test]
    fn slash_completion_uses_shared_registry_metadata_and_aliases() {
        let mut state = InteractionState::default();
        type_prompt(&mut state, "/st");

        let menu = state.slash_command_menu().expect("status completion");
        assert_eq!(menu.candidates.len(), 1);
        let candidate = menu.selected_candidate().expect("selected completion");
        assert_eq!(candidate.name, "status");
        assert_eq!(candidate.aliases, vec!["st"]);
        assert_eq!(
            candidate.description,
            "Show the current target, model, runs, approvals, usage, and budget"
        );
        assert_eq!(candidate.usage(), "/status");
    }

    #[test]
    fn slash_completion_navigates_accepts_and_dismisses() {
        let mut state = InteractionState::default();
        type_prompt(&mut state, "/");

        let menu = state.slash_command_menu().expect("slash completions");
        assert_eq!(
            menu.candidates
                .iter()
                .map(|candidate| candidate.name.as_str())
                .collect::<Vec<_>>(),
            vec!["help", "status", "new", "resume"]
        );
        assert_eq!(menu.selected, 0);

        state.move_down();
        assert_eq!(state.slash_command_menu().unwrap().selected, 1);
        assert!(state.accept_slash_completion());
        assert_eq!(state.prompt_buffer, "/status");
        assert_eq!(state.cursor(), state.prompt_buffer.len());

        assert!(state.dismiss_slash_completion());
        assert!(state.slash_command_menu().is_none());
        state.backspace();
        assert!(state.slash_command_menu().is_some());
    }

    #[test]
    fn slash_submission_is_not_misrouted_as_an_agent_prompt() {
        let mut state = InteractionState::default();
        state.select_agent("agent-id", "Alice");
        type_prompt(&mut state, "/runs");

        assert_eq!(
            state.submit(),
            Err("slash commands must use the Console command path")
        );
        assert_eq!(state.prompt_buffer, "/runs");
        assert!(state.run.is_none());

        assert_eq!(
            state.submit_command(),
            Ok(ConsoleCommandSubmission::Rejected)
        );
        let result = state.command_result.as_ref().expect("structured refusal");
        assert_eq!(result.status, ConsoleCommandStatus::Failed);
        assert!(result.lines[0].contains("run inspection commands"));
        assert!(state.run.is_none());
        assert!(state.prompt_buffer.is_empty());
    }

    #[test]
    fn supported_slash_submission_uses_typed_command_without_creating_a_run() {
        let mut state = InteractionState::default();
        state.select_agent("agent-id", "Alice");
        state.conversation_id = Some(ConversationId::new().to_string());
        type_prompt(&mut state, "/status");

        let ConsoleCommandSubmission::Execute(request) =
            state.submit_command().expect("typed Console command")
        else {
            panic!("status command was rejected")
        };
        assert!(matches!(
            request.invocation.command,
            InteractionCommand::Status
        ));
        assert_eq!(request.line, "/status");
        assert!(state.run.is_none());
        assert_eq!(
            state.command_result.as_ref().map(|result| result.status),
            Some(ConsoleCommandStatus::Running)
        );
    }

    #[test]
    fn command_reducer_switches_conversation_and_ignores_stale_results() {
        let mut state = InteractionState::default();
        state.select_agent("agent-id", "Alice");
        type_prompt(&mut state, "/new Fresh");
        let ConsoleCommandSubmission::Execute(request) =
            state.submit_command().expect("new command")
        else {
            panic!("new command was rejected")
        };
        let selected_id = ConversationId::new().to_string();
        state.apply(ControllerEvent::CommandCompleted {
            agent_id: "agent-id".to_owned(),
            request_id: "stale-request".to_owned(),
            result: ConsoleCommandResult {
                request_id: "stale-request".to_owned(),
                line: "/new Stale".to_owned(),
                status: ConsoleCommandStatus::Completed,
                title: "stale".to_owned(),
                lines: Vec::new(),
            },
            selection: Some(ConsoleConversationSelection {
                conversation_id: "stale-conversation".to_owned(),
                turns: Vec::new(),
            }),
        });
        assert_ne!(state.conversation_id.as_deref(), Some("stale-conversation"));

        state.apply(ControllerEvent::CommandCompleted {
            agent_id: "agent-id".to_owned(),
            request_id: request.request_id.clone(),
            result: ConsoleCommandResult {
                request_id: request.request_id,
                line: request.line,
                status: ConsoleCommandStatus::Completed,
                title: "Created durable interaction".to_owned(),
                lines: vec![format!("conversation: {selected_id}")],
            },
            selection: Some(ConsoleConversationSelection {
                conversation_id: selected_id.clone(),
                turns: Vec::new(),
            }),
        });
        assert_eq!(state.conversation_id.as_deref(), Some(selected_id.as_str()));
        assert_eq!(
            state.command_result.as_ref().map(|result| result.status),
            Some(ConsoleCommandStatus::Completed)
        );
        assert!(state.run.is_none());
    }

    fn writer_pointer(pool: &SqlitePool) -> *const rusqlite::Connection {
        let writer = pool.writer();
        std::ptr::from_ref(&*writer)
    }

    async fn wait_for_controller_terminal(
        controller: &mut RunController,
    ) -> (Option<String>, Vec<ControllerEvent>) {
        tokio::time::timeout(Duration::from_secs(5), async {
            let mut run_id = None;
            let mut observed = Vec::new();
            loop {
                if let Some(event) = controller.try_recv() {
                    if let ControllerEvent::Started {
                        run_id: started_id, ..
                    } = &event
                    {
                        run_id = Some(started_id.clone());
                    }
                    let terminal = event.is_terminal();
                    observed.push(event);
                    if terminal {
                        return (run_id, observed);
                    }
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("controller terminal event timeout")
    }

    async fn wait_for_history(controller: &mut RunController) -> ControllerEvent {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Some(event) = controller.try_recv() {
                    if matches!(
                        event,
                        ControllerEvent::HistoryLoaded { .. }
                            | ControllerEvent::HistoryFailed { .. }
                    ) {
                        return event;
                    }
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("controller history event timeout")
    }

    fn started_correlation(events: &[ControllerEvent]) -> (&str, &str, &str) {
        events
            .iter()
            .find_map(|event| match event {
                ControllerEvent::Started {
                    conversation_id,
                    turn_id,
                    run_id,
                    ..
                } => Some((conversation_id.as_str(), turn_id.as_str(), run_id.as_str())),
                _ => None,
            })
            .expect("controller started correlation")
    }

    #[test]
    fn tui_runtime_options_preserve_config_and_durable_database_selection() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let database_path = temp.path().join("tui-options.db");
        let config_path = temp.path().join("selected.toml");
        std::fs::write(&config_path, "").expect("write config");
        let pool = SqlitePool::open(&database_path).expect("open database");

        let options = tui_runtime_options(&pool, Some(&config_path)).expect("TUI runtime options");

        assert_eq!(options.config_path.as_deref(), Some(config_path.as_path()));
        assert_eq!(
            options.database_path.as_deref(),
            Some(database_path.as_path())
        );
        assert_eq!(options.adapter_policy, AdapterPolicy::AllowSimulated);
        assert!(options.discover_environment_providers);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn console_commands_create_prompt_resume_after_restart_and_report_status() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let database_path = temp.path().join("tui-command-runtime.db");
        let config_path = temp.path().join("selected.toml");
        std::fs::write(&config_path, "").expect("write config");
        let seed_pool = SqlitePool::open(&database_path).expect("open database");
        migrations::migrate(&seed_pool.writer()).expect("migrate database");
        let store = SqliteRunStore::new(seed_pool.clone());
        let timestamp = "2026-01-01T00:00:00Z";
        let spec = serde_json::json!({
            "name": "command-agent",
            "description": null,
            "model": "fake/default-model",
            "tools": [],
            "system_prompt": null,
            "autonomy_level": "supervised",
            "created_at": timestamp,
            "updated_at": timestamp,
        });
        let agent = store
            .create_agent("command-agent", None, &spec.to_string())
            .expect("create active agent");
        let mut options =
            tui_runtime_options(&seed_pool, Some(&config_path)).expect("TUI runtime options");
        options.workdir = temp.path().to_path_buf();
        options.disable_harness = true;
        options.discover_environment_providers = false;
        drop(store);
        drop(seed_pool);

        let restart_options = options.clone();
        let runtime = RuntimeFactory::build(options).await.expect("build runtime");
        let mut state = InteractionState::default();
        state.select_agent(agent.id.clone(), agent.name.clone());
        let mut controller = RunController::new(runtime.clone());

        type_prompt(&mut state, "/new 'Fresh Console session'");
        let ConsoleCommandSubmission::Execute(request) =
            state.submit_command().expect("new command")
        else {
            panic!("new command rejected")
        };
        controller
            .execute_command(request)
            .expect("execute new command");
        let (_, events) = wait_for_controller_terminal(&mut controller).await;
        for event in events {
            state.apply(event);
        }
        let conversation_id = state
            .conversation_id
            .clone()
            .expect("new command selects conversation");
        assert_eq!(
            state
                .command_result
                .as_ref()
                .map(|result| result.title.as_str()),
            Some("Created durable interaction")
        );

        type_prompt(&mut state, "prompt in explicitly selected session");
        let prompt = state.submit().expect("prompt after new command");
        assert_eq!(
            prompt.conversation_id.as_deref(),
            Some(conversation_id.as_str())
        );
        controller.start(prompt).expect("start selected prompt");
        let (_, events) = wait_for_controller_terminal(&mut controller).await;
        for event in events {
            state.apply(event);
        }
        assert_eq!(
            state.run.as_ref().map(|run| run.output.as_str()),
            Some("I am a fake assistant")
        );
        drop(controller);
        drop(runtime);

        let restarted = RuntimeFactory::build(restart_options)
            .await
            .expect("restart runtime");
        let mut controller = RunController::new(restarted.clone());
        let mut resumed = InteractionState::default();
        resumed.select_agent(agent.id.clone(), agent.name.clone());
        type_prompt(&mut resumed, &format!("/resume {conversation_id}"));
        let ConsoleCommandSubmission::Execute(request) =
            resumed.submit_command().expect("resume command")
        else {
            panic!("resume command rejected")
        };
        controller
            .execute_command(request)
            .expect("execute resume command");
        let (_, events) = wait_for_controller_terminal(&mut controller).await;
        for event in events {
            resumed.apply(event);
        }
        assert_eq!(
            resumed.conversation_id.as_deref(),
            Some(conversation_id.as_str())
        );
        assert_eq!(
            resumed.run.as_ref().map(|run| run.prompt.as_str()),
            Some("prompt in explicitly selected session")
        );

        for command in ["/status", "/help"] {
            type_prompt(&mut resumed, command);
            let ConsoleCommandSubmission::Execute(request) =
                resumed.submit_command().expect("query command")
            else {
                panic!("query command rejected")
            };
            controller
                .execute_command(request)
                .expect("execute query command");
            let (_, events) = wait_for_controller_terminal(&mut controller).await;
            for event in events {
                resumed.apply(event);
            }
            assert_eq!(
                resumed.command_result.as_ref().map(|result| result.status),
                Some(ConsoleCommandStatus::Completed)
            );
        }
        let help = resumed.command_result.as_ref().expect("help result");
        assert!(help.lines.iter().any(|line| line.starts_with("/new")));
        assert!(help.lines.iter().any(|line| line.starts_with("/resume")));
        assert!(help.lines.iter().any(|line| line.starts_with("x —")));
        let turns = restarted
            .interactions()
            .list_turns(conversation_id.parse().expect("conversation ID"))
            .await
            .expect("list durable turns");
        assert_eq!(
            turns.len(),
            1,
            "slash commands must never become model turns"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    #[allow(clippy::too_many_lines)]
    async fn controller_reuses_durable_interaction_across_followup_restart_and_cancel() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let database_path = temp.path().join("tui-runtime.db");
        let config_path = temp.path().join("selected.toml");
        std::fs::write(&config_path, "").expect("write config");
        let seed_pool = SqlitePool::open(&database_path).expect("open database");
        migrations::migrate(&seed_pool.writer()).expect("migrate database");
        let store = SqliteRunStore::new(seed_pool.clone());
        let timestamp = "2026-01-01T00:00:00Z";
        let spec = serde_json::json!({
            "name": "console-agent",
            "description": null,
            "model": "fake/default-model",
            "tools": [],
            "system_prompt": null,
            "autonomy_level": "supervised",
            "created_at": timestamp,
            "updated_at": timestamp,
        });
        let agent = store
            .create_agent("console-agent", None, &spec.to_string())
            .expect("create active agent");
        let abandoned_run = RunId::new();
        RunStore::create(
            &seed_pool,
            abandoned_run,
            &agent.id,
            RunStatus::new("running"),
        )
        .await
        .expect("seed abandoned run");

        let mut options =
            tui_runtime_options(&seed_pool, Some(&config_path)).expect("TUI runtime options");
        options.workdir = temp.path().to_path_buf();
        options.disable_harness = true;
        options.discover_environment_providers = false;
        drop(store);
        drop(seed_pool);

        let restart_options = options.clone();
        let runtime = RuntimeFactory::build(options)
            .await
            .expect("build shared TUI runtime");
        assert_eq!(runtime.readiness().recovered_runs, 1);
        assert_eq!(runtime.readiness().rehydrated_agents, 1);
        assert!(matches!(
            runtime.readiness().config_source,
            ConfigSource::Explicit { ref path } if path == &config_path
        ));

        let shared_service = Arc::clone(runtime.app());
        let runtime_writer = writer_pointer(runtime.pool());
        let mut controller = RunController::new(runtime.clone());
        assert!(Arc::ptr_eq(
            &shared_service,
            controller.polkagent_runtime.app()
        ));
        assert!(std::ptr::eq(
            runtime_writer,
            writer_pointer(controller.polkagent_runtime.pool())
        ));

        let mut completed_correlations = Vec::new();
        for prompt in ["first shared prompt", "second shared prompt"] {
            controller
                .start(PromptRequest {
                    agent_id: agent.id.clone(),
                    agent_name: agent.name.clone(),
                    conversation_id: None,
                    prompt: prompt.to_owned(),
                })
                .expect("start sequential prompt");
            let (_, observed) = wait_for_controller_terminal(&mut controller).await;
            let correlation = started_correlation(&observed);
            assert!(observed.iter().any(|event| matches!(
                event,
                ControllerEvent::Started { notes, .. }
                    if notes.iter().any(|note| note.contains("simulated responses"))
            )));
            assert!(observed
                .iter()
                .any(|event| matches!(event, ControllerEvent::Output(text) if !text.is_empty())));
            assert!(observed.iter().any(|event| matches!(
                event,
                ControllerEvent::Completed { text, .. } if text == "I am a fake assistant"
            )));
            completed_correlations.push((
                correlation.0.to_owned(),
                correlation.1.to_owned(),
                correlation.2.to_owned(),
            ));
        }
        assert_eq!(completed_correlations[0].0, completed_correlations[1].0);
        assert_ne!(completed_correlations[0].1, completed_correlations[1].1);
        assert_ne!(completed_correlations[0].2, completed_correlations[1].2);

        let durable_store = SqliteRunStore::new(runtime.pool().clone());
        for (_, _, run_id) in &completed_correlations {
            let run = durable_store.get_run(run_id).expect("durable Console run");
            assert_eq!(run.state, "completed");
            assert_eq!(run.agent_id, agent.id);
        }
        assert_eq!(
            durable_store
                .get_run(&abandoned_run.to_string())
                .expect("recovered run")
                .state,
            "failed"
        );

        drop(durable_store);
        drop(controller);
        drop(runtime);

        let restarted = RuntimeFactory::build(restart_options)
            .await
            .expect("restart shared TUI runtime");
        let mut controller = RunController::new(restarted.clone());
        controller
            .load_history(agent.id.clone())
            .expect("load durable Console history");
        let history = wait_for_history(&mut controller).await;
        let ControllerEvent::HistoryLoaded {
            conversation_id,
            turns,
            ..
        } = history
        else {
            panic!("durable Console history failed to load: {history:?}");
        };
        assert_eq!(
            conversation_id.as_deref(),
            Some(completed_correlations[0].0.as_str())
        );
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].prompt, "first shared prompt");
        assert_eq!(turns[1].prompt, "second shared prompt");
        assert!(turns
            .iter()
            .all(|turn| turn.output == "I am a fake assistant"));

        controller
            .start(PromptRequest {
                agent_id: agent.id.clone(),
                agent_name: agent.name.clone(),
                conversation_id: None,
                prompt: "follow up after restart".to_owned(),
            })
            .expect("start follow-up prompt");
        let (_, follow_up) = wait_for_controller_terminal(&mut controller).await;
        let follow_up_correlation = started_correlation(&follow_up);
        assert_eq!(follow_up_correlation.0, completed_correlations[0].0);
        assert!(follow_up.iter().any(|event| matches!(
            event,
            ControllerEvent::Completed { text, .. } if text == "I am a fake assistant"
        )));

        controller
            .start(PromptRequest {
                agent_id: agent.id.clone(),
                agent_name: agent.name.clone(),
                conversation_id: None,
                prompt: "cancel this durable turn".to_owned(),
            })
            .expect("start cancellable prompt");
        assert!(controller.cancel());
        let (cancelled_run_id, observed) = wait_for_controller_terminal(&mut controller).await;
        let cancelled_run_id = cancelled_run_id.expect("cancelled turn has a durable run");
        assert!(observed
            .iter()
            .any(|event| matches!(event, ControllerEvent::Cancelled(_))));
        let cancelled = SqliteRunStore::new(restarted.pool().clone())
            .get_run(&cancelled_run_id)
            .expect("load cancelled durable Console run");
        assert!(
            cancelled.state.starts_with("cancelled"),
            "unexpected durable cancellation state: {}",
            cancelled.state
        );
        let interactions = restarted
            .interactions()
            .list_interactions(ListInteractionsRequest::default())
            .await
            .expect("list durable interactions");
        assert_eq!(interactions.len(), 1);
        assert_eq!(interactions[0].turn_count, 4);
        let turns = restarted
            .interactions()
            .list_turns(interactions[0].conversation_id)
            .await
            .expect("list durable interaction turns");
        assert_eq!(
            turns.last().map(|turn| turn.state),
            Some(TurnState::Cancelled)
        );
    }
}
