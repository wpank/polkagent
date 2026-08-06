//! Headless interaction state and asynchronous run controller for the TUI.
//!
//! Terminal input/rendering stays in `input` and `views::console`; this module
//! owns the deterministic reducer and the bridge to the same process-wide
//! production runtime used by `polkagent run`.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
#[cfg(test)]
use futures::future::BoxFuture;
use polkagent_core::{AgentId, ApprovalId, ConversationId, RunId};
use polkagent_interaction::{
    format_run_command_output, AgentTargetView, ApprovalDecision, ApprovalView, ClientContext,
    CommandContext, CommandExecutor, CommandInvocation, CommandName, CommandOutput,
    CommandRegistry, CommandRequest, CreateInteractionRequest, InteractionApprovalAuthority,
    InteractionCommand, InteractionCommandRuntime, InteractionConfig, InteractionContent,
    InteractionError, InteractionErrorCode, InteractionEvent, InteractionOverrides,
    InteractionService, InteractionState as DurableInteractionState, InteractionSummary,
    InteractionTarget, InteractionTurnId, ListInteractionsRequest, ParsedLine,
    PromptRequest as InteractionPromptRequest, RunDetailView, RunSummaryView,
    ServiceCommandExecutor, StreamError, SubscriptionRequest, TranscriptRequest, TurnHandle,
    TurnState, UsageView,
};
use polkagent_runtime::{
    AdapterPolicy, ComponentState, PolkagentRuntime, RunCommandReadModel, RuntimeOptions,
    RuntimeReadiness, WarningCode,
};
use polkagent_store_sqlite::SqlitePool;
use tokio::sync::mpsc;
use unicode_segmentation::UnicodeSegmentation;

use crate::commands::interaction_agents::{
    active_agent_by_id, list_active_agent_targets, registered_agent_by_id,
    resolve_active_agent_target,
};

/// Keep a runaway streaming response from growing the terminal process
/// forever. Durable lifecycle/events remain available through the run views.
const MAX_OUTPUT_BYTES: usize = 128 * 1024;
/// Maximum UTF-8 size of the editable Console composer.
pub const MAX_COMPOSER_BYTES: usize = 128 * 1024;
/// Bound composer recall even when a durable interaction has a longer transcript.
const MAX_PROMPT_HISTORY: usize = 100;
const MAX_SESSION_SELECTOR_ITEMS: usize = 50;
const MAX_SESSION_SELECTOR_SCAN: u32 = 1_000;
const MAX_SESSION_TITLE_BYTES: usize = 256;
const MAX_PENDING_APPROVALS: usize = 100;
const MAX_APPROVAL_ERROR_BYTES: usize = 512;
const MAX_DENIAL_REASON_CHARS: usize = 4_096;
const INTERACTION_STREAM_CAPACITY: usize = 256;
const CONTROLLER_EVENT_CAPACITY: usize = 256;
/// Bound concurrently executing Console turns independently of durable history.
pub const MAX_CONCURRENT_CONSOLE_RUNS: usize = 8;
/// Retain a compact in-process activity window without growing the TUI forever.
pub const MAX_RETAINED_CONSOLE_ACTIVITIES: usize = 32;
const MAX_RETAINED_CONSOLE_VIEWPORTS: usize = 32;
const MAX_CONSOLE_TRANSCRIPT_TURNS: usize = 100;
const CONTROLLER_SHUTDOWN_GRACE: std::time::Duration = std::time::Duration::from_secs(1);
const TUI_INTERACTION_TITLE_PREFIX: &str = "TUI Console";
const SUPPORTED_CONSOLE_COMMANDS: [CommandName; 12] = [
    CommandName::Help,
    CommandName::Status,
    CommandName::Agents,
    CommandName::Agent,
    CommandName::Runs,
    CommandName::Inspect,
    CommandName::Cancel,
    CommandName::Approve,
    CommandName::Deny,
    CommandName::New,
    CommandName::Resume,
    CommandName::Model,
];

/// Ephemeral view of the existing F6 service-projected approval queue.
///
/// This is derived on demand from [`crate::tui::state::TuiState`]; the Console
/// never owns or persists a second approval queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsoleApprovalContext {
    conversation_id: ConversationId,
    pending_approval_ids: Vec<ApprovalId>,
}

impl ConsoleApprovalContext {
    #[must_use]
    pub fn new(conversation_id: ConversationId, pending_approval_ids: Vec<ApprovalId>) -> Self {
        Self {
            conversation_id,
            pending_approval_ids,
        }
    }

    fn matches_conversation(&self, conversation_id: Option<ConversationId>) -> bool {
        conversation_id == Some(self.conversation_id)
    }

    fn contains(&self, approval_id: ApprovalId) -> bool {
        self.pending_approval_ids.contains(&approval_id)
    }

    fn pending_count(&self) -> usize {
        self.pending_approval_ids.len()
    }
}

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
    /// Exact selected durable turn for dynamic help and bounded `/cancel`.
    pub selected_turn_id: Option<InteractionTurnId>,
    /// Count from the existing, correlated F6 service projection at submit.
    pub pending_approval_count: usize,
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
pub enum ConsoleApprovalDecision {
    Approve,
    Deny,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsoleApprovalResolution {
    pub conversation_id: ConversationId,
    pub approval_id: ApprovalId,
    /// Decision polarity only; denial rationale is deliberately not retained.
    pub decision: ConsoleApprovalDecision,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsoleCommandResult {
    pub request_id: String,
    pub line: String,
    pub status: ConsoleCommandStatus,
    pub title: String,
    pub lines: Vec<String>,
    /// Structured successful approval output retained for exact retries after
    /// the shared F6 queue refreshes and removes the resolved request.
    pub approval_resolution: Option<ConsoleApprovalResolution>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConsoleCommandSubmission {
    Execute(Box<ConsoleCommandRequest>),
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConsoleModelUpdate {
    Unchanged,
    Selected(Option<String>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsoleConversationSelection {
    pub conversation_id: String,
    pub model: Option<String>,
    pub turns: Vec<ConsoleRun>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsoleAgentSelection {
    pub agent_id: String,
    pub agent_name: String,
}

/// Result of inserting one bracketed-paste payload into the composer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PasteOutcome {
    /// UTF-8 bytes inserted after normalization.
    pub inserted_bytes: usize,
    /// Whether the normalized prefix was clipped at the composer byte limit.
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionPickerStatus {
    LoadingList,
    Ready,
    LoadingSelection,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsoleSessionItem {
    pub conversation_id: String,
    pub title: String,
    pub state: String,
    pub turn_count: u32,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsoleSessionPicker {
    pub request_id: String,
    pub agent_id: String,
    pub status: SessionPickerStatus,
    pub sessions: Vec<ConsoleSessionItem>,
    pub selected: usize,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionListRequest {
    pub request_id: String,
    pub agent_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionLoadRequest {
    pub request_id: String,
    pub agent_id: String,
    pub conversation: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApprovalListRequest {
    pub request_id: uuid::Uuid,
    pub conversation_id: ConversationId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalDecisionRequest {
    pub request_id: uuid::Uuid,
    pub conversation_id: ConversationId,
    pub approval_id: ApprovalId,
    pub decision: ApprovalDecision,
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
    /// In-process controller identity. Restored durable turns do not have one.
    pub activity_id: Option<String>,
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

/// Redaction-safe compact projection used by the activity renderer. Prompt,
/// output, and error detail remain in private bounded reducer state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsoleActivitySummary {
    pub activity_id: String,
    pub agent_name: String,
    pub conversation_id: Option<String>,
    pub run_id: Option<String>,
    pub status: ConsoleRunStatus,
}

#[derive(Clone)]
struct ConsoleActivity {
    activity_id: String,
    agent_id: String,
    agent_name: String,
    model: Option<String>,
    run: ConsoleRun,
}

#[derive(Clone)]
struct ConsoleViewport {
    agent_id: String,
    conversation_id: Option<String>,
    transcript: Vec<ConsoleRun>,
    prompt_buffer: String,
    prompt_cursor: usize,
    prompt_history: Vec<String>,
    prompt_history_index: Option<usize>,
    prompt_history_draft: Option<String>,
    last_selected: u64,
}

#[derive(Clone, Default)]
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
    pub selected_model: Option<String>,
    pub transcript: Vec<ConsoleRun>,
    pub run: Option<ConsoleRun>,
    activities: Vec<ConsoleActivity>,
    selected_activity_id: Option<String>,
    viewports: Vec<ConsoleViewport>,
    viewport_clock: u64,
    pub command_result: Option<ConsoleCommandResult>,
    pub session_picker: Option<ConsoleSessionPicker>,
}

impl std::fmt::Debug for InteractionState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("InteractionState")
            .field("agent_id", &self.agent_id)
            .field("agent_name", &self.agent_name)
            .field("conversation_id", &self.conversation_id)
            .field("selected_model", &self.selected_model)
            .field("prompt_bytes", &self.prompt_buffer.len())
            .field("transcript_turns", &self.transcript.len())
            .field("has_selected_run", &self.run.is_some())
            .field("activities", &self.activity_summaries())
            .field("selected_activity_id", &self.selected_activity_id)
            .field("has_command_result", &self.command_result.is_some())
            .field("has_session_picker", &self.session_picker.is_some())
            .finish_non_exhaustive()
    }
}

impl InteractionState {
    pub fn select_agent(&mut self, agent_id: impl Into<String>, agent_name: impl Into<String>) {
        let agent_id = agent_id.into();
        if self.agent_id.as_deref() != Some(agent_id.as_str()) {
            self.save_selected_viewport();
            self.conversation_id = None;
            self.selected_model = None;
            self.transcript.clear();
            self.run = None;
            self.selected_activity_id = None;
            self.prompt_history.clear();
            self.command_result = None;
            self.session_picker = None;
        }
        self.agent_id = Some(agent_id);
        self.agent_name = Some(agent_name.into());
        self.clear_prompt();
    }

    pub fn push_char(&mut self, c: char) -> bool {
        self.prepare_edit();
        if self.prompt_buffer.len().saturating_add(c.len_utf8()) > MAX_COMPOSER_BYTES {
            return false;
        }
        let insertion = self.prompt_cursor;
        self.prompt_buffer.insert(insertion, c);
        self.prompt_cursor = ceil_grapheme_boundary(&self.prompt_buffer, insertion + c.len_utf8());
        true
    }

    pub fn insert_newline(&mut self) -> bool {
        self.push_char('\n')
    }

    /// Insert a bracketed-paste payload without interpreting embedded newlines
    /// as key events. CRLF and lone CR become LF, tabs become four spaces, and
    /// other Unicode control characters are discarded.
    pub fn insert_paste(&mut self, pasted: &str) -> PasteOutcome {
        self.prepare_edit();
        let remaining = MAX_COMPOSER_BYTES.saturating_sub(self.prompt_buffer.len());
        let (normalized, truncated) = normalize_bounded_paste(pasted, remaining);
        let inserted_bytes = normalized.len();
        let insertion = self.prompt_cursor;
        self.prompt_buffer.insert_str(insertion, &normalized);
        self.prompt_cursor =
            ceil_grapheme_boundary(&self.prompt_buffer, insertion + inserted_bytes);
        PasteOutcome {
            inserted_bytes,
            truncated,
        }
    }

    pub fn backspace(&mut self) {
        self.prepare_edit();
        let previous = previous_grapheme_boundary(&self.prompt_buffer, self.prompt_cursor);
        if previous < self.prompt_cursor {
            self.prompt_buffer.drain(previous..self.prompt_cursor);
            self.prompt_cursor = previous;
        }
    }

    pub fn delete(&mut self) {
        self.prepare_edit();
        let next = next_grapheme_boundary(&self.prompt_buffer, self.prompt_cursor);
        if next > self.prompt_cursor {
            self.prompt_buffer.drain(self.prompt_cursor..next);
        }
    }

    pub fn move_left(&mut self) {
        self.prompt_cursor = previous_grapheme_boundary(&self.prompt_buffer, self.cursor());
    }

    pub fn move_right(&mut self) {
        self.prompt_cursor = next_grapheme_boundary(&self.prompt_buffer, self.cursor());
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
    #[cfg(test)]
    pub fn move_up(&mut self) {
        self.move_up_with_approval_context(None);
    }

    /// Move up with approval commands derived from the existing scoped queue.
    pub fn move_up_with_approval_context(
        &mut self,
        approval_context: Option<&ConsoleApprovalContext>,
    ) {
        if self.move_slash_completion_up(approval_context) {
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

        let column = self.prompt_buffer[line_start..cursor]
            .graphemes(true)
            .count();
        let previous_end = line_start - 1;
        let previous_start = self.prompt_buffer[..previous_end]
            .rfind('\n')
            .map_or(0, |index| index + 1);
        self.prompt_cursor = previous_start
            + grapheme_column_offset(&self.prompt_buffer[previous_start..previous_end], column);
    }

    /// Move vertically within multiline input, falling back to newer history
    /// only when the cursor is already on the last line.
    #[cfg(test)]
    pub fn move_down(&mut self) {
        self.move_down_with_approval_context(None);
    }

    /// Move down with approval commands derived from the existing scoped queue.
    pub fn move_down_with_approval_context(
        &mut self,
        approval_context: Option<&ConsoleApprovalContext>,
    ) {
        if self.move_slash_completion_down(approval_context) {
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
        let column = self.prompt_buffer[line_start..cursor]
            .graphemes(true)
            .count();
        let next_start = line_end + 1;
        let next_end = self.prompt_buffer[next_start..]
            .find('\n')
            .map_or(self.prompt_buffer.len(), |offset| next_start + offset);
        self.prompt_cursor =
            next_start + grapheme_column_offset(&self.prompt_buffer[next_start..next_end], column);
    }

    #[must_use]
    pub fn cursor(&self) -> usize {
        clamp_grapheme_boundary(&self.prompt_buffer, self.prompt_cursor)
    }

    pub fn clear_prompt(&mut self) {
        self.prompt_buffer.clear();
        self.prompt_cursor = 0;
        self.prompt_history_index = None;
        self.prompt_history_draft = None;
        self.slash_completion_selection = 0;
        self.slash_completion_dismissed = false;
    }

    pub fn begin_session_picker(&mut self) -> Result<SessionListRequest, &'static str> {
        let Some(agent_id) = self.agent_id.clone() else {
            return Err("select an active agent before browsing Console sessions");
        };
        if self
            .command_result
            .as_ref()
            .is_some_and(|result| result.status == ConsoleCommandStatus::Running)
        {
            return Err("wait for the active Console command before switching sessions");
        }
        let request_id = uuid::Uuid::now_v7().to_string();
        self.session_picker = Some(ConsoleSessionPicker {
            request_id: request_id.clone(),
            agent_id: agent_id.clone(),
            status: SessionPickerStatus::LoadingList,
            sessions: Vec::new(),
            selected: 0,
            error: None,
        });
        Ok(SessionListRequest {
            request_id,
            agent_id,
        })
    }

    pub fn close_session_picker(&mut self) {
        self.session_picker = None;
    }

    pub fn session_picker_up(&mut self) {
        let Some(picker) = &mut self.session_picker else {
            return;
        };
        if picker.status != SessionPickerStatus::Ready || picker.sessions.is_empty() {
            return;
        }
        picker.selected = if picker.selected == 0 {
            picker.sessions.len() - 1
        } else {
            picker.selected - 1
        };
        picker.error = None;
    }

    pub fn session_picker_down(&mut self) {
        let Some(picker) = &mut self.session_picker else {
            return;
        };
        if picker.status != SessionPickerStatus::Ready || picker.sessions.is_empty() {
            return;
        }
        picker.selected = (picker.selected + 1) % picker.sessions.len();
        picker.error = None;
    }

    pub fn begin_session_selection(&mut self) -> Result<SessionLoadRequest, &'static str> {
        let Some(picker) = &mut self.session_picker else {
            return Err("open the Console session selector first");
        };
        if picker.status != SessionPickerStatus::Ready {
            return Err("wait for the Console session selector to finish loading");
        }
        let Some(conversation_id) = picker
            .sessions
            .get(picker.selected)
            .map(|session| session.conversation_id.clone())
        else {
            picker.error = Some(
                "No durable sessions exist for this agent; use /new [title] in the composer."
                    .to_owned(),
            );
            return Err("no durable Console session is selected");
        };
        let request_id = uuid::Uuid::now_v7().to_string();
        picker.request_id.clone_from(&request_id);
        picker.status = SessionPickerStatus::LoadingSelection;
        picker.error = None;
        Ok(SessionLoadRequest {
            request_id,
            agent_id: picker.agent_id.clone(),
            conversation: conversation_id,
        })
    }

    pub fn fail_session_picker_request(&mut self, request_id: &str, reason: impl Into<String>) {
        let Some(picker) = &mut self.session_picker else {
            return;
        };
        if picker.request_id != request_id {
            return;
        }
        picker.status = if picker.sessions.is_empty() {
            SessionPickerStatus::Failed
        } else {
            SessionPickerStatus::Ready
        };
        picker.error = Some(reason.into());
    }

    /// Return shared slash-command candidates for the current composer text.
    #[must_use]
    #[cfg(test)]
    pub fn slash_command_menu(&self) -> Option<SlashCommandMenu> {
        self.slash_command_menu_with_approval_context(None)
    }

    /// Return shared slash-command candidates using the current scoped
    /// service-projected approval queue.
    #[must_use]
    pub fn slash_command_menu_with_approval_context(
        &self,
        approval_context: Option<&ConsoleApprovalContext>,
    ) -> Option<SlashCommandMenu> {
        if self.slash_completion_dismissed {
            return None;
        }
        if self.prompt_buffer.contains('\n') {
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
        let context = self.command_context(approval_context);

        let candidates = if has_arguments || registry.resolve(&typed_name).is_some() {
            registry
                .resolve(&typed_name)
                .filter(|spec| SUPPORTED_CONSOLE_COMMANDS.contains(&spec.command))
                .filter(|spec| spec.is_available(&context))
                .into_iter()
                .map(slash_candidate)
                .collect()
        } else {
            registry
                .specs()
                .into_iter()
                .filter(|spec| SUPPORTED_CONSOLE_COMMANDS.contains(&spec.command))
                .filter(|spec| spec.is_available(&context))
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

    fn command_context(&self, approval_context: Option<&ConsoleApprovalContext>) -> CommandContext {
        let conversation_id = self
            .conversation_id
            .as_deref()
            .and_then(|id| id.parse::<ConversationId>().ok());
        let pending_approval_count = approval_context
            .filter(|context| context.matches_conversation(conversation_id))
            .map_or(0, ConsoleApprovalContext::pending_count);
        CommandContext {
            conversation_id,
            has_active_turn: self.selected_cancel_turn_id().is_some(),
            pending_approval_count,
            can_mutate: true,
        }
    }

    fn selected_cancel_turn_id(&self) -> Option<InteractionTurnId> {
        let run = self.run.as_ref()?;
        let activity_id = run.activity_id.as_deref()?;
        if run.status.is_terminal()
            || Some(activity_id) != self.selected_activity_id.as_deref()
            || run.conversation_id != self.conversation_id
        {
            return None;
        }
        run.turn_id.as_deref()?.parse().ok()
    }

    /// Replace the typed slash name with the highlighted canonical command.
    ///
    /// Returns whether a completion was available and accepted.
    #[cfg(test)]
    pub fn accept_slash_completion(&mut self) -> bool {
        self.accept_slash_completion_with_approval_context(None)
    }

    /// Accept a completion using the current scoped approval queue.
    pub fn accept_slash_completion_with_approval_context(
        &mut self,
        approval_context: Option<&ConsoleApprovalContext>,
    ) -> bool {
        let Some(candidate) = self
            .slash_command_menu_with_approval_context(approval_context)
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
    #[cfg(test)]
    pub fn dismiss_slash_completion(&mut self) -> bool {
        self.dismiss_slash_completion_with_approval_context(None)
    }

    /// Dismiss a completion using the current scoped approval queue.
    pub fn dismiss_slash_completion_with_approval_context(
        &mut self,
        approval_context: Option<&ConsoleApprovalContext>,
    ) -> bool {
        if self
            .slash_command_menu_with_approval_context(approval_context)
            .is_none()
        {
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
        if self.is_command_input() {
            return Err("slash commands must use the Console command path");
        }
        let Some(agent_id) = self.agent_id.clone() else {
            return Err("select an active agent before prompting");
        };
        let agent_name = self.agent_name.clone().unwrap_or_else(|| agent_id.clone());
        if self.activities.iter().any(|activity| {
            activity.agent_id == agent_id
                && activity.run.conversation_id == self.conversation_id
                && !activity.run.status.is_terminal()
        }) {
            return Err("this conversation already has an active Console turn");
        }
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
                let overflow = self
                    .transcript
                    .len()
                    .saturating_sub(MAX_CONSOLE_TRANSCRIPT_TURNS);
                if overflow > 0 {
                    self.transcript.drain(..overflow);
                }
            }
        }
        self.run = Some(ConsoleRun {
            activity_id: None,
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

    /// Bind the just-submitted selected run to the controller identity and
    /// retain it in the bounded cross-session activity projection.
    pub fn bind_activity(&mut self, activity_id: String) -> Result<(), &'static str> {
        let Some(run) = &mut self.run else {
            return Err("Console activity has no submitted run");
        };
        let Some(agent_id) = self.agent_id.clone() else {
            return Err("Console activity has no selected agent");
        };
        let agent_name = self.agent_name.clone().unwrap_or_else(|| agent_id.clone());
        run.activity_id = Some(activity_id.clone());

        if self.activities.len() >= MAX_RETAINED_CONSOLE_ACTIVITIES {
            let Some(eviction) = self
                .activities
                .iter()
                .position(|activity| activity.run.status.is_terminal())
            else {
                return Err("Console activity capacity is full with active turns");
            };
            self.activities.remove(eviction);
        }
        self.activities.push(ConsoleActivity {
            activity_id: activity_id.clone(),
            agent_id,
            agent_name,
            model: self.selected_model.clone(),
            run: run.clone(),
        });
        self.selected_activity_id = Some(activity_id);
        self.save_selected_viewport();
        Ok(())
    }

    #[must_use]
    pub fn selected_activity_id(&self) -> Option<&str> {
        self.selected_activity_id.as_deref()
    }

    #[must_use]
    pub fn active_activity_count(&self) -> usize {
        self.activities
            .iter()
            .filter(|activity| !activity.run.status.is_terminal())
            .count()
    }

    #[must_use]
    pub fn activity_summaries(&self) -> Vec<ConsoleActivitySummary> {
        self.activities
            .iter()
            .map(|activity| ConsoleActivitySummary {
                activity_id: activity.activity_id.clone(),
                agent_name: activity.agent_name.clone(),
                conversation_id: activity.run.conversation_id.clone(),
                run_id: activity.run.run_id.clone(),
                status: activity.run.status.clone(),
            })
            .collect()
    }

    /// Select an activity by stable insertion order, wrapping at both ends.
    /// Only the selected run is copied into the transcript viewport; all other
    /// activity output continues accumulating in its bounded activity record.
    pub fn select_activity_relative(&mut self, delta: isize) -> bool {
        if self.activities.is_empty() {
            return false;
        }
        let current = self
            .selected_activity_id
            .as_deref()
            .and_then(|selected| {
                self.activities
                    .iter()
                    .position(|activity| activity.activity_id == selected)
            })
            .unwrap_or(0);
        let len = self.activities.len();
        let next = if delta < 0 {
            current
                .checked_sub(delta.unsigned_abs())
                .unwrap_or_else(|| len - (delta.unsigned_abs().saturating_sub(current) % len))
                % len
        } else {
            (current + delta.unsigned_abs()) % len
        };
        self.select_activity_at(next)
    }

    fn select_activity_at(&mut self, index: usize) -> bool {
        let Some(activity) = self.activities.get(index).cloned() else {
            return false;
        };
        self.save_selected_viewport();
        let viewport =
            self.restore_viewport(&activity.agent_id, activity.run.conversation_id.as_deref());
        self.selected_activity_id = Some(activity.activity_id);
        self.agent_id = Some(activity.agent_id);
        self.agent_name = Some(activity.agent_name);
        self.conversation_id = activity.run.conversation_id.clone();
        self.selected_model = activity.model;
        if let Some(viewport) = viewport {
            self.transcript = viewport.transcript;
            self.prompt_buffer = viewport.prompt_buffer;
            self.prompt_cursor = viewport.prompt_cursor.min(self.prompt_buffer.len());
            self.prompt_history = viewport.prompt_history;
            self.prompt_history_index = viewport.prompt_history_index;
            self.prompt_history_draft = viewport.prompt_history_draft;
        } else {
            self.transcript.clear();
            self.clear_prompt();
            self.prompt_history.clear();
        }
        self.run = Some(activity.run);
        self.command_result = None;
        self.session_picker = None;
        true
    }

    fn save_selected_viewport(&mut self) {
        let Some(agent_id) = self.agent_id.clone() else {
            return;
        };
        self.viewport_clock = self.viewport_clock.wrapping_add(1);
        let snapshot = ConsoleViewport {
            agent_id: agent_id.clone(),
            conversation_id: self.conversation_id.clone(),
            transcript: self
                .transcript
                .iter()
                .rev()
                .take(MAX_CONSOLE_TRANSCRIPT_TURNS)
                .cloned()
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect(),
            prompt_buffer: self.prompt_buffer.clone(),
            prompt_cursor: self.prompt_cursor,
            prompt_history: self.prompt_history.clone(),
            prompt_history_index: self.prompt_history_index,
            prompt_history_draft: self.prompt_history_draft.clone(),
            last_selected: self.viewport_clock,
        };
        if let Some(existing) = self.viewports.iter_mut().find(|viewport| {
            viewport.agent_id == agent_id && viewport.conversation_id == self.conversation_id
        }) {
            *existing = snapshot;
            return;
        }
        if self.viewports.len() >= MAX_RETAINED_CONSOLE_VIEWPORTS {
            let oldest = self
                .viewports
                .iter()
                .enumerate()
                .min_by_key(|(_, viewport)| viewport.last_selected)
                .map_or(0, |(index, _)| index);
            self.viewports.remove(oldest);
        }
        self.viewports.push(snapshot);
    }

    fn restore_viewport(
        &mut self,
        agent_id: &str,
        conversation_id: Option<&str>,
    ) -> Option<ConsoleViewport> {
        self.viewport_clock = self.viewport_clock.wrapping_add(1);
        let viewport = self.viewports.iter_mut().find(|viewport| {
            viewport.agent_id == agent_id && viewport.conversation_id.as_deref() == conversation_id
        })?;
        viewport.last_selected = self.viewport_clock;
        Some(viewport.clone())
    }

    #[must_use]
    pub fn is_command_input(&self) -> bool {
        !self.prompt_buffer.contains('\n') && self.prompt_buffer.trim_start().starts_with('/')
    }

    #[cfg(test)]
    pub fn submit_command(&mut self) -> Result<ConsoleCommandSubmission, &'static str> {
        self.submit_command_with_approval_context(None)
    }

    /// Submit a slash command using the existing scoped approval projection.
    pub fn submit_command_with_approval_context(
        &mut self,
        approval_context: Option<&ConsoleApprovalContext>,
    ) -> Result<ConsoleCommandSubmission, &'static str> {
        let raw_line = self.prompt_buffer.trim().to_owned();
        if raw_line.is_empty() {
            return Err("command cannot be empty");
        }
        if !raw_line.starts_with('/') {
            return Err("Console command input must begin with '/'");
        }
        if raw_line.contains('\n') {
            return Err("Console slash commands must fit on one line");
        }
        let Some(agent_id) = self.agent_id.clone() else {
            return Err("select an active agent before using Console commands");
        };
        let agent_name = self.agent_name.clone().unwrap_or_else(|| agent_id.clone());
        let line = redacted_console_command_line(&raw_line);
        self.record_history(&line);
        self.clear_prompt();

        let invocation = match command_registry().parse(&raw_line) {
            Ok(ParsedLine::Command(invocation)) => invocation,
            Ok(ParsedLine::Prompt(_)) => unreachable!("slash input parsed as a prompt"),
            Err(error) => {
                self.reject_command(&line, console_parse_error(&raw_line, &error.to_string()));
                return Ok(ConsoleCommandSubmission::Rejected);
            }
        };
        let context = self.command_context(approval_context);
        let recent_approval = self
            .command_result
            .as_ref()
            .and_then(|result| result.approval_resolution.as_ref());
        let exact_approval_retry =
            is_exact_recent_approval(&invocation.command, &context, recent_approval);
        if let Err(reason) = validate_console_command(
            &invocation.command,
            &context,
            approval_context,
            recent_approval,
        ) {
            self.reject_command(&line, reason);
            return Ok(ConsoleCommandSubmission::Rejected);
        }
        if command_registry()
            .resolve(invocation.command.name().as_str())
            .is_some_and(|spec| !spec.is_available(&context))
            && !exact_approval_retry
        {
            let reason = if matches!(
                invocation.command,
                InteractionCommand::Cancel {
                    target: polkagent_interaction::CancelTarget::CurrentTurn
                }
            ) {
                "/cancel is available only for the selected exact active durable turn".to_owned()
            } else {
                format!(
                    "/{} requires a selected durable Console conversation",
                    invocation.command.name().as_str()
                )
            };
            self.reject_command(&line, reason);
            return Ok(ConsoleCommandSubmission::Rejected);
        }

        let request_id = uuid::Uuid::now_v7().to_string();
        let selected_turn_id = self.selected_cancel_turn_id();
        let approval_resolution = self
            .command_result
            .as_ref()
            .and_then(|result| result.approval_resolution.clone());
        self.command_result = Some(ConsoleCommandResult {
            request_id: request_id.clone(),
            line: line.clone(),
            status: ConsoleCommandStatus::Running,
            title: "Executing shared command".to_owned(),
            lines: vec!["Waiting for the durable interaction service.".to_owned()],
            approval_resolution,
        });
        Ok(ConsoleCommandSubmission::Execute(Box::new(
            ConsoleCommandRequest {
                request_id,
                agent_id,
                agent_name,
                conversation_id: self.conversation_id.clone(),
                selected_turn_id,
                pending_approval_count: context.pending_approval_count,
                line,
                invocation,
            },
        )))
    }

    fn reject_command(&mut self, line: &str, reason: String) {
        let approval_resolution = self
            .command_result
            .as_ref()
            .and_then(|result| result.approval_resolution.clone());
        self.command_result = Some(ConsoleCommandResult {
            request_id: uuid::Uuid::now_v7().to_string(),
            line: line.to_owned(),
            status: ConsoleCommandStatus::Failed,
            title: "Command unavailable".to_owned(),
            lines: vec![reason],
            approval_resolution,
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

    fn move_slash_completion_up(
        &mut self,
        approval_context: Option<&ConsoleApprovalContext>,
    ) -> bool {
        let Some(menu) = self.slash_command_menu_with_approval_context(approval_context) else {
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

    fn move_slash_completion_down(
        &mut self,
        approval_context: Option<&ConsoleApprovalContext>,
    ) -> bool {
        let Some(menu) = self.slash_command_menu_with_approval_context(approval_context) else {
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
        if let Some(activity_id) = self.selected_activity_id.clone() {
            if let Some(activity) = self
                .activities
                .iter_mut()
                .find(|activity| activity.activity_id == activity_id)
            {
                if !activity.run.status.is_terminal() {
                    activity.run.status = ConsoleRunStatus::Cancelling;
                    "cancellation requested".clone_into(&mut activity.run.detail);
                }
            }
        }
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
                model,
                turns,
            } => {
                if self.agent_id.as_deref() != Some(agent_id.as_str())
                    || self.run.is_some()
                    || self.command_result.is_some()
                    || self.session_picker.is_some()
                {
                    return;
                }
                self.replace_conversation(conversation_id, model, turns);
            }
            ControllerEvent::HistoryFailed { .. }
            | ControllerEvent::ApprovalListLoaded { .. }
            | ControllerEvent::ApprovalListFailed { .. }
            | ControllerEvent::ApprovalDecisionCompleted { .. }
            | ControllerEvent::ApprovalDecisionFailed { .. } => {}
            ControllerEvent::CommandCompleted {
                agent_id,
                conversation_id,
                request_id,
                result,
                selection,
                model_update,
                agent_update,
            } => {
                if self.agent_id.as_deref() != Some(agent_id.as_str())
                    || self.conversation_id != conversation_id
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
                    self.replace_conversation(
                        Some(selection.conversation_id),
                        selection.model,
                        selection.turns,
                    );
                } else if let ConsoleModelUpdate::Selected(model) = model_update {
                    self.selected_model = model;
                }
                if let Some(agent) = agent_update {
                    self.agent_id = Some(agent.agent_id);
                    self.agent_name = Some(agent.agent_name);
                }
            }
            ControllerEvent::CommandFailed {
                agent_id,
                conversation_id,
                request_id,
                mut result,
            } => {
                if self.agent_id.as_deref() == Some(agent_id.as_str())
                    && self.conversation_id == conversation_id
                    && self
                        .command_result
                        .as_ref()
                        .map(|result| result.request_id.as_str())
                        == Some(request_id.as_str())
                {
                    if result.approval_resolution.is_none() {
                        result.approval_resolution = self
                            .command_result
                            .as_ref()
                            .and_then(|current| current.approval_resolution.clone());
                    }
                    self.command_result = Some(result);
                }
            }
            ControllerEvent::SessionListLoaded {
                agent_id,
                request_id,
                sessions,
            } => {
                if self.agent_id.as_deref() != Some(agent_id.as_str()) {
                    return;
                }
                let Some(picker) = &mut self.session_picker else {
                    return;
                };
                if picker.agent_id != agent_id || picker.request_id != request_id {
                    return;
                }
                picker.selected = self
                    .conversation_id
                    .as_deref()
                    .and_then(|conversation_id| {
                        sessions
                            .iter()
                            .position(|session| session.conversation_id.as_str() == conversation_id)
                    })
                    .unwrap_or(0);
                picker.sessions = sessions;
                picker.status = SessionPickerStatus::Ready;
                picker.error = None;
            }
            ControllerEvent::SessionListFailed {
                agent_id,
                request_id,
                reason,
            }
            | ControllerEvent::SessionSelectionFailed {
                agent_id,
                request_id,
                reason,
            } => {
                if self.agent_id.as_deref() == Some(agent_id.as_str()) {
                    self.fail_session_picker_request(&request_id, reason);
                }
            }
            ControllerEvent::SessionSelected {
                agent_id,
                request_id,
                selection,
            } => {
                if self.agent_id.as_deref() != Some(agent_id.as_str())
                    || self.session_picker.as_ref().is_none_or(|picker| {
                        picker.agent_id != agent_id || picker.request_id != request_id
                    })
                {
                    return;
                }
                self.session_picker = None;
                self.command_result = None;
                self.replace_conversation(
                    Some(selection.conversation_id),
                    selection.model,
                    selection.turns,
                );
            }
            event => self.apply_run_event(&event),
        }
    }

    /// Apply a controller update while retaining output for background
    /// activities that are not currently selected in the Console viewport.
    pub fn apply_update(&mut self, update: ControllerUpdate) {
        match update {
            ControllerUpdate::Control(event) => self.apply(event),
            ControllerUpdate::Activity(update) => {
                let started_conversation = match &update.event {
                    ControllerEvent::Started {
                        conversation_id, ..
                    } => Some(conversation_id.clone()),
                    _ => None,
                };
                let selected =
                    self.selected_activity_id.as_deref() == Some(update.activity_id.as_str());
                let activity = {
                    let Some(activity) = self
                        .activities
                        .iter_mut()
                        .find(|activity| activity.activity_id == update.activity_id)
                    else {
                        return;
                    };
                    if let ControllerEvent::Started {
                        conversation_id,
                        model,
                        agent_name,
                        ..
                    } = &update.event
                    {
                        activity.run.conversation_id = Some(conversation_id.clone());
                        activity.model.clone_from(model);
                        activity.agent_name.clone_from(agent_name);
                    }
                    apply_console_run_event(&mut activity.run, &update.event);
                    selected.then(|| activity.clone())
                };
                if let Some(conversation_id) = started_conversation {
                    self.rekey_viewport(
                        &update.agent_id,
                        update.requested_conversation_id.as_deref(),
                        &conversation_id,
                    );
                }
                if let Some(activity) = activity {
                    self.agent_id = Some(activity.agent_id);
                    self.agent_name = Some(activity.agent_name);
                    self.conversation_id = activity.run.conversation_id.clone();
                    self.selected_model = activity.model;
                    self.run = Some(activity.run);
                }
            }
        }
    }

    fn replace_conversation(
        &mut self,
        conversation_id: Option<String>,
        model: Option<String>,
        mut turns: Vec<ConsoleRun>,
    ) {
        self.save_selected_viewport();
        let overflow = turns.len().saturating_sub(MAX_CONSOLE_TRANSCRIPT_TURNS + 1);
        if overflow > 0 {
            turns.drain(..overflow);
        }
        self.conversation_id = conversation_id;
        self.selected_model = model;
        self.prompt_history = turns
            .iter()
            .map(|turn| bounded_grapheme_prefix(&turn.prompt, MAX_COMPOSER_BYTES).to_owned())
            .rev()
            .take(MAX_PROMPT_HISTORY)
            .collect::<Vec<_>>();
        self.prompt_history.reverse();
        self.run = turns.pop();
        self.transcript = turns;
        self.selected_activity_id = None;

        let agent_id = self.agent_id.clone();
        let selected_conversation_id = self.conversation_id.clone();
        if let Some(viewport) = agent_id.as_deref().and_then(|agent_id| {
            self.restore_viewport(agent_id, selected_conversation_id.as_deref())
        }) {
            self.prompt_buffer = viewport.prompt_buffer;
            self.prompt_cursor = viewport.prompt_cursor.min(self.prompt_buffer.len());
            self.prompt_history = viewport.prompt_history;
            self.prompt_history_index = viewport.prompt_history_index;
            self.prompt_history_draft = viewport.prompt_history_draft;
        }

        let matching_activity = self.activities.iter().rev().find(|activity| {
            Some(activity.agent_id.as_str()) == self.agent_id.as_deref()
                && activity.run.conversation_id == self.conversation_id
        });
        if let Some(activity) = matching_activity {
            self.selected_activity_id = Some(activity.activity_id.clone());
            self.agent_name = Some(activity.agent_name.clone());
            self.selected_model.clone_from(&activity.model);
            self.run = Some(activity.run.clone());
        }
    }

    fn rekey_viewport(
        &mut self,
        agent_id: &str,
        requested_conversation_id: Option<&str>,
        conversation_id: &str,
    ) {
        if requested_conversation_id == Some(conversation_id) {
            return;
        }
        if let Some(viewport) = self.viewports.iter_mut().find(|viewport| {
            viewport.agent_id == agent_id
                && viewport.conversation_id.as_deref() == requested_conversation_id
        }) {
            viewport.conversation_id = Some(conversation_id.to_owned());
        }
    }

    fn apply_run_event(&mut self, event: &ControllerEvent) {
        let Some(run) = &mut self.run else {
            return;
        };
        if let ControllerEvent::Started {
            conversation_id,
            model,
            agent_name,
            ..
        } = &event
        {
            self.conversation_id = Some(conversation_id.clone());
            self.selected_model.clone_from(model);
            self.agent_name = Some(agent_name.clone());
        }
        apply_console_run_event(run, event);
    }
}

fn apply_console_run_event(run: &mut ConsoleRun, event: &ControllerEvent) {
    match event {
        ControllerEvent::Started {
            conversation_id,
            turn_id,
            run_id,
            notes,
            ..
        } => {
            run.conversation_id = Some(conversation_id.clone());
            run.turn_id = Some(turn_id.clone());
            run.run_id = Some(run_id.clone());
            run.status = ConsoleRunStatus::Running;
            run.detail = if notes.is_empty() {
                "run started".to_owned()
            } else {
                notes.join(" ")
            };
        }
        ControllerEvent::Output(text) => append_bounded_output(&mut run.output, text),
        ControllerEvent::Progress(detail) => run.detail.clone_from(detail),
        ControllerEvent::TurnApprovalRequested { approval_id } => {
            run.detail = format!(
                "approval {approval_id} is pending; use F6 to approve or deny before cancelling"
            );
        }
        ControllerEvent::TurnApprovalResolved {
            approval_id,
            decision,
        } => {
            run.detail = format!("approval {approval_id} resolved: {decision:?}");
        }
        ControllerEvent::UsageUpdated {
            input_tokens,
            output_tokens,
        } => {
            run.input_tokens = *input_tokens;
            run.output_tokens = *output_tokens;
        }
        ControllerEvent::Completed {
            text,
            input_tokens,
            output_tokens,
        } => {
            run.status = ConsoleRunStatus::Completed;
            "run completed".clone_into(&mut run.detail);
            replace_bounded_output(&mut run.output, text);
            run.input_tokens = *input_tokens;
            run.output_tokens = *output_tokens;
        }
        ControllerEvent::Failed(reason) => {
            run.status = ConsoleRunStatus::Failed;
            run.detail.clone_from(reason);
        }
        ControllerEvent::Cancelled(reason) => {
            run.status = ConsoleRunStatus::Cancelled;
            run.detail.clone_from(reason);
        }
        ControllerEvent::TimedOut => {
            run.status = ConsoleRunStatus::TimedOut;
            "run timed out".clone_into(&mut run.detail);
        }
        ControllerEvent::HistoryLoaded { .. }
        | ControllerEvent::HistoryFailed { .. }
        | ControllerEvent::CommandCompleted { .. }
        | ControllerEvent::CommandFailed { .. }
        | ControllerEvent::SessionListLoaded { .. }
        | ControllerEvent::SessionListFailed { .. }
        | ControllerEvent::SessionSelected { .. }
        | ControllerEvent::SessionSelectionFailed { .. }
        | ControllerEvent::ApprovalListLoaded { .. }
        | ControllerEvent::ApprovalListFailed { .. }
        | ControllerEvent::ApprovalDecisionCompleted { .. }
        | ControllerEvent::ApprovalDecisionFailed { .. } => {}
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
        .grapheme_indices(true)
        .find_map(|(index, _)| (index >= overflow).then_some(index))
        .unwrap_or(output.len());
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

fn validate_console_command(
    command: &InteractionCommand,
    context: &CommandContext,
    approval_context: Option<&ConsoleApprovalContext>,
    recent_approval: Option<&ConsoleApprovalResolution>,
) -> Result<(), String> {
    match command {
        InteractionCommand::Help {
            command: Some(command),
        } if !SUPPORTED_CONSOLE_COMMANDS.contains(command) => Err(format!(
            "/{} is not supported in the Console",
            command.as_str()
        )),
        InteractionCommand::Cancel {
            target: polkagent_interaction::CancelTarget::Run(_)
                | polkagent_interaction::CancelTarget::All,
        } => Err(
            "the Console supports only /cancel (or /stop) for the selected exact active turn; run-ID and all-activity cancellation are unavailable"
                .to_owned(),
        ),
        InteractionCommand::Help {
            command: Some(CommandName::Approve | CommandName::Deny),
        } if context.pending_approval_count == 0 => Err(
            "approval commands are available only when the selected Console conversation has an exact pending request projected by the authorized F6 queue"
                .to_owned(),
        ),
        InteractionCommand::Deny {
            reason: Some(reason),
            ..
        } if reason.chars().count() > MAX_DENIAL_REASON_CHARS => Err(format!(
            "approval denial reason must be at most {MAX_DENIAL_REASON_CHARS} characters"
        )),
        InteractionCommand::Approve { approval_id }
        | InteractionCommand::Deny { approval_id, .. } => {
            let Some(approval_context) = approval_context else {
                return Err(
                    "approval commands are unavailable until the authorized F6 queue has projected the selected Console conversation"
                        .to_owned(),
                );
            };
            if !approval_context.matches_conversation(context.conversation_id) {
                return Err(
                    "the projected approval queue is stale for the selected Console conversation; refresh F6 before deciding"
                        .to_owned(),
                );
            }
            let exact_recent_retry =
                is_exact_recent_approval(command, context, recent_approval);
            if !approval_context.contains(*approval_id) && !exact_recent_retry {
                return Err(
                    "the exact full approval ID is neither pending in the selected conversation's authorized service projection nor the most recently resolved Console approval"
                        .to_owned(),
                );
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn is_exact_recent_approval(
    command: &InteractionCommand,
    context: &CommandContext,
    recent_approval: Option<&ConsoleApprovalResolution>,
) -> bool {
    let approval_id = match command {
        InteractionCommand::Approve { approval_id }
        | InteractionCommand::Deny { approval_id, .. } => *approval_id,
        _ => return false,
    };
    recent_approval.is_some_and(|recent| {
        Some(recent.conversation_id) == context.conversation_id && recent.approval_id == approval_id
    })
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
        "harness" => {
            "harness selection is unavailable in the Console; configure the runtime and restart"
                .to_owned()
        }
        _ => shared_error.to_owned(),
    }
}

fn redacted_console_command_line(line: &str) -> String {
    let mut arguments = line.split_whitespace();
    let Some(command) = arguments.next() else {
        return String::new();
    };
    if !command.eq_ignore_ascii_case("/deny") {
        return line.to_owned();
    }
    let Some(approval_id) = arguments.next() else {
        return "/deny".to_owned();
    };
    if arguments.next().is_some() {
        format!("/deny {approval_id} [reason redacted]")
    } else {
        format!("/deny {approval_id}")
    }
}

fn slash_candidate(spec: &polkagent_interaction::CommandSpec) -> SlashCommandCandidate {
    SlashCommandCandidate {
        name: spec.name.clone(),
        aliases: spec.aliases.clone(),
        description: if spec.command == CommandName::Cancel {
            "Cancel the selected exact active Console turn".to_owned()
        } else {
            spec.description.clone()
        },
        input_hint: (spec.command != CommandName::Cancel)
            .then(|| spec.input_hint.clone())
            .flatten(),
    }
}

fn clamp_grapheme_boundary(text: &str, cursor: usize) -> usize {
    let cursor = cursor.min(text.len());
    if cursor == text.len() {
        return cursor;
    }
    text.grapheme_indices(true)
        .map(|(index, _)| index)
        .rfind(|index| *index <= cursor)
        .unwrap_or(0)
}

fn ceil_grapheme_boundary(text: &str, cursor: usize) -> usize {
    if cursor >= text.len() {
        return text.len();
    }
    text.grapheme_indices(true)
        .map(|(index, _)| index)
        .find(|index| *index >= cursor)
        .unwrap_or(text.len())
}

fn previous_grapheme_boundary(text: &str, cursor: usize) -> usize {
    let cursor = clamp_grapheme_boundary(text, cursor);
    text[..cursor]
        .grapheme_indices(true)
        .next_back()
        .map_or(cursor, |(index, _)| index)
}

fn next_grapheme_boundary(text: &str, cursor: usize) -> usize {
    let cursor = clamp_grapheme_boundary(text, cursor);
    text[cursor..]
        .graphemes(true)
        .next()
        .map_or(cursor, |grapheme| cursor + grapheme.len())
}

fn grapheme_column_offset(line: &str, column: usize) -> usize {
    line.grapheme_indices(true)
        .nth(column)
        .map_or(line.len(), |(index, _)| index)
}

fn bounded_grapheme_prefix(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }
    let end = text
        .grapheme_indices(true)
        .take_while(|(index, grapheme)| index.saturating_add(grapheme.len()) <= max_bytes)
        .last()
        .map_or(0, |(index, grapheme)| index + grapheme.len());
    &text[..end]
}

fn normalize_bounded_paste(pasted: &str, max_bytes: usize) -> (String, bool) {
    let mut normalized = String::with_capacity(pasted.len().min(max_bytes));
    let mut consumed = 0;
    for grapheme in pasted.graphemes(true) {
        let normalized_len = normalized_paste_grapheme_len(grapheme);
        if normalized.len().saturating_add(normalized_len) > max_bytes {
            return (normalized, true);
        }
        append_normalized_paste_grapheme(&mut normalized, grapheme);
        consumed += grapheme.len();
    }
    (normalized, consumed < pasted.len())
}

fn normalized_paste_grapheme_len(grapheme: &str) -> usize {
    let mut length = 0;
    let mut chars = grapheme.chars().peekable();
    while let Some(character) = chars.next() {
        match character {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                length += 1;
            }
            '\n' => length += 1,
            '\t' => length += 4,
            control if control.is_control() => {}
            printable => length += printable.len_utf8(),
        }
    }
    length
}

fn append_normalized_paste_grapheme(output: &mut String, grapheme: &str) {
    let mut chars = grapheme.chars().peekable();
    while let Some(character) = chars.next() {
        match character {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                output.push('\n');
            }
            '\n' => output.push('\n'),
            '\t' => output.push_str("    "),
            control if control.is_control() => {}
            printable => output.push(printable),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControllerEvent {
    HistoryLoaded {
        agent_id: String,
        conversation_id: Option<String>,
        model: Option<String>,
        turns: Vec<ConsoleRun>,
    },
    HistoryFailed {
        agent_id: String,
        reason: String,
    },
    CommandCompleted {
        agent_id: String,
        conversation_id: Option<String>,
        request_id: String,
        result: ConsoleCommandResult,
        selection: Option<ConsoleConversationSelection>,
        model_update: ConsoleModelUpdate,
        agent_update: Option<ConsoleAgentSelection>,
    },
    CommandFailed {
        agent_id: String,
        conversation_id: Option<String>,
        request_id: String,
        result: ConsoleCommandResult,
    },
    SessionListLoaded {
        agent_id: String,
        request_id: String,
        sessions: Vec<ConsoleSessionItem>,
    },
    SessionListFailed {
        agent_id: String,
        request_id: String,
        reason: String,
    },
    SessionSelected {
        agent_id: String,
        request_id: String,
        selection: ConsoleConversationSelection,
    },
    SessionSelectionFailed {
        agent_id: String,
        request_id: String,
        reason: String,
    },
    ApprovalListLoaded {
        request_id: uuid::Uuid,
        conversation_id: ConversationId,
        approvals: Vec<ApprovalView>,
        total_count: usize,
    },
    ApprovalListFailed {
        request_id: uuid::Uuid,
        conversation_id: ConversationId,
        code: InteractionErrorCode,
        reason: String,
    },
    ApprovalDecisionCompleted {
        request_id: uuid::Uuid,
        conversation_id: ConversationId,
        approval_id: ApprovalId,
        decision: ApprovalDecision,
        approval: ApprovalView,
    },
    ApprovalDecisionFailed {
        request_id: uuid::Uuid,
        conversation_id: ConversationId,
        approval_id: ApprovalId,
        decision: ApprovalDecision,
        code: InteractionErrorCode,
        reason: String,
    },
    Started {
        conversation_id: String,
        model: Option<String>,
        turn_id: String,
        run_id: String,
        agent_name: String,
        notes: Vec<String>,
    },
    Output(String),
    Progress(String),
    TurnApprovalRequested {
        approval_id: ApprovalId,
    },
    TurnApprovalResolved {
        approval_id: ApprovalId,
        decision: ApprovalDecision,
    },
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

/// Correlated run update retained even when another Console session is shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunActivityUpdate {
    pub activity_id: String,
    pub agent_id: String,
    pub requested_conversation_id: Option<String>,
    pub event: ControllerEvent,
}

/// One item from the bounded controller channels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControllerUpdate {
    Control(ControllerEvent),
    Activity(RunActivityUpdate),
}

impl ControllerUpdate {
    #[must_use]
    pub fn event(&self) -> &ControllerEvent {
        match self {
            Self::Control(event) => event,
            Self::Activity(update) => &update.event,
        }
    }

    #[must_use]
    pub fn activity_id(&self) -> Option<&str> {
        match self {
            Self::Control(_) => None,
            Self::Activity(update) => Some(&update.activity_id),
        }
    }

    fn into_event(self) -> ControllerEvent {
        match self {
            Self::Control(event) => event,
            Self::Activity(update) => update.event,
        }
    }
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
                | Self::SessionListLoaded { .. }
                | Self::SessionListFailed { .. }
                | Self::SessionSelected { .. }
                | Self::SessionSelectionFailed { .. }
                | Self::ApprovalListLoaded { .. }
                | Self::ApprovalListFailed { .. }
                | Self::ApprovalDecisionCompleted { .. }
                | Self::ApprovalDecisionFailed { .. }
        )
    }
}

/// Runtime bridge owned by `App`. It can be driven without blocking the
/// Crossterm event loop and exposes plain events for deterministic reduction.
#[derive(Clone)]
struct RunActivityContext {
    activity: String,
    agent: String,
    requested_conversation: Option<String>,
}

impl RunActivityContext {
    fn update(&self, event: ControllerEvent) -> RunActivityUpdate {
        RunActivityUpdate {
            activity_id: self.activity.clone(),
            agent_id: self.agent.clone(),
            requested_conversation_id: self.requested_conversation.clone(),
            event,
        }
    }
}

struct ActiveConsoleRun {
    agent_id: String,
    conversation_id: Option<String>,
    cancel: Option<tokio::sync::oneshot::Sender<()>>,
}

#[cfg(test)]
type TestRunWorker = Arc<
    dyn Fn(
            PromptRequest,
            RunActivityContext,
            mpsc::Sender<RunActivityUpdate>,
            tokio::sync::oneshot::Receiver<()>,
        ) -> BoxFuture<'static, ()>
        + Send
        + Sync,
>;

pub struct RunController {
    task_runtime: Option<tokio::runtime::Handle>,
    polkagent_runtime: PolkagentRuntime,
    approval_service: Arc<dyn InteractionService>,
    event_tx: mpsc::Sender<ControllerEvent>,
    event_rx: mpsc::Receiver<ControllerEvent>,
    run_event_tx: mpsc::Sender<RunActivityUpdate>,
    run_event_rx: mpsc::Receiver<RunActivityUpdate>,
    active_runs: BTreeMap<String, ActiveConsoleRun>,
    control_active: bool,
    tasks: Vec<tokio::task::JoinHandle<()>>,
    shutdown_grace: std::time::Duration,
    #[cfg(test)]
    test_run_worker: Option<TestRunWorker>,
}

impl RunController {
    #[must_use]
    pub fn new(polkagent_runtime: PolkagentRuntime) -> Self {
        let (event_tx, event_rx) = mpsc::channel(CONTROLLER_EVENT_CAPACITY);
        let (run_event_tx, run_event_rx) = mpsc::channel(CONTROLLER_EVENT_CAPACITY);
        let approval_service: Arc<dyn InteractionService> =
            polkagent_runtime.interactions().clone();
        Self {
            task_runtime: tokio::runtime::Handle::try_current().ok(),
            polkagent_runtime,
            approval_service,
            event_tx,
            event_rx,
            run_event_tx,
            run_event_rx,
            active_runs: BTreeMap::new(),
            control_active: false,
            tasks: Vec::new(),
            shutdown_grace: CONTROLLER_SHUTDOWN_GRACE,
            #[cfg(test)]
            test_run_worker: None,
        }
    }

    #[cfg(test)]
    fn with_test_run_worker(
        polkagent_runtime: PolkagentRuntime,
        worker: TestRunWorker,
        shutdown_grace: std::time::Duration,
    ) -> Self {
        let mut controller = Self::new(polkagent_runtime);
        controller.test_run_worker = Some(worker);
        controller.shutdown_grace = shutdown_grace;
        controller
    }

    #[cfg(test)]
    fn with_test_approval_service(mut self, service: Arc<dyn InteractionService>) -> Self {
        self.approval_service = service;
        self
    }

    fn track_task(&mut self, task: tokio::task::JoinHandle<()>) {
        self.tasks.retain(|task| !task.is_finished());
        self.tasks.push(task);
    }

    #[allow(
        clippy::too_many_lines,
        reason = "the prompt task keeps cancellation, durable subscription recovery, bounded event delivery, and terminal-state ordering together"
    )]
    pub fn start(&mut self, request: PromptRequest) -> Result<String, &'static str> {
        if self.active_runs.len() >= MAX_CONCURRENT_CONSOLE_RUNS {
            return Err("Console concurrent-run capacity is full");
        }
        if self.active_runs.values().any(|run| {
            run.agent_id == request.agent_id && run.conversation_id == request.conversation_id
        }) {
            return Err("this conversation already has an active Console turn");
        }
        let Some(task_runtime) = self.task_runtime.clone() else {
            return Err("interactive run runtime is unavailable");
        };

        let activity_id = uuid::Uuid::now_v7().to_string();
        let context = RunActivityContext {
            activity: activity_id.clone(),
            agent: request.agent_id.clone(),
            requested_conversation: request.conversation_id.clone(),
        };
        let polkagent_runtime = self.polkagent_runtime.clone();
        let event_tx = self.run_event_tx.clone();
        let (cancel_tx, mut cancel_rx) = tokio::sync::oneshot::channel();
        self.active_runs.insert(
            activity_id.clone(),
            ActiveConsoleRun {
                agent_id: request.agent_id.clone(),
                conversation_id: request.conversation_id.clone(),
                cancel: Some(cancel_tx),
            },
        );

        #[cfg(test)]
        if let Some(worker) = self.test_run_worker.clone() {
            let task = task_runtime.spawn(worker(request, context, event_tx, cancel_rx));
            self.track_task(task);
            return Ok(activity_id);
        }

        let task = task_runtime.spawn(async move {
            let PromptRequest {
                agent_id,
                agent_name,
                conversation_id,
                prompt,
            } = request;
            let typed_agent_id = match parse_selected_agent_id(&agent_id) {
                Ok(agent_id) => agent_id,
                Err(reason) => {
                    let _ = event_tx.send(context.update(ControllerEvent::Failed(reason))).await;
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
                    let _ = event_tx
                        .send(context.update(ControllerEvent::Failed(error.to_string())))
                        .await;
                    return;
                }
            };
            let client_context = match tui_client_context(&polkagent_runtime) {
                Ok(context) => context,
                Err(error) => {
                    let _ = event_tx
                        .send(context.update(ControllerEvent::Failed(error.to_string())))
                        .await;
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
                        let _ = event_tx.send(context.update(ControllerEvent::Progress(
                            "cancellation queued until the durable turn is ready".to_owned(),
                        ))).await;
                    }
                    result = &mut prompt => match result {
                        Ok(started) => break started,
                        Err(error) => {
                            let _ = event_tx
                                .send(context.update(ControllerEvent::Failed(error.to_string())))
                                .await;
                            return;
                        }
                    }
                }
            };
            let Some(run_id) = started.handle.run_ids.first().copied() else {
                let _ = event_tx
                    .send(context.update(ControllerEvent::Failed(
                        "durable interaction turn has no linked run".to_owned(),
                    )))
                    .await;
                return;
            };
            let mut events = started.events;
            let notes = runtime_notes(polkagent_runtime.readiness());
            let _ = event_tx
                .send(context.update(started_controller_event(
                    &interaction,
                    turn_id,
                    run_id,
                    agent_name,
                    notes,
                )))
                .await;
            if cancellation_requested {
                request_turn_cancellation(service.as_ref(), turn_id, &event_tx, &context).await;
            }
            loop {
                let envelope = tokio::select! {
                    biased;
                    _ = &mut cancel_rx, if !cancellation_requested => {
                        cancellation_requested = true;
                        request_turn_cancellation(service.as_ref(), turn_id, &event_tx, &context).await;
                        continue;
                    }
                    result = events.recv() => match result {
                        Ok(event) => event,
                        Err(StreamError::Lagged { last_seen_sequence, resume_after_sequence }) => {
                            let _ = event_tx
                                .send(context.update(ControllerEvent::Progress(format!(
                                    "interaction stream lagged at sequence {resume_after_sequence}; replaying durable events"
                                ))))
                                .await;
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
                                    let _ = event_tx
                                        .send(context.update(ControllerEvent::Failed(format!(
                                            "resubscribe to durable interaction events: {error}"
                                        ))))
                                        .await;
                                    return;
                                }
                            }
                            continue;
                        }
                        Err(StreamError::Backend(error)) => {
                            let _ = event_tx
                                .send(context.update(ControllerEvent::Failed(format!(
                                    "interaction event stream failed: {error}"
                                ))))
                                .await;
                            return;
                        }
                        Err(StreamError::Closed) => {
                            let _ = event_tx
                                .send(context.update(ControllerEvent::Failed(
                                    "interaction event stream closed before a terminal event"
                                        .to_owned(),
                                )))
                                .await;
                            return;
                        }
                    }
                };

                let projected = project_interaction_event(envelope.event);
                let terminal = projected.is_terminal();
                if event_tx.send(context.update(projected)).await.is_err() || terminal {
                    return;
                }
            }
        });
        self.track_task(task);

        Ok(activity_id)
    }

    #[allow(
        clippy::too_many_lines,
        reason = "the async command boundary keeps validation, execution, and correlated completion together"
    )]
    pub fn execute_command(
        &mut self,
        request: Box<ConsoleCommandRequest>,
    ) -> Result<(), &'static str> {
        if self.control_active {
            return Err("a Console action is already active");
        }
        let Some(task_runtime) = self.task_runtime.clone() else {
            return Err("interactive command runtime is unavailable");
        };
        self.control_active = true;
        let polkagent_runtime = self.polkagent_runtime.clone();
        let interaction_service = Arc::clone(&self.approval_service);
        let event_tx = self.event_tx.clone();
        let task = task_runtime.spawn(async move {
            let ConsoleCommandRequest {
                request_id,
                agent_id,
                agent_name: _,
                conversation_id,
                selected_turn_id,
                pending_approval_count,
                line,
                invocation,
            } = *request;
            let selected_conversation_id = conversation_id.clone();
            let typed_agent_id = match agent_id.parse::<AgentId>() {
                Ok(agent_id) => agent_id,
                Err(error) => {
                    send_command_failure(
                        &event_tx,
                        agent_id,
                        selected_conversation_id,
                        request_id,
                        line,
                        format!("invalid selected agent ID: {error}"),
                    )
                    .await;
                    return;
                }
            };
            let conversation_id = match conversation_id
                .as_deref()
                .map(|id| {
                    id.parse::<ConversationId>().map_err(|error| {
                        format!("invalid selected Console conversation ID '{id}': {error}")
                    })
                })
                .transpose()
            {
                Ok(conversation_id) => conversation_id,
                Err(error) => {
                    send_command_failure(
                        &event_tx,
                        agent_id,
                        selected_conversation_id,
                        request_id,
                        line,
                        error,
                    )
                    .await;
                    return;
                }
            };
            let current_turn_cancel = matches!(
                invocation.command,
                InteractionCommand::Cancel {
                    target: polkagent_interaction::CancelTarget::CurrentTurn
                }
            );
            let approval_command = matches!(
                invocation.command,
                InteractionCommand::Approve { .. } | InteractionCommand::Deny { .. }
            );
            let selected_has_active_turn = selected_turn_id.is_some();
            let selected_turn_id = selected_turn_id.filter(|_| current_turn_cancel);
            if current_turn_cancel && selected_turn_id.is_none() {
                send_command_failure(
                    &event_tx,
                    agent_id,
                    selected_conversation_id,
                    request_id,
                    line,
                    "the selected Console activity no longer has an exact cancellable durable turn"
                        .to_owned(),
                )
                .await;
                return;
            }
            let service = interaction_service;
            if matches!(&invocation.command, InteractionCommand::Agent { .. }) {
                let Some(selected) = conversation_id else {
                    send_command_failure(
                        &event_tx,
                        agent_id,
                        selected_conversation_id,
                        request_id,
                        line,
                        "/agent requires a selected durable Console conversation".to_owned(),
                    )
                    .await;
                    return;
                };
                match service.list_turns(selected).await {
                    Ok(turns) if turns.iter().any(|turn| !turn.state.is_terminal()) => {
                        send_command_failure(
                            &event_tx,
                            agent_id,
                            selected_conversation_id,
                            request_id,
                            line,
                            "cannot change agents while this durable conversation has active work; cancel or wait for it to finish".to_owned(),
                        )
                        .await;
                        return;
                    }
                    Ok(_) => {}
                    Err(error) => {
                        send_command_failure(
                            &event_tx,
                            agent_id,
                            selected_conversation_id,
                            request_id,
                            line,
                            format!("checking durable active work before changing agents: {error}"),
                        )
                        .await;
                        return;
                    }
                }
            }
            let command_runtime: Arc<dyn InteractionCommandRuntime> = Arc::new(TuiCommandRuntime {
                service: Arc::clone(&service),
                pool: polkagent_runtime.pool().clone(),
                run_commands: RunCommandReadModel::new(polkagent_runtime.pool().clone()),
                agent_id: typed_agent_id,
                exact_turn_filter: selected_turn_id,
                approval_authority_bound: polkagent_runtime
                    .interactions()
                    .approval_authority_bound(),
            });
            let executor = match ServiceCommandExecutor::new(
                command_registry().clone(),
                service,
                command_runtime,
            ) {
                Ok(executor) => executor,
                Err(error) => {
                    send_command_failure(
                        &event_tx,
                        agent_id,
                        selected_conversation_id,
                        request_id,
                        line,
                        error.to_string(),
                    )
                    .await;
                    return;
                }
            };
            let client_context = match tui_client_context(&polkagent_runtime) {
                Ok(context) => context,
                Err(error) => {
                    send_command_failure(
                        &event_tx,
                        agent_id,
                        selected_conversation_id,
                        request_id,
                        line,
                        error.to_string(),
                    )
                    .await;
                    return;
                }
            };
            let output = executor
                .execute(CommandRequest {
                    invocation,
                    context: CommandContext {
                        conversation_id,
                        has_active_turn: selected_has_active_turn,
                        pending_approval_count,
                        can_mutate: true,
                    },
                    client_context,
                })
                .await;
            let outcome = match output {
                Ok(output) => {
                    let approval_resolution = match (&output, conversation_id) {
                        (
                            CommandOutput::ApprovalResolved {
                                approval_id,
                                decision,
                            },
                            Some(conversation_id),
                        ) => Some(ConsoleApprovalResolution {
                            conversation_id,
                            approval_id: *approval_id,
                            decision: match decision {
                                ApprovalDecision::Approve => ConsoleApprovalDecision::Approve,
                                ApprovalDecision::Deny { .. } => ConsoleApprovalDecision::Deny,
                            },
                        }),
                        _ => None,
                    };
                    project_console_command_output(
                        &polkagent_runtime,
                        typed_agent_id,
                        output,
                    )
                    .await
                    .map(|outcome| (outcome, approval_resolution))
                }
                Err(error) if approval_command => {
                    let (code, reason) = safe_approval_error(&error);
                    Err(format!("{code}: {reason}"))
                }
                Err(error) => Err(error.to_string()),
            };
            match outcome {
                Ok((outcome, approval_resolution)) => {
                    let result = ConsoleCommandResult {
                        request_id: request_id.clone(),
                        line,
                        status: ConsoleCommandStatus::Completed,
                        title: outcome.title,
                        lines: outcome.lines,
                        approval_resolution,
                    };
                    let _ = event_tx
                        .send(ControllerEvent::CommandCompleted {
                            agent_id,
                            conversation_id: selected_conversation_id,
                            request_id,
                            result,
                            selection: outcome.selection,
                            model_update: outcome.model_update,
                            agent_update: outcome.agent_update,
                        })
                        .await;
                }
                Err(error) => {
                    send_command_failure(
                        &event_tx,
                        agent_id,
                        selected_conversation_id,
                        request_id,
                        line,
                        error,
                    )
                    .await;
                }
            }
        });
        self.track_task(task);
        Ok(())
    }

    pub fn list_sessions(&mut self, request: SessionListRequest) -> Result<(), &'static str> {
        if self.control_active {
            return Err("a Console action is already active");
        }
        let Some(task_runtime) = self.task_runtime.clone() else {
            return Err("interactive session selector runtime is unavailable");
        };
        self.control_active = true;
        let polkagent_runtime = self.polkagent_runtime.clone();
        let event_tx = self.event_tx.clone();
        let task = task_runtime.spawn(async move {
            let SessionListRequest {
                request_id,
                agent_id,
            } = request;
            let typed_agent_id = match agent_id.parse::<AgentId>() {
                Ok(agent_id) => agent_id,
                Err(error) => {
                    let _ = event_tx
                        .send(ControllerEvent::SessionListFailed {
                            agent_id,
                            request_id,
                            reason: format!("invalid selected agent ID: {error}"),
                        })
                        .await;
                    return;
                }
            };
            let result = polkagent_runtime
                .interactions()
                .list_interactions(ListInteractionsRequest {
                    limit: MAX_SESSION_SELECTOR_SCAN,
                    offset: 0,
                    state: None,
                })
                .await
                .map(|interactions| project_session_items(interactions, typed_agent_id));
            match result {
                Ok(sessions) => {
                    let _ = event_tx
                        .send(ControllerEvent::SessionListLoaded {
                            agent_id,
                            request_id,
                            sessions,
                        })
                        .await;
                }
                Err(error) => {
                    let _ = event_tx
                        .send(ControllerEvent::SessionListFailed {
                            agent_id,
                            request_id,
                            reason: format!("list durable Console sessions: {error}"),
                        })
                        .await;
                }
            }
        });
        self.track_task(task);
        Ok(())
    }

    pub fn load_session(&mut self, request: SessionLoadRequest) -> Result<(), &'static str> {
        if self.control_active {
            return Err("a Console action is already active");
        }
        let Some(task_runtime) = self.task_runtime.clone() else {
            return Err("interactive session selector runtime is unavailable");
        };
        self.control_active = true;
        let polkagent_runtime = self.polkagent_runtime.clone();
        let event_tx = self.event_tx.clone();
        let task = task_runtime.spawn(async move {
            let SessionLoadRequest {
                request_id,
                agent_id,
                conversation: conversation_id,
            } = request;
            let result = async {
                let typed_agent_id = parse_selected_agent_id(&agent_id)?;
                let conversation_id =
                    conversation_id.parse::<ConversationId>().map_err(|error| {
                        format!(
                            "invalid selected Console conversation ID '{conversation_id}': {error}"
                        )
                    })?;
                let interaction = polkagent_runtime
                    .interactions()
                    .load_interaction(conversation_id)
                    .await
                    .map_err(|error| format!("load durable Console session: {error}"))?;
                validate_console_target(&interaction, typed_agent_id)
                    .map_err(|error| error.to_string())?;
                let (_, turns) = load_interaction_history(&polkagent_runtime, &interaction).await?;
                Ok::<_, String>(ConsoleConversationSelection {
                    conversation_id: interaction.conversation_id.to_string(),
                    model: interaction.config.model.clone(),
                    turns,
                })
            }
            .await;
            match result {
                Ok(selection) => {
                    let _ = event_tx
                        .send(ControllerEvent::SessionSelected {
                            agent_id,
                            request_id,
                            selection,
                        })
                        .await;
                }
                Err(reason) => {
                    let _ = event_tx
                        .send(ControllerEvent::SessionSelectionFailed {
                            agent_id,
                            request_id,
                            reason,
                        })
                        .await;
                }
            }
        });
        self.track_task(task);
        Ok(())
    }

    /// Load the durable TUI interaction for a selected agent without blocking
    /// input or rendering. A stale result is ignored by the reducer.
    pub fn load_history(&mut self, agent_id: String) -> Result<(), &'static str> {
        let Some(task_runtime) = self.task_runtime.clone() else {
            return Err("interactive run runtime is unavailable");
        };
        let polkagent_runtime = self.polkagent_runtime.clone();
        let event_tx = self.event_tx.clone();
        let task = task_runtime.spawn(async move {
            let typed_agent_id = match agent_id.parse::<AgentId>() {
                Ok(agent_id) => agent_id,
                Err(error) => {
                    let _ = event_tx
                        .send(ControllerEvent::HistoryFailed {
                            agent_id,
                            reason: format!("invalid selected agent ID: {error}"),
                        })
                        .await;
                    return;
                }
            };
            match load_console_history(&polkagent_runtime, typed_agent_id).await {
                Ok((conversation_id, model, turns)) => {
                    let _ = event_tx
                        .send(ControllerEvent::HistoryLoaded {
                            agent_id,
                            conversation_id: conversation_id.map(|id| id.to_string()),
                            model,
                            turns,
                        })
                        .await;
                }
                Err(error) => {
                    let _ = event_tx
                        .send(ControllerEvent::HistoryFailed {
                            agent_id,
                            reason: error,
                        })
                        .await;
                }
            }
        });
        self.track_task(task);
        Ok(())
    }

    /// List pending approvals through the authenticated shared service.
    /// Completion is correlated to the exact durable conversation and never
    /// performs database I/O on the terminal thread.
    pub fn list_approvals(&mut self, request: ApprovalListRequest) -> Result<(), &'static str> {
        if self.control_active {
            return Err("another Console or approval action is already active");
        }
        let Some(task_runtime) = self.task_runtime.clone() else {
            return Err("interactive approval runtime is unavailable");
        };
        self.control_active = true;
        let service = Arc::clone(&self.approval_service);
        let event_tx = self.event_tx.clone();
        let task = task_runtime.spawn(async move {
            match service
                .list_pending_approvals(request.conversation_id)
                .await
            {
                Ok(mut approvals) => {
                    let total_count = approvals.len();
                    approvals.truncate(MAX_PENDING_APPROVALS);
                    let _ = event_tx
                        .send(ControllerEvent::ApprovalListLoaded {
                            request_id: request.request_id,
                            conversation_id: request.conversation_id,
                            approvals,
                            total_count,
                        })
                        .await;
                }
                Err(error) => {
                    let (code, reason) = safe_approval_error(&error);
                    let _ = event_tx
                        .send(ControllerEvent::ApprovalListFailed {
                            request_id: request.request_id,
                            conversation_id: request.conversation_id,
                            code,
                            reason,
                        })
                        .await;
                }
            }
        });
        self.track_task(task);
        Ok(())
    }

    /// Resolve one exact approval request through the shared durable CAS.
    pub fn decide_approval(
        &mut self,
        request: ApprovalDecisionRequest,
    ) -> Result<(), &'static str> {
        if self.control_active {
            return Err("another Console or approval action is already active");
        }
        let Some(task_runtime) = self.task_runtime.clone() else {
            return Err("interactive approval runtime is unavailable");
        };
        self.control_active = true;
        let service = Arc::clone(&self.approval_service);
        let event_tx = self.event_tx.clone();
        let task = task_runtime.spawn(async move {
            let result = match &request.decision {
                ApprovalDecision::Approve => {
                    service
                        .approve(request.conversation_id, request.approval_id)
                        .await
                }
                ApprovalDecision::Deny { reason } => {
                    service
                        .deny(request.conversation_id, request.approval_id, reason.clone())
                        .await
                }
            };
            match result {
                Ok(approval) => {
                    let _ = event_tx
                        .send(ControllerEvent::ApprovalDecisionCompleted {
                            request_id: request.request_id,
                            conversation_id: request.conversation_id,
                            approval_id: request.approval_id,
                            decision: request.decision,
                            approval,
                        })
                        .await;
                }
                Err(error) => {
                    let (code, reason) = safe_approval_error(&error);
                    let _ = event_tx
                        .send(ControllerEvent::ApprovalDecisionFailed {
                            request_id: request.request_id,
                            conversation_id: request.conversation_id,
                            approval_id: request.approval_id,
                            decision: request.decision,
                            code,
                            reason,
                        })
                        .await;
                }
            }
        });
        self.track_task(task);
        Ok(())
    }

    pub fn cancel_activity(&mut self, activity_id: &str) -> bool {
        self.active_runs
            .get_mut(activity_id)
            .and_then(|run| run.cancel.take())
            .is_some_and(|cancel| cancel.send(()).is_ok())
    }

    /// Compatibility helper for single-run callers. Multi-run surfaces must
    /// use [`Self::cancel_activity`] with the selected stable identity.
    #[allow(
        dead_code,
        reason = "single-run compatibility tests and external CLI fixtures use this helper"
    )]
    pub fn cancel(&mut self) -> bool {
        if self.active_runs.len() != 1 {
            return false;
        }
        let activity_id = self.active_runs.keys().next().cloned();
        activity_id.is_some_and(|activity_id| self.cancel_activity(&activity_id))
    }

    #[must_use]
    #[allow(
        dead_code,
        reason = "single-run compatibility tests and external CLI fixtures use this helper"
    )]
    pub fn is_active(&self) -> bool {
        self.control_active || !self.active_runs.is_empty()
    }

    #[must_use]
    pub const fn is_control_active(&self) -> bool {
        self.control_active
    }

    /// Whether this process has an explicit stable approval authority bound to
    /// the same durable interaction service used by F6 and slash commands.
    #[must_use]
    pub fn approval_authority_bound(&self) -> bool {
        self.polkagent_runtime
            .interactions()
            .approval_authority_bound()
    }

    #[must_use]
    #[allow(
        dead_code,
        reason = "headless orchestration tests inspect the bounded active-run count"
    )]
    pub fn active_run_count(&self) -> usize {
        self.active_runs.len()
    }

    fn apply_controller_update(&mut self, update: &ControllerUpdate) {
        match update {
            ControllerUpdate::Control(event) => {
                if event.is_terminal() {
                    self.control_active = false;
                }
            }
            ControllerUpdate::Activity(update) => {
                if let ControllerEvent::Started {
                    conversation_id, ..
                } = &update.event
                {
                    if let Some(run) = self.active_runs.get_mut(&update.activity_id) {
                        run.conversation_id = Some(conversation_id.clone());
                    }
                }
                if update.event.is_terminal() {
                    self.active_runs.remove(&update.activity_id);
                }
            }
        }
    }

    /// Non-blocking compatibility hook for headless cross-surface fixtures.
    /// The production TUI awaits [`Self::recv`] instead of polling this method.
    #[allow(
        dead_code,
        reason = "the binary compiles this shared module without the integration fixtures that exercise the compatibility hook"
    )]
    pub fn try_recv(&mut self) -> Option<ControllerEvent> {
        self.try_recv_update().map(ControllerUpdate::into_event)
    }

    pub fn try_recv_update(&mut self) -> Option<ControllerUpdate> {
        let update = match self.run_event_rx.try_recv() {
            Ok(update) => Some(ControllerUpdate::Activity(update)),
            Err(mpsc::error::TryRecvError::Empty) => match self.event_rx.try_recv() {
                Ok(event) => Some(ControllerUpdate::Control(event)),
                Err(mpsc::error::TryRecvError::Empty | mpsc::error::TryRecvError::Disconnected) => {
                    None
                }
            },
            Err(mpsc::error::TryRecvError::Disconnected) => {
                self.event_rx.try_recv().ok().map(ControllerUpdate::Control)
            }
        }?;
        self.apply_controller_update(&update);
        Some(update)
    }

    /// Wait for one controller completion without polling the runtime bridge.
    #[allow(
        dead_code,
        reason = "single-run compatibility tests and cross-surface fixtures receive unwrapped events"
    )]
    pub async fn recv(&mut self) -> Option<ControllerEvent> {
        self.recv_update().await.map(ControllerUpdate::into_event)
    }

    /// Wait for one correlated control or run update without polling.
    pub async fn recv_update(&mut self) -> Option<ControllerUpdate> {
        let update = tokio::select! {
            event = self.run_event_rx.recv() => {
                event.map(ControllerUpdate::Activity)
            }
            event = self.event_rx.recv() => {
                event.map(ControllerUpdate::Control)
            }
        }?;
        self.apply_controller_update(&update);
        Some(update)
    }

    /// Cancel, drain, and reap every task spawned by this controller.
    ///
    /// Production workers translate this signal into
    /// [`InteractionService::cancel_turn`]. The service owns the durable
    /// human-decision-versus-cancellation CAS; the TUI must not preflight or
    /// reproduce coordinator state locally.
    pub async fn shutdown(&mut self) {
        for run in self.active_runs.values_mut() {
            if let Some(cancel) = run.cancel.take() {
                let _ = cancel.send(());
            }
        }
        let mut tasks = std::mem::take(&mut self.tasks);
        let completed = tokio::time::timeout(self.shutdown_grace, async {
            for task in &mut tasks {
                let _ = task.await;
            }
        })
        .await
        .is_ok();
        if !completed {
            for task in &tasks {
                if !task.is_finished() {
                    task.abort();
                }
            }
            for task in tasks {
                if !task.is_finished() {
                    let _ = task.await;
                }
            }
        }
        self.active_runs.clear();
        self.control_active = false;
    }
}

fn safe_approval_error(error: &InteractionError) -> (InteractionErrorCode, String) {
    let code = error.code;
    let redacted = polkagent_telemetry::redact_string(&error.message);
    (
        code,
        bounded_grapheme_prefix(&redacted, MAX_APPROVAL_ERROR_BYTES).to_owned(),
    )
}

impl Drop for RunController {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
        for run in self.active_runs.values_mut() {
            if let Some(cancel) = run.cancel.take() {
                let _ = cancel.send(());
            }
        }
    }
}

fn tui_interaction_title(agent_name: &str) -> String {
    format!("{TUI_INTERACTION_TITLE_PREFIX} · {agent_name}")
}

fn started_controller_event(
    interaction: &InteractionSummary,
    turn_id: InteractionTurnId,
    run_id: RunId,
    agent_name: String,
    notes: Vec<String>,
) -> ControllerEvent {
    ControllerEvent::Started {
        conversation_id: interaction.conversation_id.to_string(),
        model: interaction.config.model.clone(),
        turn_id: turn_id.to_string(),
        run_id: run_id.to_string(),
        agent_name,
        notes,
    }
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

async fn send_command_failure(
    event_tx: &mpsc::Sender<ControllerEvent>,
    agent_id: String,
    conversation_id: Option<String>,
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
        approval_resolution: None,
    };
    let _ = event_tx
        .send(ControllerEvent::CommandFailed {
            agent_id,
            conversation_id,
            request_id,
            result,
        })
        .await;
}

fn project_session_items(
    interactions: Vec<InteractionSummary>,
    agent_id: AgentId,
) -> Vec<ConsoleSessionItem> {
    interactions
        .into_iter()
        .filter(|interaction| {
            matches!(interaction.config.target, InteractionTarget::Agent(target) if target == agent_id)
        })
        .take(MAX_SESSION_SELECTOR_ITEMS)
        .map(|interaction| {
            let title = interaction
                .title
                .as_deref()
                .map(str::trim)
                .filter(|title| !title.is_empty())
                .map_or("Untitled session", |title| {
                    bounded_grapheme_prefix(title, MAX_SESSION_TITLE_BYTES)
                })
                .to_owned();
            let state = match interaction.state {
                DurableInteractionState::Active => "active",
                DurableInteractionState::Archived => "archived",
            }
            .to_owned();
            ConsoleSessionItem {
                conversation_id: interaction.conversation_id.to_string(),
                title,
                state,
                turn_count: interaction.turn_count,
                updated_at: interaction.updated_at.format("%Y-%m-%d %H:%M UTC").to_string(),
            }
        })
        .collect()
}

struct ProjectedConsoleCommand {
    title: String,
    lines: Vec<String>,
    selection: Option<ConsoleConversationSelection>,
    model_update: ConsoleModelUpdate,
    agent_update: Option<ConsoleAgentSelection>,
}

#[allow(
    clippy::too_many_lines,
    reason = "one exhaustive projection keeps supported and refused command outputs explicit"
)]
async fn project_console_command_output(
    runtime: &PolkagentRuntime,
    agent_id: AgentId,
    output: CommandOutput,
) -> Result<ProjectedConsoleCommand, String> {
    if let Some(rendered) = format_run_command_output(&output) {
        let title = match &output {
            CommandOutput::Runs { .. } => "Durable conversation runs",
            CommandOutput::RunInspected { .. } => "Durable run inspection",
            _ => "Durable run command",
        };
        return Ok(ProjectedConsoleCommand {
            title: title.to_owned(),
            lines: rendered.lines().map(str::to_owned).collect(),
            selection: None,
            model_update: ConsoleModelUpdate::Unchanged,
            agent_update: None,
        });
    }
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
                    if spec.command == CommandName::Cancel {
                        return "/cancel (/stop) — Cancel the selected exact active Console turn; run-ID and all-activity forms are unavailable".to_owned();
                    }
                    let hint = spec
                        .input_hint
                        .as_deref()
                        .map_or(String::new(), |hint| format!(" {hint}"));
                    format!("/{}{hint} — {}", spec.name, spec.description)
                })
                .collect::<Vec<_>>();
            lines.push("x — shortcut for the same selected exact active turn".to_owned());
            lines.push(
                "Approval commands appear only for the authorized selected conversation's exact pending queue; F6 remains the visual path."
                    .to_owned(),
            );
            lines.push(
                "Provider/harness/autonomy and group commands are unavailable in Console."
                    .to_owned(),
            );
            Ok(ProjectedConsoleCommand {
                title: "Supported Console commands".to_owned(),
                lines,
                selection: None,
                model_update: ConsoleModelUpdate::Unchanged,
                agent_update: None,
            })
        }
        CommandOutput::Status {
            interaction,
            active_turns,
            pending_approvals,
        } => {
            let target = match interaction.config.target {
                InteractionTarget::Agent(target_id) => {
                    active_agent_by_id(runtime.pool(), target_id).map_or_else(
                        |_| display_interaction_target(&interaction.config.target),
                        |target| format!("{} ({})", target.name, target.agent_id),
                    )
                }
                _ => display_interaction_target(&interaction.config.target),
            };
            let approval_status = if runtime.interactions().approval_authority_bound() {
                format!("pending approvals: {}", pending_approvals.len())
            } else {
                "pending approvals: unavailable (no explicit TUI authority)".to_owned()
            };
            let mut lines = vec![
                format!("conversation: {}", interaction.conversation_id),
                format!("target: {target}"),
                format!("state: {:?}", interaction.state),
                format!("turns: {}", interaction.turn_count),
                format!("active turns: {}", active_turns.len()),
                approval_status,
                format!(
                    "model: {} (durable conversation selection)",
                    interaction
                        .config
                        .model
                        .as_deref()
                        .unwrap_or("runtime/agent default")
                ),
                format!(
                    "provider: {} (selection unavailable in Console)",
                    interaction
                        .config
                        .provider
                        .as_deref()
                        .unwrap_or("runtime default")
                ),
            ];
            lines.extend(
                pending_approvals
                    .into_iter()
                    .map(|approval_id| format!("pending approval: {approval_id}")),
            );
            Ok(ProjectedConsoleCommand {
                title: "Durable Console status".to_owned(),
                lines,
                selection: None,
                model_update: ConsoleModelUpdate::Selected(interaction.config.model),
                agent_update: None,
            })
        }
        CommandOutput::Agents { agents } => {
            let mut lines = if agents.is_empty() {
                vec!["no active agents".to_owned()]
            } else {
                agents
                    .into_iter()
                    .map(|agent| {
                        let selected = if agent.agent_id == agent_id { "*" } else { " " };
                        format!(
                            "{selected} {} ({}) [{}]",
                            agent.name, agent.agent_id, agent.state
                        )
                    })
                    .collect()
            };
            lines.push(
                "readiness: not projected; lifecycle is from the durable registry".to_owned(),
            );
            Ok(ProjectedConsoleCommand {
                title: "Active durable agents".to_owned(),
                lines,
                selection: None,
                model_update: ConsoleModelUpdate::Unchanged,
                agent_update: None,
            })
        }
        CommandOutput::TargetChanged { target } => {
            let target_id = match target {
                InteractionTarget::Agent(target_id) => target_id,
                InteractionTarget::Group(_) | InteractionTarget::Auto => {
                    return Err("Console received a non-agent target".to_owned());
                }
            };
            let target = registered_agent_by_id(runtime.pool(), target_id)
                .map_err(|error| format!("resolving persisted Console target: {error}"))?;
            Ok(ProjectedConsoleCommand {
                title: "Updated durable Console target".to_owned(),
                lines: vec![
                    format!("agent: {} ({})", target.name, target.agent_id),
                    "scope: selected conversation (persisted)".to_owned(),
                ],
                selection: None,
                model_update: ConsoleModelUpdate::Unchanged,
                agent_update: Some(ConsoleAgentSelection {
                    agent_id: target.agent_id.to_string(),
                    agent_name: target.name,
                }),
            })
        }
        CommandOutput::InteractionCreated { interaction } => {
            let target_id =
                console_target_agent_id(&interaction).map_err(|error| error.to_string())?;
            let target = active_agent_by_id(runtime.pool(), target_id)
                .map_err(|error| format!("resolving created Console target: {error}"))?;
            let (_, turns) = load_interaction_history(runtime, &interaction).await?;
            Ok(ProjectedConsoleCommand {
                title: "Created durable interaction".to_owned(),
                lines: vec![
                    format!("conversation: {}", interaction.conversation_id),
                    format!("agent: {}", target.name),
                    format!(
                        "title: {}",
                        interaction.title.as_deref().unwrap_or("untitled")
                    ),
                ],
                selection: Some(ConsoleConversationSelection {
                    conversation_id: interaction.conversation_id.to_string(),
                    model: interaction.config.model.clone(),
                    turns,
                }),
                model_update: ConsoleModelUpdate::Unchanged,
                agent_update: Some(ConsoleAgentSelection {
                    agent_id: target.agent_id.to_string(),
                    agent_name: target.name,
                }),
            })
        }
        CommandOutput::InteractionResumed { interaction } => {
            let target_id =
                console_target_agent_id(&interaction).map_err(|error| error.to_string())?;
            let target = active_agent_by_id(runtime.pool(), target_id)
                .map_err(|error| format!("resolving resumed Console target: {error}"))?;
            let (_, turns) = load_interaction_history(runtime, &interaction).await?;
            Ok(ProjectedConsoleCommand {
                title: "Resumed durable interaction".to_owned(),
                lines: vec![
                    format!("conversation: {}", interaction.conversation_id),
                    format!("agent: {}", target.name),
                    format!("restored turns: {}", turns.len()),
                ],
                selection: Some(ConsoleConversationSelection {
                    conversation_id: interaction.conversation_id.to_string(),
                    model: interaction.config.model.clone(),
                    turns,
                }),
                model_update: ConsoleModelUpdate::Unchanged,
                agent_update: Some(ConsoleAgentSelection {
                    agent_id: target.agent_id.to_string(),
                    agent_name: target.name,
                }),
            })
        }
        CommandOutput::Model { model, changed } => Ok(ProjectedConsoleCommand {
            title: if changed {
                "Updated durable Console model"
            } else {
                "Durable Console model"
            }
            .to_owned(),
            lines: vec![
                format!(
                    "model: {}",
                    model.as_deref().unwrap_or("runtime/agent default")
                ),
                format!(
                    "scope: selected conversation ({})",
                    if changed { "persisted" } else { "current" }
                ),
            ],
            selection: None,
            model_update: ConsoleModelUpdate::Selected(model),
            agent_update: None,
        }),
        CommandOutput::Runs { .. } | CommandOutput::RunInspected { .. } => {
            Err("shared run command output could not be rendered safely".to_owned())
        }
        CommandOutput::CancellationRequested {
            turn_id: Some(turn_id),
            run_ids,
        } => {
            let mut lines = vec![format!("turn: {turn_id}")];
            lines.extend(run_ids.into_iter().map(|run_id| format!("run: {run_id}")));
            Ok(ProjectedConsoleCommand {
                title: "Cancellation requested for selected Console turn".to_owned(),
                lines,
                selection: None,
                model_update: ConsoleModelUpdate::Unchanged,
                agent_update: None,
            })
        }
        CommandOutput::CancellationRequested { turn_id: None, .. } => {
            Err("shared command returned an output unsupported by Console".to_owned())
        }
        CommandOutput::ApprovalResolved {
            approval_id,
            decision,
        } => Ok(ProjectedConsoleCommand {
            title: "Durable approval resolved".to_owned(),
            lines: vec![
                format!("approval: {approval_id}"),
                match decision {
                    ApprovalDecision::Approve => "decision: approved".to_owned(),
                    ApprovalDecision::Deny { .. } => "decision: denied".to_owned(),
                },
                "scope: selected authorized Console conversation".to_owned(),
            ],
            selection: None,
            model_update: ConsoleModelUpdate::Unchanged,
            agent_update: None,
        }),
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
    pool: SqlitePool,
    run_commands: RunCommandReadModel,
    agent_id: AgentId,
    exact_turn_filter: Option<InteractionTurnId>,
    approval_authority_bound: bool,
}

#[async_trait]
impl InteractionCommandRuntime for TuiCommandRuntime {
    async fn default_interaction_config(&self) -> Result<InteractionConfig, InteractionError> {
        Ok(InteractionConfig::new(InteractionTarget::Agent(
            self.agent_id,
        )))
    }

    async fn list_agents(&self) -> Result<Vec<AgentTargetView>, InteractionError> {
        list_active_agent_targets(&self.pool)
    }

    async fn resolve_agent(&self, selector: &str) -> Result<AgentTargetView, InteractionError> {
        resolve_active_agent_target(&self.pool, selector)
    }

    async fn list_runs(
        &self,
        conversation_id: ConversationId,
    ) -> Result<Vec<RunSummaryView>, InteractionError> {
        self.run_commands.list_runs(conversation_id).await
    }

    async fn inspect_run(&self, _run_id: RunId) -> Result<RunDetailView, InteractionError> {
        Err(tui_command_unsupported("run inspection"))
    }

    async fn inspect_run_for_conversation(
        &self,
        conversation_id: ConversationId,
        run_id: RunId,
    ) -> Result<RunDetailView, InteractionError> {
        self.run_commands.inspect_run(conversation_id, run_id).await
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
            .filter(|turn| {
                self.exact_turn_filter
                    .is_none_or(|turn_id| turn.handle.turn_id == turn_id)
            })
            .map(|turn| turn.handle)
            .collect())
    }

    async fn pending_approvals(
        &self,
        conversation_id: ConversationId,
    ) -> Result<Vec<ApprovalId>, InteractionError> {
        if !self.approval_authority_bound {
            return Ok(Vec::new());
        }
        Ok(self
            .service
            .list_pending_approvals(conversation_id)
            .await?
            .into_iter()
            .filter(|approval| approval.status == polkagent_interaction::ApprovalStatus::Pending)
            .map(|approval| approval.approval_id)
            .collect())
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
    let target = console_target_agent_id(interaction)?;
    if target == agent_id {
        Ok(())
    } else {
        Err(InteractionError::new(
            InteractionErrorCode::Conflict,
            format!(
                "conversation {} targets agent {target}, not selected agent {agent_id}",
                interaction.conversation_id
            ),
        ))
    }
}

fn console_target_agent_id(interaction: &InteractionSummary) -> Result<AgentId, InteractionError> {
    match interaction.config.target {
        InteractionTarget::Agent(target) => Ok(target),
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
) -> Result<(Option<ConversationId>, Option<String>, Vec<ConsoleRun>), String> {
    let service = runtime.interactions();
    let Some(interaction) = find_console_interaction(service.as_ref(), agent_id)
        .await
        .map_err(|error| error.to_string())?
    else {
        return Ok((None, None, Vec::new()));
    };
    let (conversation_id, turns) = load_interaction_history(runtime, &interaction).await?;
    Ok((conversation_id, interaction.config.model, turns))
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
        let (mut output, usage) = if summary.state == TurnState::Completed {
            let output = transcript_turn.assistant_text.ok_or_else(|| {
                "completed durable TUI turn is missing its assistant transcript".to_owned()
            })?;
            let usage = load_completed_turn_usage(
                service.as_ref(),
                interaction.conversation_id,
                summary.handle.turn_id,
            )
            .await?;
            (output, usage)
        } else {
            (String::new(), UsageView::default())
        };
        truncate_output(&mut output);
        let status = console_status(summary.state);
        turns.push(ConsoleRun {
            activity_id: None,
            conversation_id: Some(interaction.conversation_id.to_string()),
            turn_id: Some(summary.handle.turn_id.to_string()),
            run_id: summary.handle.run_ids.first().map(ToString::to_string),
            prompt: transcript_turn.user_text,
            output,
            detail: format!("restored durable {} turn", status.label()),
            status,
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
        });
    }
    Ok((Some(interaction.conversation_id), turns))
}

async fn load_completed_turn_usage(
    service: &dyn InteractionService,
    conversation_id: ConversationId,
    turn_id: InteractionTurnId,
) -> Result<UsageView, String> {
    let mut after_sequence = None;
    loop {
        let mut events = service
            .subscribe(SubscriptionRequest {
                conversation_id,
                turn_id: Some(turn_id),
                after_sequence,
                capacity: INTERACTION_STREAM_CAPACITY,
            })
            .await
            .map_err(|error| format!("load TUI turn usage: {error}"))?;
        loop {
            match events.recv().await {
                Ok(envelope) => {
                    after_sequence = Some(envelope.sequence);
                    match envelope.event {
                        InteractionEvent::TurnCompleted { result } => return Ok(result.usage),
                        InteractionEvent::TurnFailed { .. }
                        | InteractionEvent::TurnCancelled { .. }
                        | InteractionEvent::TurnTimedOut => {
                            return Err(
                                "completed durable TUI turn has a non-completed terminal event"
                                    .to_owned(),
                            );
                        }
                        InteractionEvent::TurnStarted { .. }
                        | InteractionEvent::AgentMessageDelta { .. }
                        | InteractionEvent::ThoughtDelta { .. }
                        | InteractionEvent::ToolCallStarted { .. }
                        | InteractionEvent::ToolCallUpdated { .. }
                        | InteractionEvent::PlanUpdated { .. }
                        | InteractionEvent::ApprovalRequested { .. }
                        | InteractionEvent::ApprovalResolved { .. }
                        | InteractionEvent::UsageUpdated { .. }
                        | InteractionEvent::RunStateChanged { .. } => {}
                    }
                }
                Err(StreamError::Lagged {
                    last_seen_sequence, ..
                }) => {
                    after_sequence = last_seen_sequence.or(after_sequence);
                    break;
                }
                Err(StreamError::Closed) => {
                    return Err(
                        "completed durable TUI turn closed before its terminal usage event"
                            .to_owned(),
                    );
                }
                Err(StreamError::Backend(error)) => {
                    return Err(format!("load TUI turn usage: {error}"));
                }
            }
        }
    }
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
    event_tx: &mpsc::Sender<RunActivityUpdate>,
    context: &RunActivityContext,
) {
    let progress = match service.cancel_turn(turn_id).await {
        Ok(()) => format!("cancellation requested for turn {turn_id}"),
        Err(error) => format!("cancelling interaction turn {turn_id}: {error}"),
    };
    let _ = event_tx
        .send(context.update(ControllerEvent::Progress(progress)))
        .await;
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
            ControllerEvent::Progress(format!(
                "tool {} {}: {:?}",
                call.call_id, call.title, call.status
            ))
        }
        InteractionEvent::PlanUpdated { entries } => {
            ControllerEvent::Progress(format!("plan updated: {} step(s)", entries.len()))
        }
        InteractionEvent::ApprovalRequested { request } => ControllerEvent::TurnApprovalRequested {
            approval_id: request.approval_id,
        },
        InteractionEvent::ApprovalResolved {
            approval_id,
            decision,
        } => ControllerEvent::TurnApprovalResolved {
            approval_id,
            decision,
        },
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
    tui_runtime_options_with_approval(pool, config_path, None)
}

/// Build TUI runtime options with an explicit process-scoped approval
/// authority. Only the explicit `polkagent tui` command calls this variant;
/// the default no-subcommand surface uses [`tui_runtime_options`] and remains
/// authority-unbound.
pub fn tui_runtime_options_with_approval(
    pool: &SqlitePool,
    config_path: Option<&Path>,
    approval_authority: Option<InteractionApprovalAuthority>,
) -> anyhow::Result<RuntimeOptions> {
    let workdir = std::env::current_dir()
        .map_err(|error| anyhow::anyhow!("resolving TUI working directory: {error}"))?;
    let mut options = RuntimeOptions::new(workdir);
    options.config_path = config_path.map(Path::to_path_buf);
    options.database_path = Some(pool.path().to_path_buf());
    options.adapter_policy = AdapterPolicy::AllowSimulated;
    options.approval_authority = approval_authority;
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
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;
    use std::time::Duration;

    use polkagent_core::{AgentSpec, EffectId, PrincipalId, RunId};
    use polkagent_interaction::ToolCallId;
    use polkagent_runtime::{ConfigSource, RuntimeFactory};
    use polkagent_store_sqlite::{migrations, SqliteRunStore};
    use polkagent_store_trait::{RunStatus, RunStore};
    use tokio::sync::Notify;

    #[derive(Default)]
    struct ControlledFakeRun {
        emit_output: Notify,
        finish: Notify,
        cancellations: AtomicUsize,
    }

    async fn controller_test_runtime() -> (tempfile::TempDir, PolkagentRuntime) {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let database_path = temp.path().join("controller-test.db");
        let pool = SqlitePool::open(&database_path).expect("open database");
        migrations::migrate(&pool.writer()).expect("migrate database");
        let mut options = tui_runtime_options(&pool, None).expect("TUI runtime options");
        options.workdir = temp.path().to_path_buf();
        options.disable_harness = true;
        options.discover_environment_providers = false;
        drop(pool);
        let runtime = Box::pin(RuntimeFactory::build(options))
            .await
            .expect("build runtime");
        (temp, runtime)
    }

    #[derive(Clone)]
    struct ApprovalFixtureService {
        inner: Arc<dyn InteractionService>,
        state: Arc<Mutex<ApprovalFixtureState>>,
    }

    struct ApprovalFixtureState {
        conversation_id: ConversationId,
        approval: ApprovalView,
        decision: Option<ApprovalDecision>,
    }

    impl ApprovalFixtureService {
        fn new(inner: Arc<dyn InteractionService>, conversation_id: ConversationId) -> Self {
            Self {
                inner,
                state: Arc::new(Mutex::new(ApprovalFixtureState {
                    conversation_id,
                    approval: ApprovalView {
                        approval_id: ApprovalId::new(),
                        effect_id: EffectId::new(),
                        run_id: RunId::new(),
                        tool_call_id: Some(ToolCallId::new()),
                        title: "Write notes.txt".to_owned(),
                        description: "Write one file in the selected workspace".to_owned(),
                        status: polkagent_interaction::ApprovalStatus::Pending,
                        policy_reason: Some("filesystem write grant requires approval".to_owned()),
                        expires_at: None,
                    },
                    decision: None,
                })),
            }
        }

        fn approval_id(&self) -> ApprovalId {
            self.state
                .lock()
                .expect("approval fixture lock")
                .approval
                .approval_id
        }

        fn decide(
            &self,
            conversation_id: ConversationId,
            approval_id: ApprovalId,
            decision: ApprovalDecision,
        ) -> Result<ApprovalView, InteractionError> {
            let mut state = self.state.lock().expect("approval fixture lock");
            if state.conversation_id != conversation_id || state.approval.approval_id != approval_id
            {
                return Err(InteractionError::new(
                    InteractionErrorCode::PermissionDenied,
                    "approval is outside the authenticated conversation scope",
                ));
            }
            if let Some(existing) = &state.decision {
                if existing != &decision {
                    let unsafe_detail = match &decision {
                        ApprovalDecision::Deny {
                            reason: Some(reason),
                        } => {
                            format!("; untrusted denial detail: {reason}")
                        }
                        ApprovalDecision::Approve | ApprovalDecision::Deny { reason: None } => {
                            String::new()
                        }
                    };
                    return Err(InteractionError::new(
                        InteractionErrorCode::Conflict,
                        format!("approval already has a different durable decision{unsafe_detail}"),
                    ));
                }
                return Ok(state.approval.clone());
            }
            state.approval.status = match decision {
                ApprovalDecision::Approve => polkagent_interaction::ApprovalStatus::Approved,
                ApprovalDecision::Deny { .. } => polkagent_interaction::ApprovalStatus::Denied,
            };
            state.decision = Some(decision);
            Ok(state.approval.clone())
        }
    }

    #[async_trait]
    impl InteractionService for ApprovalFixtureService {
        async fn new_interaction(
            &self,
            request: CreateInteractionRequest,
        ) -> Result<InteractionSummary, InteractionError> {
            self.inner.new_interaction(request).await
        }

        async fn list_interactions(
            &self,
            request: ListInteractionsRequest,
        ) -> Result<Vec<InteractionSummary>, InteractionError> {
            self.inner.list_interactions(request).await
        }

        async fn load_interaction(
            &self,
            conversation_id: ConversationId,
        ) -> Result<InteractionSummary, InteractionError> {
            self.inner.load_interaction(conversation_id).await
        }

        async fn verify_interaction_origin(
            &self,
            conversation_id: ConversationId,
            working_directory: &std::path::Path,
        ) -> Result<(), InteractionError> {
            self.inner
                .verify_interaction_origin(conversation_id, working_directory)
                .await
        }

        async fn list_turns(
            &self,
            conversation_id: ConversationId,
        ) -> Result<Vec<polkagent_interaction::TurnSummary>, InteractionError> {
            self.inner.list_turns(conversation_id).await
        }

        async fn load_transcript(
            &self,
            request: TranscriptRequest,
        ) -> Result<Vec<polkagent_interaction::InteractionTranscriptTurn>, InteractionError>
        {
            self.inner.load_transcript(request).await
        }

        async fn delete_interaction(
            &self,
            conversation_id: ConversationId,
        ) -> Result<(), InteractionError> {
            self.inner.delete_interaction(conversation_id).await
        }

        async fn prompt(
            &self,
            request: InteractionPromptRequest,
        ) -> Result<polkagent_interaction::StartedTurn, InteractionError> {
            self.inner.prompt(request).await
        }

        async fn cancel_turn(&self, turn_id: InteractionTurnId) -> Result<(), InteractionError> {
            self.inner.cancel_turn(turn_id).await
        }

        async fn set_config_option(
            &self,
            conversation_id: ConversationId,
            update: polkagent_interaction::ConfigUpdate,
        ) -> Result<InteractionConfig, InteractionError> {
            self.inner.set_config_option(conversation_id, update).await
        }

        async fn list_pending_approvals(
            &self,
            conversation_id: ConversationId,
        ) -> Result<Vec<ApprovalView>, InteractionError> {
            let state = self.state.lock().expect("approval fixture lock");
            if state.conversation_id != conversation_id {
                return Err(InteractionError::new(
                    InteractionErrorCode::PermissionDenied,
                    "conversation is outside the authenticated approval scope",
                ));
            }
            Ok((state.decision.is_none())
                .then(|| state.approval.clone())
                .into_iter()
                .collect())
        }

        async fn approve(
            &self,
            conversation_id: ConversationId,
            approval_id: ApprovalId,
        ) -> Result<ApprovalView, InteractionError> {
            self.decide(conversation_id, approval_id, ApprovalDecision::Approve)
        }

        async fn deny(
            &self,
            conversation_id: ConversationId,
            approval_id: ApprovalId,
            reason: Option<String>,
        ) -> Result<ApprovalView, InteractionError> {
            self.decide(
                conversation_id,
                approval_id,
                ApprovalDecision::Deny { reason },
            )
        }

        async fn subscribe(
            &self,
            request: SubscriptionRequest,
        ) -> Result<polkagent_interaction::BoxInteractionEventStream, InteractionError> {
            self.inner.subscribe(request).await
        }
    }

    fn controlled_worker(gates: Arc<BTreeMap<String, Arc<ControlledFakeRun>>>) -> TestRunWorker {
        Arc::new(move |request, context, event_tx, mut cancel_rx| {
            let gates = Arc::clone(&gates);
            Box::pin(async move {
                let gate = Arc::clone(
                    gates
                        .get(&request.agent_id)
                        .expect("controlled gate for agent"),
                );
                let _ = event_tx
                    .send(
                        context.update(ControllerEvent::Started {
                            conversation_id: request
                                .conversation_id
                                .clone()
                                .unwrap_or_else(|| format!("conversation-{}", request.agent_id)),
                            model: Some("fake/controlled".to_owned()),
                            turn_id: format!("turn-{}", request.agent_id),
                            run_id: format!("run-{}", request.agent_id),
                            agent_name: request.agent_name,
                            notes: vec!["controlled fake execution".to_owned()],
                        }),
                    )
                    .await;
                tokio::select! {
                    _ = &mut cancel_rx => {
                        gate.cancellations.fetch_add(1, Ordering::SeqCst);
                        let _ = event_tx.send(context.update(ControllerEvent::Cancelled(
                            "controlled cancellation".to_owned(),
                        ))).await;
                        return;
                    }
                    () = gate.emit_output.notified() => {}
                }
                let _ = event_tx
                    .send(context.update(ControllerEvent::Output(format!(
                        "output-{}",
                        request.agent_id
                    ))))
                    .await;
                tokio::select! {
                    _ = &mut cancel_rx => {
                        gate.cancellations.fetch_add(1, Ordering::SeqCst);
                        let _ = event_tx.send(context.update(ControllerEvent::Cancelled(
                            "controlled cancellation".to_owned(),
                        ))).await;
                    }
                    () = gate.finish.notified() => {
                        let _ = event_tx.send(context.update(ControllerEvent::Completed {
                            text: format!("output-{}", request.agent_id),
                            input_tokens: 1,
                            output_tokens: 1,
                        })).await;
                    }
                }
            })
        })
    }

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
            model: Some("fake/model-a".to_owned()),
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
        assert_eq!(state.selected_model.as_deref(), Some("fake/model-a"));
        assert_eq!(run.run_id.as_deref(), Some("run-id"));
        assert_eq!(run.output, "hello world");
        assert_eq!(run.status, ConsoleRunStatus::Completed);
        assert_eq!((run.input_tokens, run.output_tokens), (9, 2));
    }

    #[test]
    fn tui_tool_progress_preserves_durable_identity_and_status() {
        let call_id = polkagent_interaction::ToolCallId::new();
        let event = InteractionEvent::ToolCallUpdated {
            call: polkagent_interaction::ToolCallView {
                call_id,
                run_id: RunId::new(),
                name: "test.observe".to_owned(),
                title: "test.observe".to_owned(),
                kind: polkagent_interaction::ToolCallKind::Other,
                status: polkagent_interaction::ToolCallStatus::Succeeded,
                arguments: None,
                summary: Some("completed safely".to_owned()),
                output: None,
                locations: Vec::new(),
                diff: None,
                error: None,
            },
        };
        assert_eq!(
            project_interaction_event(event),
            ControllerEvent::Progress(format!("tool {call_id} test.observe: Succeeded"))
        );
    }

    #[test]
    fn reducer_bounds_terminal_output_on_grapheme_boundaries() {
        let mut state = InteractionState::default();
        state.select_agent("agent-id", "Alice");
        type_prompt(&mut state, "produce a large answer");
        state.submit().expect("valid prompt");
        let final_text = format!("{}tail", "e\u{301}".repeat(MAX_OUTPUT_BYTES));

        state.apply(ControllerEvent::Completed {
            text: final_text,
            input_tokens: 1,
            output_tokens: 2,
        });

        let output = &state.run.as_ref().expect("completed run").output;
        assert!(output.len() <= MAX_OUTPUT_BYTES);
        let body = output.strip_suffix("tail").expect("preserve newest output");
        assert!(!body.is_empty());
        assert!(body.graphemes(true).all(|grapheme| grapheme == "e\u{301}"));
    }

    #[test]
    fn reducer_restores_durable_transcript_and_composer_history() {
        let mut state = InteractionState::default();
        state.select_agent("agent-id", "Alice");
        let restored = |ordinal: u8| ConsoleRun {
            activity_id: None,
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
            model: Some("fake/model-a".to_owned()),
            turns: vec![restored(1), restored(2)],
        });

        assert_eq!(state.conversation_id.as_deref(), Some("conversation-id"));
        assert_eq!(state.selected_model.as_deref(), Some("fake/model-a"));
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
            model: Some("fake/stale".to_owned()),
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
    fn prompt_editor_moves_and_deletes_extended_grapheme_clusters() {
        let mut state = InteractionState::default();
        type_prompt(&mut state, "a👩\u{200d}💻e\u{301}界");
        assert_eq!(state.cursor(), state.prompt_buffer.len());

        state.move_left();
        assert_eq!(
            &state.prompt_buffer[..state.cursor()],
            "a👩\u{200d}💻e\u{301}"
        );

        state.backspace();
        assert_eq!(state.prompt_buffer, "a👩\u{200d}💻界");
        assert_eq!(&state.prompt_buffer[..state.cursor()], "a👩\u{200d}💻");

        state.delete();
        assert_eq!(state.prompt_buffer, "a👩\u{200d}💻");
        state.move_left();
        state.delete();
        assert_eq!(state.prompt_buffer, "a");
        assert_eq!(state.cursor(), 1);
    }

    #[test]
    fn prompt_editor_keeps_cursor_out_of_a_cluster_joined_by_insertion() {
        let mut state = InteractionState::default();
        type_prompt(&mut state, "👩💻");
        state.move_left();

        assert!(state.push_char('\u{200d}'));
        assert_eq!(state.prompt_buffer, "👩\u{200d}💻");
        assert_eq!(state.cursor(), state.prompt_buffer.len());

        state.backspace();
        assert!(state.prompt_buffer.is_empty());
        assert_eq!(state.cursor(), 0);
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
    fn vertical_movement_uses_grapheme_columns_for_zwj_combining_and_cjk() {
        let mut state = InteractionState::default();
        type_prompt(&mut state, "a👩\u{200d}💻b\ne\u{301}界");

        state.move_up();
        assert_eq!(&state.prompt_buffer[..state.cursor()], "a👩\u{200d}💻");
        state.move_down();
        assert_eq!(state.cursor(), state.prompt_buffer.len());
    }

    #[test]
    fn multiline_paste_is_normalized_as_content_without_command_execution() {
        let mut state = InteractionState::default();
        state.select_agent("agent-id", "Alice");

        let outcome = state.insert_paste("/status\r\nexplain\t界\rnext\u{1b}[31m");

        assert_eq!(state.prompt_buffer, "/status\nexplain    界\nnext[31m");
        assert_eq!(outcome.inserted_bytes, state.prompt_buffer.len());
        assert!(!outcome.truncated);
        assert!(!state.is_command_input());
        assert!(state.slash_command_menu().is_none());
        assert!(state.run.is_none());
        assert!(state.command_result.is_none());

        let request = state.submit().expect("explicitly submit pasted content");
        assert_eq!(request.prompt, "/status\nexplain    界\nnext[31m");
        assert!(state.command_result.is_none());
    }

    #[test]
    fn composer_size_limit_never_splits_a_pasted_grapheme() {
        let mut state = InteractionState::default();
        let pasted = "界".repeat(MAX_COMPOSER_BYTES);

        let outcome = state.insert_paste(&pasted);

        assert!(outcome.truncated);
        assert!(state.prompt_buffer.len() <= MAX_COMPOSER_BYTES);
        assert_eq!(state.prompt_buffer.len() % "界".len(), 0);
        assert_eq!(state.cursor(), state.prompt_buffer.len());
        assert!(state
            .prompt_buffer
            .graphemes(true)
            .all(|cluster| cluster == "界"));
        assert!(!state.push_char('界'));
        assert!(state.prompt_buffer.len() <= MAX_COMPOSER_BYTES);
    }

    #[test]
    fn composer_limit_rejects_one_oversized_grapheme_instead_of_splitting_it() {
        let mut oversized = String::from("e");
        oversized.extend(std::iter::repeat_n('\u{301}', MAX_COMPOSER_BYTES));
        assert_eq!(oversized.graphemes(true).count(), 1);
        let mut state = InteractionState::default();

        let outcome = state.insert_paste(&oversized);

        assert!(outcome.truncated);
        assert_eq!(outcome.inserted_bytes, 0);
        assert!(state.prompt_buffer.is_empty());
        assert_eq!(state.cursor(), 0);
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
        state.conversation_id = Some(ConversationId::new().to_string());
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

        state.clear_prompt();
        type_prompt(&mut state, "/ru");
        let run = state
            .slash_command_menu()
            .and_then(|menu| menu.selected_candidate().cloned())
            .expect("run completion");
        let registry_run = command_registry().resolve("runs").expect("registry run");
        assert_eq!(run.name, registry_run.name);
        assert_eq!(run.description, registry_run.description);
        assert_eq!(run.input_hint, registry_run.input_hint);
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
            vec!["help", "agents", "new", "resume"]
        );
        assert_eq!(menu.selected, 0);

        state.move_down();
        assert_eq!(state.slash_command_menu().unwrap().selected, 1);
        assert!(state.accept_slash_completion());
        assert_eq!(state.prompt_buffer, "/agents");
        assert_eq!(state.cursor(), state.prompt_buffer.len());

        assert!(state.dismiss_slash_completion());
        assert!(state.slash_command_menu().is_none());
        state.conversation_id = Some(ConversationId::new().to_string());
        state.backspace();
        assert!(state.slash_command_menu().is_some());

        state.clear_prompt();
        state.conversation_id = Some(ConversationId::new().to_string());
        type_prompt(&mut state, "/");
        let available = state
            .slash_command_menu()
            .expect("conversation-scoped completions")
            .candidates
            .into_iter()
            .map(|candidate| candidate.name)
            .collect::<Vec<_>>();
        assert_eq!(
            available,
            vec!["help", "status", "agents", "agent", "new", "resume", "runs", "inspect", "model"]
        );
    }

    #[test]
    fn cancel_completion_and_submission_require_the_selected_exact_durable_turn() {
        let mut state = InteractionState::default();
        let agent_id = AgentId::new().to_string();
        let conversation_id = ConversationId::new().to_string();
        let turn_id = InteractionTurnId::new().to_string();
        state.select_agent(agent_id.clone(), "Alice");
        state.conversation_id = Some(conversation_id.clone());
        type_prompt(&mut state, "active prompt");
        state.submit().expect("submit active prompt");
        state
            .bind_activity("activity-a".to_owned())
            .expect("bind active prompt");

        type_prompt(&mut state, "/");
        assert!(!state
            .slash_command_menu()
            .expect("starting command menu")
            .candidates
            .iter()
            .any(|candidate| candidate.name == "cancel"));
        state.clear_prompt();
        state.apply_update(ControllerUpdate::Activity(RunActivityUpdate {
            activity_id: "activity-a".to_owned(),
            agent_id,
            requested_conversation_id: Some(conversation_id.clone()),
            event: ControllerEvent::Started {
                conversation_id,
                model: None,
                turn_id: turn_id.clone(),
                run_id: RunId::new().to_string(),
                agent_name: "Alice".to_owned(),
                notes: Vec::new(),
            },
        }));

        type_prompt(&mut state, "/");
        let cancel = state
            .slash_command_menu()
            .expect("active command menu")
            .candidates
            .into_iter()
            .find(|candidate| candidate.name == "cancel")
            .expect("selected-turn cancel command");
        assert_eq!(cancel.aliases, vec!["stop"]);
        assert_eq!(cancel.usage(), "/cancel");
        assert!(cancel.description.contains("selected exact active"));

        state.clear_prompt();
        type_prompt(&mut state, "/stop");
        let ConsoleCommandSubmission::Execute(request) =
            state.submit_command().expect("submit exact cancellation")
        else {
            panic!("exact cancellation was rejected");
        };
        assert!(matches!(
            request.invocation.command,
            InteractionCommand::Cancel {
                target: polkagent_interaction::CancelTarget::CurrentTurn
            }
        ));
        assert_eq!(
            request.selected_turn_id.map(|id| id.to_string()).as_deref(),
            Some(turn_id.as_str())
        );
        assert_eq!(
            state.run.as_ref().map(|run| &run.status),
            Some(&ConsoleRunStatus::Running),
            "command submission must not manufacture a model turn or terminal state"
        );
    }

    #[test]
    fn cancel_run_and_all_forms_are_structured_refusals() {
        let mut state = InteractionState::default();
        state.select_agent(AgentId::new().to_string(), "Alice");
        state.conversation_id = Some(ConversationId::new().to_string());
        for line in [
            format!("/cancel {}", RunId::new()),
            "/cancel all".to_owned(),
        ] {
            type_prompt(&mut state, &line);
            assert_eq!(
                state.submit_command(),
                Ok(ConsoleCommandSubmission::Rejected)
            );
            let refusal = state.command_result.as_ref().expect("structured refusal");
            assert_eq!(refusal.status, ConsoleCommandStatus::Failed);
            assert!(refusal.lines[0].contains("selected exact active turn"));
        }
    }

    #[test]
    fn approval_commands_require_the_exact_scoped_projection_and_retain_only_exact_retry() {
        let mut state = InteractionState::default();
        let agent_id = AgentId::new().to_string();
        let conversation_id = ConversationId::new();
        let approval_id = ApprovalId::new();
        state.select_agent(agent_id.clone(), "Alice");
        state.conversation_id = Some(conversation_id.to_string());

        type_prompt(&mut state, "/");
        let unavailable = state
            .slash_command_menu()
            .expect("default command menu")
            .candidates;
        assert!(!unavailable
            .iter()
            .any(|item| { matches!(item.name.as_str(), "approve" | "deny") }));
        state.clear_prompt();
        type_prompt(&mut state, "/help approve");
        assert_eq!(
            state.submit_command(),
            Ok(ConsoleCommandSubmission::Rejected)
        );

        let approvals = ConsoleApprovalContext::new(conversation_id, vec![approval_id]);
        state.clear_prompt();
        type_prompt(&mut state, "/");
        let available = state
            .slash_command_menu_with_approval_context(Some(&approvals))
            .expect("authorized command menu")
            .candidates;
        assert!(available.iter().any(|item| item.name == "approve"));
        assert!(available.iter().any(|item| item.name == "deny"));

        state.clear_prompt();
        type_prompt(&mut state, &format!("/approve {approval_id}"));
        let ConsoleCommandSubmission::Execute(request) = state
            .submit_command_with_approval_context(Some(&approvals))
            .expect("submit exact approval")
        else {
            panic!("exact projected approval was rejected")
        };
        assert_eq!(request.pending_approval_count, 1);
        assert!(state.run.is_none(), "approval command created model work");

        state.apply(ControllerEvent::CommandCompleted {
            agent_id,
            conversation_id: Some(conversation_id.to_string()),
            request_id: request.request_id.clone(),
            result: ConsoleCommandResult {
                request_id: request.request_id,
                line: request.line,
                status: ConsoleCommandStatus::Completed,
                title: "Durable approval resolved".to_owned(),
                lines: vec![format!("approval: {approval_id}")],
                approval_resolution: Some(ConsoleApprovalResolution {
                    conversation_id,
                    approval_id,
                    decision: ConsoleApprovalDecision::Approve,
                }),
            },
            selection: None,
            model_update: ConsoleModelUpdate::Unchanged,
            agent_update: None,
        });

        let refreshed = ConsoleApprovalContext::new(conversation_id, Vec::new());
        type_prompt(&mut state, &format!("/deny {approval_id} changed decision"));
        assert!(matches!(
            state.submit_command_with_approval_context(Some(&refreshed)),
            Ok(ConsoleCommandSubmission::Execute(_))
        ));

        let unrelated = ApprovalId::new();
        type_prompt(&mut state, &format!("/approve {unrelated}"));
        assert_eq!(
            state.submit_command_with_approval_context(Some(&refreshed)),
            Ok(ConsoleCommandSubmission::Rejected)
        );

        type_prompt(
            &mut state,
            &format!("/deny {approval_id} {}", "é".repeat(4_097)),
        );
        assert_eq!(
            state.submit_command_with_approval_context(Some(&approvals)),
            Ok(ConsoleCommandSubmission::Rejected)
        );
        let reason = state.command_result.as_ref().expect("bounded refusal");
        assert!(reason.lines.join("\n").contains("4096"));

        let approval_prefix = &approval_id.to_string()[..8];
        type_prompt(&mut state, &format!("/approve {approval_prefix}"));
        assert_eq!(
            state.submit_command_with_approval_context(Some(&approvals)),
            Ok(ConsoleCommandSubmission::Rejected)
        );
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
        assert!(result.lines[0].contains("requires a selected durable"));
        assert!(state.run.is_none());
        assert!(state.prompt_buffer.is_empty());

        state.conversation_id = Some(ConversationId::new().to_string());
        type_prompt(&mut state, "/runs");
        let ConsoleCommandSubmission::Execute(runs) = state
            .submit_command()
            .expect("conversation-scoped runs command")
        else {
            panic!("runs command was rejected")
        };
        assert!(matches!(runs.invocation.command, InteractionCommand::Runs));
        assert!(state.run.is_none());

        type_prompt(&mut state, "/harness codex");
        assert_eq!(
            state.submit_command(),
            Ok(ConsoleCommandSubmission::Rejected)
        );
        let result = state.command_result.as_ref().expect("harness refusal");
        assert_eq!(result.status, ConsoleCommandStatus::Failed);
        assert!(result.lines[0].contains("harness selection is unavailable"));
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

    #[tokio::test(flavor = "current_thread")]
    async fn run_command_projection_uses_shared_bounds_and_redaction() {
        let (_temp, runtime) = Box::pin(controller_test_runtime()).await;
        let runs = (0..25)
            .map(|_| RunSummaryView {
                run_id: RunId::new(),
                agent_id: AgentId::new(),
                state: "failed:provider-token".to_owned(),
                summary: Some("private prompt summary".to_owned()),
            })
            .collect();

        let list =
            project_console_command_output(&runtime, AgentId::new(), CommandOutput::Runs { runs })
                .await
                .expect("project run list");
        assert_eq!(list.title, "Durable conversation runs");
        assert_eq!(list.lines.len(), 21);
        assert!(!list.lines.join("\n").contains("provider-token"));
        assert!(!list.lines.join("\n").contains("private prompt"));

        let artifacts = (0..25)
            .map(|_| polkagent_core::ArtifactId::new().to_string())
            .collect();
        let inspection = project_console_command_output(
            &runtime,
            AgentId::new(),
            CommandOutput::RunInspected {
                run: RunDetailView {
                    run: RunSummaryView {
                        run_id: RunId::new(),
                        agent_id: AgentId::new(),
                        state: "cancelled:secret reason".to_owned(),
                        summary: Some("private summary".to_owned()),
                    },
                    artifacts,
                    error: Some("provider credential leaked".to_owned()),
                },
            },
        )
        .await
        .expect("project run inspection");
        let rendered = inspection.lines.join("\n");
        assert_eq!(inspection.title, "Durable run inspection");
        assert_eq!(inspection.lines.len(), 24);
        assert!(!rendered.contains("secret reason"));
        assert!(!rendered.contains("private summary"));
        assert!(!rendered.contains("provider credential"));
    }

    #[test]
    fn model_command_updates_only_its_request_agent_and_conversation() {
        let first_id = ConversationId::new().to_string();
        let second_id = ConversationId::new().to_string();
        let mut state = InteractionState::default();
        state.select_agent("agent-id", "Alice");
        state.conversation_id = Some(first_id.clone());
        state.selected_model = Some("fake/default-model".to_owned());
        type_prompt(&mut state, "/model model-a");
        let ConsoleCommandSubmission::Execute(request) =
            state.submit_command().expect("typed model command")
        else {
            panic!("model command was rejected")
        };
        assert!(matches!(
            request.invocation.command,
            InteractionCommand::Model {
                model: Some(ref model)
            } if model == "model-a"
        ));
        assert!(state.run.is_none());

        let completed = ControllerEvent::CommandCompleted {
            agent_id: request.agent_id.clone(),
            conversation_id: request.conversation_id.clone(),
            request_id: request.request_id.clone(),
            result: ConsoleCommandResult {
                request_id: request.request_id,
                line: request.line,
                status: ConsoleCommandStatus::Completed,
                title: "Updated durable Console model".to_owned(),
                lines: vec!["model: fake/model-a".to_owned()],
                approval_resolution: None,
            },
            selection: None,
            model_update: ConsoleModelUpdate::Selected(Some("fake/model-a".to_owned())),
            agent_update: None,
        };

        state.conversation_id = Some(second_id);
        state.apply(completed.clone());
        assert_eq!(
            state.selected_model.as_deref(),
            Some("fake/default-model"),
            "stale conversation result must be ignored"
        );

        state.conversation_id = Some(first_id);
        state.apply(completed);
        assert_eq!(state.selected_model.as_deref(), Some("fake/model-a"));
        assert_eq!(
            state.command_result.as_ref().map(|result| result.status),
            Some(ConsoleCommandStatus::Completed)
        );
        assert!(state.run.is_none(), "model commands never become turns");
    }

    #[test]
    fn stale_run_command_result_cannot_replace_the_selected_conversation_viewport() {
        let first_id = ConversationId::new().to_string();
        let second_id = ConversationId::new().to_string();
        let mut state = InteractionState::default();
        state.select_agent("agent-id", "Alice");
        state.conversation_id = Some(first_id.clone());
        type_prompt(&mut state, "/runs");
        let ConsoleCommandSubmission::Execute(request) =
            state.submit_command().expect("typed runs command")
        else {
            panic!("runs command was rejected")
        };
        let mismatched_completion = ControllerEvent::CommandCompleted {
            agent_id: request.agent_id,
            conversation_id: request.conversation_id,
            request_id: request.request_id.clone(),
            result: ConsoleCommandResult {
                request_id: request.request_id,
                line: request.line,
                status: ConsoleCommandStatus::Completed,
                title: "Durable conversation runs".to_owned(),
                lines: vec!["stale run projection".to_owned()],
                approval_resolution: None,
            },
            selection: None,
            model_update: ConsoleModelUpdate::Unchanged,
            agent_update: None,
        };

        state.replace_conversation(Some(second_id.clone()), None, Vec::new());
        type_prompt(&mut state, "draft for selected conversation");
        state.apply(mismatched_completion);

        assert_eq!(state.conversation_id.as_deref(), Some(second_id.as_str()));
        assert_eq!(state.prompt_buffer, "draft for selected conversation");
        assert_ne!(
            state
                .command_result
                .as_ref()
                .map(|result| result.title.as_str()),
            Some("Durable conversation runs")
        );
        assert!(state.run.is_none());
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
            conversation_id: None,
            request_id: "stale-request".to_owned(),
            result: ConsoleCommandResult {
                request_id: "stale-request".to_owned(),
                line: "/new Stale".to_owned(),
                status: ConsoleCommandStatus::Completed,
                title: "stale".to_owned(),
                lines: Vec::new(),
                approval_resolution: None,
            },
            selection: Some(ConsoleConversationSelection {
                conversation_id: "stale-conversation".to_owned(),
                model: Some("fake/stale".to_owned()),
                turns: Vec::new(),
            }),
            model_update: ConsoleModelUpdate::Unchanged,
            agent_update: None,
        });
        assert_ne!(state.conversation_id.as_deref(), Some("stale-conversation"));

        state.apply(ControllerEvent::CommandCompleted {
            agent_id: "agent-id".to_owned(),
            conversation_id: None,
            request_id: request.request_id.clone(),
            result: ConsoleCommandResult {
                request_id: request.request_id,
                line: request.line,
                status: ConsoleCommandStatus::Completed,
                title: "Created durable interaction".to_owned(),
                lines: vec![format!("conversation: {selected_id}")],
                approval_resolution: None,
            },
            selection: Some(ConsoleConversationSelection {
                conversation_id: selected_id.clone(),
                model: Some("fake/model-a".to_owned()),
                turns: Vec::new(),
            }),
            model_update: ConsoleModelUpdate::Unchanged,
            agent_update: None,
        });
        assert_eq!(state.conversation_id.as_deref(), Some(selected_id.as_str()));
        assert_eq!(state.selected_model.as_deref(), Some("fake/model-a"));
        assert_eq!(
            state.command_result.as_ref().map(|result| result.status),
            Some(ConsoleCommandStatus::Completed)
        );
        assert!(state.run.is_none());
    }

    #[test]
    fn agent_command_reducer_preserves_session_state_and_rejects_stale_completion() {
        let selected_conversation = ConversationId::new().to_string();
        let mut state = InteractionState::default();
        state.select_agent("old-agent", "Old Agent");
        state.conversation_id = Some(selected_conversation.clone());
        state.selected_model = Some("fake/selected".to_owned());
        state.transcript.push(ConsoleRun {
            activity_id: None,
            conversation_id: Some(selected_conversation.clone()),
            turn_id: Some("old-turn".to_owned()),
            run_id: Some("old-run".to_owned()),
            prompt: "retained prompt".to_owned(),
            output: "retained output".to_owned(),
            status: ConsoleRunStatus::Completed,
            detail: "completed".to_owned(),
            input_tokens: 1,
            output_tokens: 2,
        });
        type_prompt(&mut state, "/agent new-agent");
        let ConsoleCommandSubmission::Execute(request) =
            state.submit_command().expect("agent command")
        else {
            panic!("agent command rejected")
        };
        let event = ControllerEvent::CommandCompleted {
            agent_id: request.agent_id.clone(),
            conversation_id: request.conversation_id.clone(),
            request_id: request.request_id.clone(),
            result: ConsoleCommandResult {
                request_id: request.request_id.clone(),
                line: request.line.clone(),
                status: ConsoleCommandStatus::Completed,
                title: "Updated durable Console target".to_owned(),
                lines: vec!["agent: New Agent (new-agent)".to_owned()],
                approval_resolution: None,
            },
            selection: None,
            model_update: ConsoleModelUpdate::Unchanged,
            agent_update: Some(ConsoleAgentSelection {
                agent_id: "new-agent".to_owned(),
                agent_name: "New Agent".to_owned(),
            }),
        };

        let mut stale_event = event.clone();
        if let ControllerEvent::CommandCompleted {
            conversation_id, ..
        } = &mut stale_event
        {
            *conversation_id = Some(ConversationId::new().to_string());
        }
        state.apply(stale_event);
        assert_eq!(state.agent_id.as_deref(), Some("old-agent"));

        state.apply(event);
        assert_eq!(state.agent_id.as_deref(), Some("new-agent"));
        assert_eq!(state.agent_name.as_deref(), Some("New Agent"));
        assert_eq!(
            state.conversation_id.as_deref(),
            Some(selected_conversation.as_str())
        );
        assert_eq!(state.selected_model.as_deref(), Some("fake/selected"));
        assert_eq!(state.transcript.len(), 1);
        assert_eq!(state.transcript[0].prompt, "retained prompt");
    }

    #[test]
    fn session_picker_guards_requests_and_agents_then_switches_exact_conversation() {
        let mut state = InteractionState::default();
        state.select_agent("agent-id", "Alice");
        let first_id = ConversationId::new().to_string();
        let second_id = ConversationId::new().to_string();
        state.conversation_id = Some(first_id.clone());
        let request = state.begin_session_picker().expect("open selector");
        let sessions = vec![
            ConsoleSessionItem {
                conversation_id: second_id.clone(),
                title: "Second".to_owned(),
                state: "active".to_owned(),
                turn_count: 2,
                updated_at: "2026-01-02 00:00 UTC".to_owned(),
            },
            ConsoleSessionItem {
                conversation_id: first_id.clone(),
                title: "First".to_owned(),
                state: "active".to_owned(),
                turn_count: 1,
                updated_at: "2026-01-01 00:00 UTC".to_owned(),
            },
        ];

        state.apply(ControllerEvent::SessionListLoaded {
            agent_id: "agent-id".to_owned(),
            request_id: "stale".to_owned(),
            sessions: sessions.clone(),
        });
        assert_eq!(
            state.session_picker.as_ref().map(|picker| picker.status),
            Some(SessionPickerStatus::LoadingList)
        );
        state.apply(ControllerEvent::SessionListLoaded {
            agent_id: "other-agent".to_owned(),
            request_id: request.request_id.clone(),
            sessions: sessions.clone(),
        });
        assert_eq!(
            state.session_picker.as_ref().map(|picker| picker.status),
            Some(SessionPickerStatus::LoadingList)
        );
        state.apply(ControllerEvent::SessionListLoaded {
            agent_id: request.agent_id,
            request_id: request.request_id,
            sessions,
        });
        let picker = state.session_picker.as_ref().expect("ready selector");
        assert_eq!(picker.status, SessionPickerStatus::Ready);
        assert_eq!(picker.selected, 1, "current conversation is highlighted");

        state.session_picker_down();
        let load = state
            .begin_session_selection()
            .expect("select second session");
        assert_eq!(load.conversation, second_id);
        state.apply(ControllerEvent::SessionSelected {
            agent_id: "agent-id".to_owned(),
            request_id: "stale-load".to_owned(),
            selection: ConsoleConversationSelection {
                conversation_id: "stale-conversation".to_owned(),
                model: Some("fake/stale".to_owned()),
                turns: Vec::new(),
            },
        });
        assert_eq!(state.conversation_id.as_deref(), Some(first_id.as_str()));
        assert!(state.session_picker.is_some());

        state.apply(ControllerEvent::SessionSelectionFailed {
            agent_id: "agent-id".to_owned(),
            request_id: load.request_id,
            reason: "service unavailable".to_owned(),
        });
        let picker = state.session_picker.as_ref().expect("selector stays open");
        assert_eq!(picker.status, SessionPickerStatus::Ready);
        assert_eq!(picker.error.as_deref(), Some("service unavailable"));

        let retry = state
            .begin_session_selection()
            .expect("retry selected session");
        state.apply(ControllerEvent::SessionSelected {
            agent_id: retry.agent_id,
            request_id: retry.request_id,
            selection: ConsoleConversationSelection {
                conversation_id: second_id.clone(),
                model: Some("fake/model-b".to_owned()),
                turns: Vec::new(),
            },
        });
        assert_eq!(state.conversation_id.as_deref(), Some(second_id.as_str()));
        assert_eq!(state.selected_model.as_deref(), Some("fake/model-b"));
        assert!(state.session_picker.is_none());
    }

    #[test]
    fn session_picker_remains_available_during_an_active_turn() {
        let mut state = InteractionState::default();
        state.select_agent("agent-id", "Alice");
        type_prompt(&mut state, "active prompt");
        state.submit().expect("start reducer turn");

        let request = state
            .begin_session_picker()
            .expect("active work does not block durable session navigation");
        assert_eq!(request.agent_id, "agent-id");
        assert_eq!(
            state.run.as_ref().map(|run| &run.status),
            Some(&ConsoleRunStatus::Starting)
        );
        assert!(state.session_picker.is_some());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn controller_runs_two_sessions_concurrently_and_cancels_exactly_one() {
        let (_temp, runtime) = Box::pin(controller_test_runtime()).await;
        let gate_a = Arc::new(ControlledFakeRun::default());
        let gate_b = Arc::new(ControlledFakeRun::default());
        let gates = Arc::new(BTreeMap::from([
            ("agent-a".to_owned(), Arc::clone(&gate_a)),
            ("agent-b".to_owned(), Arc::clone(&gate_b)),
        ]));
        let mut controller = RunController::with_test_run_worker(
            runtime,
            controlled_worker(gates),
            Duration::from_millis(100),
        );
        let mut state = InteractionState::default();

        state.select_agent("agent-a", "Alice");
        state.conversation_id = Some("conversation-a".to_owned());
        state.transcript.push(ConsoleRun {
            activity_id: None,
            conversation_id: Some("conversation-a".to_owned()),
            turn_id: Some("durable-turn-a".to_owned()),
            run_id: Some("durable-run-a".to_owned()),
            prompt: "durable prompt a".to_owned(),
            output: "durable answer a".to_owned(),
            status: ConsoleRunStatus::Completed,
            detail: "restored".to_owned(),
            input_tokens: 0,
            output_tokens: 0,
        });
        type_prompt(&mut state, "prompt a");
        let request_a = state.submit().expect("submit a");
        let activity_a = controller.start(request_a).expect("start a");
        state
            .bind_activity(activity_a.clone())
            .expect("bind activity a");
        type_prompt(&mut state, "draft a");

        state.select_agent("agent-b", "Bob");
        state.conversation_id = Some("conversation-b".to_owned());
        type_prompt(&mut state, "prompt b");
        let request_b = state.submit().expect("submit b");
        let activity_b = controller.start(request_b).expect("start b");
        state
            .bind_activity(activity_b.clone())
            .expect("bind activity b");
        type_prompt(&mut state, "draft b");

        for _ in 0..2 {
            let update = tokio::time::timeout(Duration::from_secs(1), controller.recv_update())
                .await
                .expect("started update timeout")
                .expect("started update");
            assert!(matches!(update.event(), ControllerEvent::Started { .. }));
            state.apply_update(update);
        }
        assert_eq!(controller.active_run_count(), 2);
        assert_eq!(state.active_activity_count(), 2);

        gate_a.emit_output.notify_one();
        gate_b.emit_output.notify_one();
        for _ in 0..2 {
            let update = tokio::time::timeout(Duration::from_secs(1), controller.recv_update())
                .await
                .expect("output update timeout")
                .expect("output update");
            assert!(matches!(update.event(), ControllerEvent::Output(_)));
            state.apply_update(update);
        }

        assert!(state.select_activity_relative(-1));
        assert_eq!(state.selected_activity_id(), Some(activity_a.as_str()));
        assert_eq!(state.prompt_buffer, "draft a");
        assert_eq!(state.transcript.len(), 1);
        assert_eq!(
            state.run.as_ref().map(|run| run.output.as_str()),
            Some("output-agent-a")
        );
        assert_eq!(
            state.submit(),
            Err("this conversation already has an active Console turn")
        );
        assert_eq!(
            state.prompt_buffer, "draft a",
            "duplicate rejection preserves the draft"
        );

        assert!(controller.cancel_activity(&activity_a));
        state.mark_cancelling();
        let cancelled = tokio::time::timeout(Duration::from_secs(1), controller.recv_update())
            .await
            .expect("cancel update timeout")
            .expect("cancel update");
        assert_eq!(cancelled.activity_id(), Some(activity_a.as_str()));
        assert!(matches!(cancelled.event(), ControllerEvent::Cancelled(_)));
        state.apply_update(cancelled);
        assert_eq!(gate_a.cancellations.load(Ordering::SeqCst), 1);
        assert_eq!(gate_b.cancellations.load(Ordering::SeqCst), 0);
        assert_eq!(controller.active_run_count(), 1);

        gate_b.finish.notify_one();
        let completed = tokio::time::timeout(Duration::from_secs(1), controller.recv_update())
            .await
            .expect("completion update timeout")
            .expect("completion update");
        assert_eq!(completed.activity_id(), Some(activity_b.as_str()));
        assert!(matches!(
            completed.event(),
            ControllerEvent::Completed { .. }
        ));
        state.apply_update(completed);
        assert_eq!(controller.active_run_count(), 0);

        assert!(state.select_activity_relative(1));
        assert_eq!(state.selected_activity_id(), Some(activity_b.as_str()));
        assert_eq!(state.prompt_buffer, "draft b");
        assert_eq!(
            state.run.as_ref().map(|run| &run.status),
            Some(&ConsoleRunStatus::Completed)
        );
        assert_eq!(
            state.run.as_ref().map(|run| run.output.as_str()),
            Some("output-agent-b")
        );

        controller.shutdown().await;
        assert!(controller.tasks.is_empty());
    }

    #[test]
    fn cancel_command_targets_one_exact_runtime_turn_without_touching_other_activity() {
        const CHILD_TEST: &str = "tui::interaction::tests::cancel_command_targets_one_exact_runtime_turn_without_touching_other_activity_child";
        let output = std::process::Command::new(
            std::env::current_exe().expect("resolve current test binary"),
        )
        .args(["--exact", CHILD_TEST, "--nocapture"])
        .env("POLKAGENT_TUI_CANCEL_FIXTURE_KEY", "fixture-key")
        .output()
        .expect("run isolated provider-backed cancellation child");
        assert!(
            output.status.success(),
            "provider-backed cancellation child failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[tokio::test(flavor = "current_thread")]
    #[allow(
        clippy::too_many_lines,
        reason = "the real-runtime cancellation proof keeps two provider-backed turns, exact command scope, stale selection guards, and durable no-extra-work assertions together"
    )]
    async fn cancel_command_targets_one_exact_runtime_turn_without_touching_other_activity_child() {
        use tokio::io::{AsyncBufReadExt as _, BufReader};

        const API_KEY_ENV: &str = "POLKAGENT_TUI_CANCEL_FIXTURE_KEY";
        if std::env::var(API_KEY_ENV).as_deref() != Ok("fixture-key") {
            return;
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind delayed TUI provider");
        let address = listener.local_addr().expect("delayed provider address");
        let (request_tx, mut request_rx) = mpsc::channel(2);
        let provider_task = tokio::spawn(async move {
            let mut connections = tokio::task::JoinSet::new();
            for _ in 0..2 {
                let (stream, _) = listener.accept().await.expect("accept provider request");
                let mut reader = BufReader::new(stream);
                let mut request_line = String::new();
                reader
                    .read_line(&mut request_line)
                    .await
                    .expect("read provider request line");
                assert!(request_line.starts_with("POST /v1/chat/completions "));
                request_tx
                    .send(())
                    .await
                    .expect("signal delayed provider request");
                connections.spawn(async move {
                    std::future::pending::<()>().await;
                    drop(reader);
                });
            }
            while connections.join_next().await.is_some() {}
        });

        let temp = tempfile::TempDir::new().expect("tempdir");
        let database_path = temp.path().join("tui-cancel.db");
        let config_path = temp.path().join("polkagent.toml");
        std::fs::write(
            &config_path,
            format!(
                "[[providers]]\n\
                 id = \"cancel-provider\"\n\
                 provider_type = \"local\"\n\
                 base_url = \"http://{address}/v1\"\n\
                 api_key_env = \"{API_KEY_ENV}\"\n\
                 default_model = \"fixture-model\"\n"
            ),
        )
        .expect("write delayed provider config");
        let pool = SqlitePool::open(&database_path).expect("open database");
        migrations::migrate(&pool.writer()).expect("migrate database");
        let store = SqliteRunStore::new(pool.clone());
        let timestamp = "2026-01-01T00:00:00Z";
        let create_agent = |name: &str| {
            let spec = serde_json::json!({
                "name": name,
                "description": null,
                "model": "fixture/model",
                "tools": [],
                "system_prompt": null,
                "autonomy_level": "supervised",
                "created_at": timestamp,
                "updated_at": timestamp,
            });
            store
                .create_agent(name, None, &spec.to_string())
                .expect("create active agent")
        };
        let agent_a = create_agent("cancel-agent-a");
        let agent_b = create_agent("cancel-agent-b");
        let mut options =
            tui_runtime_options(&pool, Some(&config_path)).expect("TUI runtime options");
        options.workdir = temp.path().to_path_buf();
        options.disable_harness = true;
        options.discover_environment_providers = false;
        options.provider_override = Some("cancel-provider".to_owned());
        drop(store);
        drop(pool);
        let runtime = Box::pin(RuntimeFactory::build(options))
            .await
            .expect("build provider-backed TUI runtime");
        let mut controller = RunController::new(runtime.clone());
        let mut state = InteractionState::default();

        state.select_agent(agent_a.id.clone(), agent_a.name.clone());
        type_prompt(&mut state, "provider-backed prompt a");
        let activity_a = controller
            .start(state.submit().expect("submit prompt a"))
            .expect("start prompt a");
        state
            .bind_activity(activity_a.clone())
            .expect("bind prompt a");
        type_prompt(&mut state, "preserved draft a");
        state.select_agent(agent_b.id.clone(), agent_b.name.clone());
        type_prompt(&mut state, "provider-backed prompt b");
        let activity_b = controller
            .start(state.submit().expect("submit prompt b"))
            .expect("start prompt b");
        state
            .bind_activity(activity_b.clone())
            .expect("bind prompt b");
        type_prompt(&mut state, "preserved draft b");

        let mut started = 0;
        tokio::time::timeout(Duration::from_secs(10), async {
            while started < 2 {
                let update = controller
                    .recv_update()
                    .await
                    .expect("controller update channel remains open");
                if matches!(update.event(), ControllerEvent::Started { .. }) {
                    started += 1;
                }
                state.apply_update(update);
            }
            request_rx.recv().await.expect("first provider request");
            request_rx.recv().await.expect("second provider request");
        })
        .await
        .expect("provider-backed turns did not become active");
        assert_eq!(controller.active_run_count(), 2);

        assert!(state.select_activity_relative(-1));
        assert_eq!(state.selected_activity_id(), Some(activity_a.as_str()));
        let conversation_a = state.conversation_id.clone().expect("conversation a");
        let turn_a = state
            .run
            .as_ref()
            .and_then(|run| run.turn_id.clone())
            .expect("turn a");
        state.clear_prompt();
        type_prompt(&mut state, "/help cancel");
        let ConsoleCommandSubmission::Execute(help_request) = state
            .submit_command()
            .expect("submit selected cancellation help")
        else {
            panic!("selected cancellation help was rejected");
        };
        controller
            .execute_command(help_request)
            .expect("execute selected cancellation help");
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match controller
                    .recv_update()
                    .await
                    .expect("controller update channel remains open")
                {
                    ControllerUpdate::Control(event) if event.is_terminal() => {
                        state.apply(event);
                        break;
                    }
                    ControllerUpdate::Control(_) => {}
                    activity @ ControllerUpdate::Activity(_) => state.apply_update(activity),
                }
            }
        })
        .await
        .expect("selected cancellation help did not complete");
        let help = state.command_result.as_ref().expect("cancellation help");
        assert!(help
            .lines
            .iter()
            .any(|line| line.contains("/cancel (/stop)")));
        assert!(help
            .lines
            .iter()
            .any(|line| line.contains("run-ID and all-activity forms are unavailable")));

        state.clear_prompt();
        type_prompt(&mut state, "/cancel");
        let ConsoleCommandSubmission::Execute(cancel_request) = state
            .submit_command()
            .expect("submit selected cancellation")
        else {
            panic!("selected exact cancellation was rejected");
        };
        assert_eq!(
            cancel_request.conversation_id.as_deref(),
            Some(conversation_a.as_str())
        );
        assert_eq!(
            cancel_request
                .selected_turn_id
                .map(|id| id.to_string())
                .as_deref(),
            Some(turn_a.as_str())
        );
        let duplicate_request = ConsoleCommandRequest {
            request_id: uuid::Uuid::now_v7().to_string(),
            ..(*cancel_request).clone()
        };
        controller
            .execute_command(cancel_request)
            .expect("execute shared cancellation command");

        assert!(state.select_activity_relative(1));
        assert_eq!(state.selected_activity_id(), Some(activity_b.as_str()));
        assert_eq!(state.prompt_buffer, "preserved draft b");
        let conversation_b = state.conversation_id.clone().expect("conversation b");
        let turn_b = state
            .run
            .as_ref()
            .and_then(|run| run.turn_id.clone())
            .expect("turn b");

        let mut saw_command = false;
        let mut saw_cancelled_a = false;
        tokio::time::timeout(Duration::from_secs(10), async {
            while !(saw_command && saw_cancelled_a) {
                let update = controller
                    .recv_update()
                    .await
                    .expect("controller update channel remains open");
                match &update {
                    ControllerUpdate::Control(ControllerEvent::CommandCompleted { .. }) => {
                        saw_command = true;
                    }
                    ControllerUpdate::Activity(activity)
                        if activity.activity_id == activity_a
                            && matches!(activity.event, ControllerEvent::Cancelled(_)) =>
                    {
                        saw_cancelled_a = true;
                    }
                    _ => {}
                }
                state.apply_update(update);
            }
        })
        .await
        .expect("exact cancellation did not complete");
        assert_eq!(controller.active_run_count(), 1);
        assert_eq!(state.selected_activity_id(), Some(activity_b.as_str()));
        assert_eq!(
            state.conversation_id.as_deref(),
            Some(conversation_b.as_str())
        );
        assert_eq!(state.prompt_buffer, "preserved draft b");
        assert_eq!(
            state.run.as_ref().map(|run| &run.status),
            Some(&ConsoleRunStatus::Running)
        );
        assert!(state.command_result.is_none());

        controller
            .execute_command(Box::new(duplicate_request))
            .expect("execute duplicate terminal cancellation");
        let duplicate = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let ControllerUpdate::Control(event) = controller
                    .recv_update()
                    .await
                    .expect("controller update channel remains open")
                {
                    if event.is_terminal() {
                        break event;
                    }
                }
            }
        })
        .await
        .expect("duplicate cancellation did not report a result");
        assert!(matches!(duplicate, ControllerEvent::CommandFailed { .. }));
        state.apply(duplicate);
        assert_eq!(state.selected_activity_id(), Some(activity_b.as_str()));
        assert_eq!(state.prompt_buffer, "preserved draft b");

        assert!(controller.cancel_activity(&activity_b));
        state.mark_cancelling();
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let update = controller
                    .recv_update()
                    .await
                    .expect("controller update channel remains open");
                let cancelled_b = matches!(
                    &update,
                    ControllerUpdate::Activity(activity)
                        if activity.activity_id == activity_b
                            && matches!(activity.event, ControllerEvent::Cancelled(_))
                );
                state.apply_update(update);
                if cancelled_b {
                    break;
                }
            }
        })
        .await
        .expect("shortcut cancellation for activity b did not complete");
        assert_eq!(controller.active_run_count(), 0);
        assert_eq!(state.prompt_buffer, "preserved draft b");

        let (turn_count, run_count, cancelled_turns) = {
            let writer = runtime.pool().writer();
            let turn_count: i64 = writer
                .query_row("SELECT COUNT(*) FROM interaction_turns", [], |row| {
                    row.get(0)
                })
                .expect("count durable turns");
            let run_count: i64 = writer
                .query_row("SELECT COUNT(*) FROM runs", [], |row| row.get(0))
                .expect("count durable runs");
            let cancelled_turns: i64 = writer
                .query_row(
                    "SELECT COUNT(*) FROM interaction_turns WHERE state = 'cancelled'",
                    [],
                    |row| row.get(0),
                )
                .expect("count cancelled turns");
            (turn_count, run_count, cancelled_turns)
        };
        assert_eq!((turn_count, run_count, cancelled_turns), (2, 2, 2));
        let turn_b_state = runtime
            .interactions()
            .list_turns(conversation_b.parse().expect("conversation b UUID"))
            .await
            .expect("load conversation b turns")
            .into_iter()
            .find(|turn| turn.handle.turn_id.to_string() == turn_b)
            .map(|turn| turn.state);
        assert_eq!(turn_b_state, Some(TurnState::Cancelled));

        provider_task.abort();
        controller.shutdown().await;
    }

    #[tokio::test(flavor = "current_thread")]
    #[allow(clippy::too_many_lines)]
    async fn run_commands_are_scoped_and_non_blocking_during_concurrent_activities() {
        let (_temp, runtime) = Box::pin(controller_test_runtime()).await;
        let agent_a = AgentId::new().to_string();
        let agent_b = AgentId::new().to_string();
        let conversation_a = ConversationId::new().to_string();
        let conversation_b = ConversationId::new().to_string();
        let run_a = RunId::new().to_string();
        let run_b = RunId::new().to_string();
        let timestamp = "2026-01-01T00:00:00Z";
        {
            let connection = runtime.pool().writer();
            connection
                .execute(
                    "INSERT INTO agents
                     (id, name, description, state, spec_json, created_at, updated_at)
                     VALUES (?1, ?2, NULL, 'active', '{}', ?3, ?3)",
                    [agent_a.as_str(), "Alice", timestamp],
                )
                .expect("seed first agent");
            connection
                .execute(
                    "INSERT INTO agents
                     (id, name, description, state, spec_json, created_at, updated_at)
                     VALUES (?1, ?2, NULL, 'active', '{}', ?3, ?3)",
                    [agent_b.as_str(), "Bob", timestamp],
                )
                .expect("seed second agent");
            connection
                .execute(
                    "INSERT INTO runs
                     (id, agent_id, conversation_id, state, params_json, created_at, updated_at)
                     VALUES (?1, ?2, ?3, 'completed', '{\"prompt\":\"private a\"}', ?4, ?4)",
                    [
                        run_a.as_str(),
                        agent_a.as_str(),
                        conversation_a.as_str(),
                        timestamp,
                    ],
                )
                .expect("seed selected run");
            connection
                .execute(
                    "INSERT INTO runs
                     (id, agent_id, conversation_id, state, params_json, created_at, updated_at)
                     VALUES (?1, ?2, ?3, 'failed:private reason', '{\"prompt\":\"private b\"}', ?4, ?4)",
                    [
                        run_b.as_str(),
                        agent_b.as_str(),
                        conversation_b.as_str(),
                        timestamp,
                    ],
                )
                .expect("seed foreign run");
        }

        let gate_a = Arc::new(ControlledFakeRun::default());
        let gate_b = Arc::new(ControlledFakeRun::default());
        let gates = Arc::new(BTreeMap::from([
            (agent_a.clone(), Arc::clone(&gate_a)),
            (agent_b.clone(), Arc::clone(&gate_b)),
        ]));
        let mut controller = RunController::with_test_run_worker(
            runtime.clone(),
            controlled_worker(gates),
            Duration::from_millis(100),
        );
        let mut state = InteractionState::default();

        state.select_agent(agent_a.clone(), "Alice");
        state.conversation_id = Some(conversation_a.clone());
        type_prompt(&mut state, "active prompt a");
        let activity_a = controller
            .start(state.submit().expect("submit a"))
            .expect("start a");
        state
            .bind_activity(activity_a.clone())
            .expect("bind activity a");

        state.select_agent(agent_b.clone(), "Bob");
        state.conversation_id = Some(conversation_b.clone());
        type_prompt(&mut state, "active prompt b");
        let activity_b = controller
            .start(state.submit().expect("submit b"))
            .expect("start b");
        state.bind_activity(activity_b).expect("bind activity b");

        for _ in 0..2 {
            let update = tokio::time::timeout(Duration::from_secs(1), controller.recv_update())
                .await
                .expect("started update timeout")
                .expect("started update");
            assert!(matches!(update.event(), ControllerEvent::Started { .. }));
            state.apply_update(update);
        }
        assert!(state.select_activity_relative(-1));
        assert_eq!(state.selected_activity_id(), Some(activity_a.as_str()));
        assert_eq!(
            state.conversation_id.as_deref(),
            Some(conversation_a.as_str())
        );
        let activities_before = state.activity_summaries();

        type_prompt(&mut state, "/runs");
        let ConsoleCommandSubmission::Execute(request) =
            state.submit_command().expect("runs command")
        else {
            panic!("runs command rejected")
        };
        controller
            .execute_command(request)
            .expect("execute runs command alongside active work");
        state.apply(wait_for_control_terminal(&mut controller).await);
        let runs = state.command_result.as_ref().expect("runs result");
        assert_eq!(runs.status, ConsoleCommandStatus::Completed);
        assert!(runs.lines.join("\n").contains(&run_a));
        assert!(!runs.lines.join("\n").contains(&run_b));
        assert_eq!(controller.active_run_count(), 2);
        assert_eq!(state.active_activity_count(), 2);
        assert_eq!(state.activity_summaries(), activities_before);
        assert_eq!(
            runtime
                .pool()
                .writer()
                .query_row("SELECT COUNT(*) FROM runs", [], |row| row.get::<_, i64>(0))
                .expect("count runs after list"),
            2,
            "read commands must not create durable runs"
        );

        type_prompt(&mut state, &format!("/inspect {run_a}"));
        let ConsoleCommandSubmission::Execute(request) =
            state.submit_command().expect("inspect selected run")
        else {
            panic!("inspect selected run rejected")
        };
        controller
            .execute_command(request)
            .expect("execute selected inspection");
        state.apply(wait_for_control_terminal(&mut controller).await);
        let inspection = state.command_result.as_ref().expect("inspection result");
        assert_eq!(inspection.status, ConsoleCommandStatus::Completed);
        assert!(inspection.lines.join("\n").contains(&run_a));

        type_prompt(&mut state, &format!("/inspect {run_b}"));
        let ConsoleCommandSubmission::Execute(request) =
            state.submit_command().expect("inspect foreign run")
        else {
            panic!("foreign inspect rejected before scoped service lookup")
        };
        controller
            .execute_command(request)
            .expect("execute foreign inspection");
        state.apply(wait_for_control_terminal(&mut controller).await);
        let refusal = state.command_result.as_ref().expect("foreign refusal");
        assert_eq!(refusal.status, ConsoleCommandStatus::Failed);
        assert!(refusal.lines.join("\n").contains("not_found"));
        assert!(!refusal.lines.join("\n").contains(&conversation_b));
        let foreign_refusal = refusal.lines.clone();

        let missing_run = RunId::new();
        type_prompt(&mut state, &format!("/inspect {missing_run}"));
        let ConsoleCommandSubmission::Execute(request) =
            state.submit_command().expect("inspect missing run")
        else {
            panic!("missing inspect rejected before scoped service lookup")
        };
        controller
            .execute_command(request)
            .expect("execute missing inspection");
        state.apply(wait_for_control_terminal(&mut controller).await);
        let missing_refusal = state.command_result.as_ref().expect("missing refusal");
        assert_eq!(missing_refusal.status, ConsoleCommandStatus::Failed);
        assert_eq!(missing_refusal.lines, foreign_refusal);
        assert_eq!(controller.active_run_count(), 2);
        assert_eq!(state.activity_summaries(), activities_before);

        controller.shutdown().await;
        assert_eq!(gate_a.cancellations.load(Ordering::SeqCst), 1);
        assert_eq!(gate_b.cancellations.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn approval_controller_fails_closed_without_composed_authority() {
        let (temp, runtime) = Box::pin(controller_test_runtime()).await;
        let agent_id = AgentId::new();
        runtime
            .pool()
            .writer()
            .execute(
                "INSERT INTO agents
                 (id, name, description, state, spec_json, created_at, updated_at)
                 VALUES (?1, 'approval-fixture', NULL, 'active', '{}', ?2, ?2)",
                [agent_id.to_string(), "2026-01-01T00:00:00Z".to_owned()],
            )
            .expect("seed approval fixture agent");
        let interaction = runtime
            .interactions()
            .new_interaction(CreateInteractionRequest {
                title: Some("approval authority fixture".to_owned()),
                config: InteractionConfig::new(InteractionTarget::Agent(agent_id)),
                client_context: ClientContext::new(temp.path().to_path_buf())
                    .expect("fixture client context"),
            })
            .await
            .expect("create durable interaction");
        let conversation_id = interaction.conversation_id;
        let mut controller = RunController::new(runtime);
        let request_id = uuid::Uuid::now_v7();
        controller
            .list_approvals(ApprovalListRequest {
                request_id,
                conversation_id,
            })
            .expect("queue asynchronous approval list");
        let update = tokio::time::timeout(Duration::from_secs(1), controller.recv_update())
            .await
            .expect("approval unavailable timeout")
            .expect("approval unavailable update");
        assert!(
            matches!(
                update.event(),
                ControllerEvent::ApprovalListFailed {
                    request_id: actual_request,
                    conversation_id: actual_conversation,
                    code: InteractionErrorCode::Unavailable,
                    reason,
                } if *actual_request == request_id
                    && *actual_conversation == conversation_id
                    && reason.contains("approval")
            ),
            "unexpected unavailable approval event: {update:?}"
        );
        assert!(!controller.is_control_active());
        controller.shutdown().await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn approval_controller_preserves_scope_retry_conflict_and_restart_semantics() {
        let (_temp, runtime) = Box::pin(controller_test_runtime()).await;
        let conversation_id = ConversationId::new();
        let wrong_conversation_id = ConversationId::new();
        let inner: Arc<dyn InteractionService> = runtime.interactions().clone();
        let fixture = Arc::new(ApprovalFixtureService::new(inner, conversation_id));
        let approval_id = fixture.approval_id();

        let service: Arc<dyn InteractionService> = fixture.clone();
        let activity_gate = Arc::new(ControlledFakeRun::default());
        let activity_agent = AgentId::new().to_string();
        let mut controller = RunController::with_test_run_worker(
            runtime.clone(),
            controlled_worker(Arc::new(BTreeMap::from([(
                activity_agent.clone(),
                Arc::clone(&activity_gate),
            )]))),
            Duration::from_millis(100),
        )
        .with_test_approval_service(service);
        controller
            .start(PromptRequest {
                agent_id: activity_agent,
                agent_name: "Active agent".to_owned(),
                conversation_id: Some(conversation_id.to_string()),
                prompt: "keep running".to_owned(),
            })
            .expect("start concurrent Console activity");
        let started = controller
            .recv_update()
            .await
            .expect("concurrent activity start");
        assert!(matches!(started.event(), ControllerEvent::Started { .. }));
        assert_eq!(controller.active_run_count(), 1);
        let wrong_request = uuid::Uuid::now_v7();
        controller
            .list_approvals(ApprovalListRequest {
                request_id: wrong_request,
                conversation_id: wrong_conversation_id,
            })
            .expect("queue wrong-scope list");
        let wrong = controller.recv_update().await.expect("wrong-scope update");
        assert!(matches!(
            wrong.event(),
            ControllerEvent::ApprovalListFailed {
                request_id,
                conversation_id,
                code: InteractionErrorCode::PermissionDenied,
                ..
            } if *request_id == wrong_request && *conversation_id == wrong_conversation_id
        ));
        assert_eq!(controller.active_run_count(), 1);

        let list_request = uuid::Uuid::now_v7();
        controller
            .list_approvals(ApprovalListRequest {
                request_id: list_request,
                conversation_id,
            })
            .expect("queue exact list");
        let listed = controller.recv_update().await.expect("exact list update");
        assert!(matches!(
            listed.event(),
            ControllerEvent::ApprovalListLoaded { approvals, .. }
                if approvals.len() == 1 && approvals[0].approval_id == approval_id
        ));
        assert_eq!(controller.active_run_count(), 1);
        controller.shutdown().await;
        assert_eq!(activity_gate.cancellations.load(Ordering::SeqCst), 1);

        let service: Arc<dyn InteractionService> = fixture.clone();
        let mut restarted = RunController::new(runtime).with_test_approval_service(service);
        let restart_list_request = uuid::Uuid::now_v7();
        restarted
            .list_approvals(ApprovalListRequest {
                request_id: restart_list_request,
                conversation_id,
            })
            .expect("queue list after controller restart");
        let restarted_list = restarted.recv_update().await.expect("restart list update");
        assert!(matches!(
            restarted_list.event(),
            ControllerEvent::ApprovalListLoaded { approvals, .. }
                if approvals.len() == 1 && approvals[0].approval_id == approval_id
        ));

        for _ in 0..2 {
            let request_id = uuid::Uuid::now_v7();
            restarted
                .decide_approval(ApprovalDecisionRequest {
                    request_id,
                    conversation_id,
                    approval_id,
                    decision: ApprovalDecision::Approve,
                })
                .expect("queue idempotent approval");
            let update = restarted.recv_update().await.expect("approval completion");
            assert!(matches!(
                update.event(),
                ControllerEvent::ApprovalDecisionCompleted {
                    approval,
                    decision: ApprovalDecision::Approve,
                    ..
                } if approval.approval_id == approval_id
                    && approval.status == polkagent_interaction::ApprovalStatus::Approved
            ));
        }

        let conflict_request = uuid::Uuid::now_v7();
        restarted
            .decide_approval(ApprovalDecisionRequest {
                request_id: conflict_request,
                conversation_id,
                approval_id,
                decision: ApprovalDecision::Deny {
                    reason: Some("changed decision".to_owned()),
                },
            })
            .expect("queue conflicting denial");
        let conflict = restarted.recv_update().await.expect("conflict completion");
        assert!(matches!(
            conflict.event(),
            ControllerEvent::ApprovalDecisionFailed {
                code: InteractionErrorCode::Conflict,
                ..
            }
        ));
        restarted.shutdown().await;
    }

    #[tokio::test(flavor = "current_thread")]
    #[allow(clippy::too_many_lines)]
    async fn console_approval_commands_preserve_scope_retry_redaction_and_unrelated_activity() {
        let (_temp, runtime) = Box::pin(controller_test_runtime()).await;
        let conversation_id = ConversationId::new();
        let wrong_conversation_id = ConversationId::new();
        let inner: Arc<dyn InteractionService> = runtime.interactions().clone();
        let fixture = Arc::new(ApprovalFixtureService::new(inner, conversation_id));
        let approval_id = fixture.approval_id();
        let service: Arc<dyn InteractionService> = fixture;
        let gate = Arc::new(ControlledFakeRun::default());
        let agent_id = AgentId::new().to_string();
        let mut controller = RunController::with_test_run_worker(
            runtime.clone(),
            controlled_worker(Arc::new(BTreeMap::from([(
                agent_id.clone(),
                Arc::clone(&gate),
            )]))),
            Duration::from_millis(100),
        )
        .with_test_approval_service(service);

        let mut state = InteractionState::default();
        state.select_agent(agent_id.clone(), "Approval agent");
        state.conversation_id = Some(conversation_id.to_string());
        type_prompt(&mut state, "keep unrelated work waiting");
        let prompt = state.submit().expect("submit controlled activity");
        let activity_id = controller.start(prompt).expect("start controlled activity");
        state
            .bind_activity(activity_id)
            .expect("bind controlled activity");
        state.apply_update(
            controller
                .recv_update()
                .await
                .expect("controlled activity started"),
        );
        assert_eq!(controller.active_run_count(), 1);
        let activity_before = state.activity_summaries();
        let initial_turn_count: i64 = runtime
            .pool()
            .writer()
            .query_row("SELECT COUNT(*) FROM interaction_turns", [], |row| {
                row.get(0)
            })
            .expect("count initial interaction turns");

        let projected = ConsoleApprovalContext::new(conversation_id, vec![approval_id]);
        type_prompt(&mut state, &format!("/approve {approval_id}"));
        let ConsoleCommandSubmission::Execute(request) = state
            .submit_command_with_approval_context(Some(&projected))
            .expect("submit projected approval")
        else {
            panic!("projected approval was rejected")
        };
        let mut switched = state.clone();
        switched.replace_conversation(Some(wrong_conversation_id.to_string()), None, Vec::new());
        type_prompt(&mut switched, "preserved switched draft");

        controller
            .execute_command(request)
            .expect("execute shared approval command");
        let completed = wait_for_control_terminal(&mut controller).await;
        assert!(matches!(
            completed,
            ControllerEvent::CommandCompleted { .. }
        ));
        state.apply(completed.clone());
        switched.apply(completed);
        assert_eq!(switched.prompt_buffer, "preserved switched draft");
        assert_ne!(
            switched
                .command_result
                .as_ref()
                .and_then(|result| result.approval_resolution.as_ref())
                .map(|resolution| resolution.approval_id),
            Some(approval_id),
            "stale completion crossed the selected conversation"
        );
        assert_eq!(controller.active_run_count(), 1);
        assert_eq!(state.activity_summaries(), activity_before);

        let refresh_id = uuid::Uuid::now_v7();
        controller
            .list_approvals(ApprovalListRequest {
                request_id: refresh_id,
                conversation_id,
            })
            .expect("refresh shared approval queue");
        let refreshed = controller.recv_update().await.expect("refreshed queue");
        assert!(matches!(
            refreshed.event(),
            ControllerEvent::ApprovalListLoaded { approvals, .. } if approvals.is_empty()
        ));
        let empty_projection = ConsoleApprovalContext::new(conversation_id, Vec::new());

        type_prompt(&mut state, &format!("/approve {approval_id}"));
        let ConsoleCommandSubmission::Execute(retry) = state
            .submit_command_with_approval_context(Some(&empty_projection))
            .expect("submit exact retry after refresh")
        else {
            panic!("exact retry was rejected after refresh")
        };
        controller
            .execute_command(retry)
            .expect("execute idempotent retry");
        let retry = wait_for_control_terminal(&mut controller).await;
        assert!(matches!(retry, ControllerEvent::CommandCompleted { .. }));
        state.apply(retry);

        let secret = "sk-approval-secret-123456789";
        type_prompt(
            &mut state,
            &format!("/deny {approval_id} changed decision {secret}"),
        );
        let ConsoleCommandSubmission::Execute(conflict) = state
            .submit_command_with_approval_context(Some(&empty_projection))
            .expect("submit exact opposite retry")
        else {
            panic!("exact opposite retry was rejected before durable CAS")
        };
        controller
            .execute_command(conflict)
            .expect("execute conflicting retry");
        state.apply(wait_for_control_terminal(&mut controller).await);
        let refusal = state.command_result.as_ref().expect("conflict result");
        assert_eq!(refusal.status, ConsoleCommandStatus::Failed);
        assert!(refusal.lines.join("\n").contains("conflict"));
        assert!(!refusal.lines.join("\n").contains(secret));
        assert!(refusal.lines.join("\n").contains("REDACTED"));
        assert!(!refusal.line.contains(secret));
        assert!(refusal.line.contains("[reason redacted]"));
        assert!(!state
            .prompt_history
            .iter()
            .any(|line| line.contains(secret)));
        assert_eq!(
            refusal
                .approval_resolution
                .as_ref()
                .map(|resolution| resolution.approval_id),
            Some(approval_id),
            "opposite conflict erased the exact retry marker"
        );

        type_prompt(&mut state, &format!("/approve {approval_id}"));
        assert!(matches!(
            state.submit_command_with_approval_context(Some(&empty_projection)),
            Ok(ConsoleCommandSubmission::Execute(_))
        ));

        let final_turn_count: i64 = runtime
            .pool()
            .writer()
            .query_row("SELECT COUNT(*) FROM interaction_turns", [], |row| {
                row.get(0)
            })
            .expect("count final interaction turns");
        assert_eq!(final_turn_count, initial_turn_count);
        assert_eq!(controller.active_run_count(), 1);
        controller.shutdown().await;
        assert_eq!(gate.cancellations.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn pending_approval_no_longer_blocks_tui_turn_cancellation_dispatch() {
        let (_temp, runtime) = Box::pin(controller_test_runtime()).await;
        let gate = Arc::new(ControlledFakeRun::default());
        let agent_id = AgentId::new().to_string();
        let conversation_id = ConversationId::new().to_string();
        let mut controller = RunController::with_test_run_worker(
            runtime,
            controlled_worker(Arc::new(BTreeMap::from([(
                agent_id.clone(),
                Arc::clone(&gate),
            )]))),
            Duration::from_millis(25),
        );
        let activity_id = controller
            .start(PromptRequest {
                agent_id: agent_id.clone(),
                agent_name: "Approval waiter".to_owned(),
                conversation_id: Some(conversation_id.clone()),
                prompt: "wait for approval".to_owned(),
            })
            .expect("start approval waiter");
        let started = controller.recv_update().await.expect("started update");
        assert!(matches!(started.event(), ControllerEvent::Started { .. }));

        controller
            .run_event_tx
            .send(RunActivityUpdate {
                activity_id: activity_id.clone(),
                agent_id,
                requested_conversation_id: Some(conversation_id),
                event: ControllerEvent::TurnApprovalRequested {
                    approval_id: ApprovalId::new(),
                },
            })
            .await
            .expect("queue approval request update");
        let requested = controller
            .recv_update()
            .await
            .expect("approval request update");
        assert!(matches!(
            requested.event(),
            ControllerEvent::TurnApprovalRequested { .. }
        ));
        assert!(controller.cancel_activity(&activity_id));
        let cancelled = controller
            .recv_update()
            .await
            .expect("worker cancellation update");
        assert!(matches!(cancelled.event(), ControllerEvent::Cancelled(_)));
        assert_eq!(gate.cancellations.load(Ordering::SeqCst), 1);

        controller.shutdown().await;
        assert_eq!(
            gate.cancellations.load(Ordering::SeqCst),
            1,
            "shutdown must not duplicate a dispatched turn cancellation"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn shutdown_dispatches_pending_approval_cancellation_through_the_real_service() {
        use crate::commands::chat::tests::{
            chat_approval_service, persist_agent, seed_chat_approval,
        };

        let (_controller_temp, runtime) = Box::pin(controller_test_runtime()).await;
        let approval_temp = tempfile::TempDir::new().expect("approval tempdir");
        let pool = SqlitePool::open(approval_temp.path().join("tui-shutdown-approval.db"))
            .expect("open shutdown approval database");
        migrations::migrate(&pool.writer()).expect("migrate shutdown approval database");
        let agent_id = AgentId::new();
        persist_agent(
            &pool,
            &AgentSpec::new(agent_id, "shutdown-approval-agent", "fake/model"),
        );
        let fixture = Box::pin(seed_chat_approval(&pool, agent_id)).await;
        let service = chat_approval_service(&pool, &fixture.authority);
        assert_eq!(
            service
                .list_pending_approvals(fixture.conversation_id)
                .await
                .expect("list approval before TUI shutdown")
                .len(),
            1
        );

        let cancellation_service = Arc::clone(&service);
        let cancellation_turn_id = fixture.interaction_turn_id;
        let started_run_id = fixture.run_id.to_string();
        let worker: TestRunWorker = Arc::new(move |request, context, event_tx, mut cancel_rx| {
            let service = Arc::clone(&cancellation_service);
            let run_id = started_run_id.clone();
            Box::pin(async move {
                let _ = event_tx
                    .send(
                        context.update(ControllerEvent::Started {
                            conversation_id: request
                                .conversation_id
                                .clone()
                                .unwrap_or_else(|| "shutdown-conversation".to_owned()),
                            model: Some("fake/controlled".to_owned()),
                            turn_id: cancellation_turn_id.to_string(),
                            run_id,
                            agent_name: request.agent_name,
                            notes: vec!["real approval cancellation fixture".to_owned()],
                        }),
                    )
                    .await;
                if (&mut cancel_rx).await.is_ok() {
                    request_turn_cancellation(
                        service.as_ref(),
                        cancellation_turn_id,
                        &event_tx,
                        &context,
                    )
                    .await;
                }
            })
        });
        let mut controller =
            RunController::with_test_run_worker(runtime, worker, Duration::from_millis(250));
        controller
            .start(PromptRequest {
                agent_id: agent_id.to_string(),
                agent_name: "Shutdown approval agent".to_owned(),
                conversation_id: Some(fixture.conversation_id.to_string()),
                prompt: "wait for approval".to_owned(),
            })
            .expect("start shutdown approval activity");
        let started = controller.recv_update().await.expect("started update");
        assert!(matches!(started.event(), ControllerEvent::Started { .. }));

        controller.shutdown().await;

        assert!(service
            .list_pending_approvals(fixture.conversation_id)
            .await
            .expect("list approvals after TUI shutdown")
            .is_empty());
        let writer = pool.writer();
        let (run_state, attempts): (String, i64) = writer
            .query_row(
                "SELECT state,
                        (SELECT COUNT(*) FROM effect_attempts WHERE intent_id = ?2)
                   FROM runs WHERE id = ?1",
                rusqlite::params![fixture.run_id.to_string(), fixture.effect_id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("load TUI shutdown cancellation outcome");
        assert_eq!(run_state, "cancelled:approval_cancelled");
        assert_eq!(
            attempts, 0,
            "shutdown cancellation must perform no tool I/O"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn controller_enforces_run_capacity_and_channel_backpressure() {
        let (_temp, runtime) = Box::pin(controller_test_runtime()).await;
        let gates = Arc::new(
            (0..MAX_CONCURRENT_CONSOLE_RUNS)
                .map(|index| {
                    (
                        format!("agent-{index}"),
                        Arc::new(ControlledFakeRun::default()),
                    )
                })
                .collect::<BTreeMap<_, _>>(),
        );
        let mut controller = RunController::with_test_run_worker(
            runtime,
            controlled_worker(Arc::clone(&gates)),
            Duration::from_millis(100),
        );
        let first = PromptRequest {
            agent_id: "agent-0".to_owned(),
            agent_name: "Agent 0".to_owned(),
            conversation_id: Some("conversation-0".to_owned()),
            prompt: "first".to_owned(),
        };
        controller.start(first.clone()).expect("first start");
        assert_eq!(
            controller.start(first),
            Err("this conversation already has an active Console turn")
        );
        for index in 1..MAX_CONCURRENT_CONSOLE_RUNS {
            controller
                .start(PromptRequest {
                    agent_id: format!("agent-{index}"),
                    agent_name: format!("Agent {index}"),
                    conversation_id: Some(format!("conversation-{index}")),
                    prompt: format!("prompt {index}"),
                })
                .expect("start within capacity");
        }
        assert_eq!(controller.active_run_count(), MAX_CONCURRENT_CONSOLE_RUNS);
        assert_eq!(
            controller.start(PromptRequest {
                agent_id: "overflow-agent".to_owned(),
                agent_name: "Overflow".to_owned(),
                conversation_id: Some("overflow-conversation".to_owned()),
                prompt: "overflow".to_owned(),
            }),
            Err("Console concurrent-run capacity is full")
        );

        controller.shutdown().await;
        assert_eq!(controller.active_run_count(), 0);
        assert!(controller.tasks.is_empty());
        while controller.try_recv_update().is_some() {}

        let sample_context = RunActivityContext {
            activity: "backpressure".to_owned(),
            agent: "agent".to_owned(),
            requested_conversation: None,
        };
        for index in 0..CONTROLLER_EVENT_CAPACITY {
            controller
                .run_event_tx
                .try_send(sample_context.update(ControllerEvent::Progress(index.to_string())))
                .expect("bounded channel accepts through capacity");
        }
        assert!(matches!(
            controller
                .run_event_tx
                .try_send(sample_context.update(ControllerEvent::Progress("overflow".to_owned()))),
            Err(mpsc::error::TrySendError::Full(_))
        ));
    }

    #[test]
    fn activity_retention_evicts_oldest_terminal_and_debug_is_redacted() {
        let mut state = InteractionState::default();
        for index in 0..=MAX_RETAINED_CONSOLE_ACTIVITIES {
            let agent_id = format!("agent-{index}");
            let conversation_id = format!("conversation-{index}");
            let activity_id = format!("activity-{index}");
            state.select_agent(agent_id.clone(), format!("Agent {index}"));
            state.conversation_id = Some(conversation_id.clone());
            type_prompt(&mut state, &format!("private-prompt-{index}"));
            state.submit().expect("submit retained activity");
            state
                .bind_activity(activity_id.clone())
                .expect("bind retained activity");
            state.apply_update(ControllerUpdate::Activity(RunActivityUpdate {
                activity_id,
                agent_id,
                requested_conversation_id: Some(conversation_id),
                event: ControllerEvent::Failed(format!("private-error-{index}")),
            }));
        }

        let summaries = state.activity_summaries();
        assert_eq!(summaries.len(), MAX_RETAINED_CONSOLE_ACTIVITIES);
        assert_eq!(
            summaries
                .first()
                .map(|summary| summary.activity_id.as_str()),
            Some("activity-1")
        );
        assert_eq!(
            summaries.last().map(|summary| summary.activity_id.as_str()),
            Some("activity-32")
        );
        let debug = format!("{state:?}");
        assert!(!debug.contains("private-prompt"));
        assert!(!debug.contains("private-error"));
        assert!(debug.contains("activity-32"));
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
                let event = controller
                    .recv()
                    .await
                    .expect("controller event channel remains open");
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
        })
        .await
        .expect("controller terminal event timeout")
    }

    async fn wait_for_control_terminal(controller: &mut RunController) -> ControllerEvent {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match controller
                    .recv_update()
                    .await
                    .expect("controller update channel remains open")
                {
                    ControllerUpdate::Control(event) if event.is_terminal() => return event,
                    ControllerUpdate::Control(_) => {}
                    ControllerUpdate::Activity(update) => panic!(
                        "unexpected activity update while waiting for command: {:?}",
                        update.event
                    ),
                }
            }
        })
        .await
        .expect("controller command event timeout")
    }

    async fn wait_for_history(controller: &mut RunController) -> ControllerEvent {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let event = controller
                    .recv()
                    .await
                    .expect("controller event channel remains open");
                if matches!(
                    event,
                    ControllerEvent::HistoryLoaded { .. } | ControllerEvent::HistoryFailed { .. }
                ) {
                    return event;
                }
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
        assert!(options.approval_authority.is_none());

        let authority = InteractionApprovalAuthority {
            tenant_id: "tenant-a".to_owned(),
            workspace_id: "workspace-a".to_owned(),
            principal_id: PrincipalId::new(),
            surface: "tui".to_owned(),
        };
        let approval_options =
            tui_runtime_options_with_approval(&pool, None, Some(authority.clone()))
                .expect("approval-enabled TUI runtime options");
        assert_eq!(approval_options.approval_authority, Some(authority));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn explicit_tui_authority_composes_the_production_approval_runtime() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let database_path = temp.path().join("tui-authority.db");
        let pool = SqlitePool::open(&database_path).expect("open TUI authority database");
        migrations::migrate(&pool.writer()).expect("migrate TUI authority database");
        let authority = InteractionApprovalAuthority {
            tenant_id: "tenant-a".to_owned(),
            workspace_id: "workspace-a".to_owned(),
            principal_id: PrincipalId::new(),
            surface: "tui".to_owned(),
        };
        let mut options = tui_runtime_options_with_approval(&pool, None, Some(authority))
            .expect("approval-enabled TUI runtime options");
        options.workdir = temp.path().to_path_buf();
        options.disable_harness = true;
        options.discover_environment_providers = false;
        drop(pool);

        let runtime = Box::pin(RuntimeFactory::build(options))
            .await
            .expect("build approval-enabled TUI runtime");
        assert_eq!(
            runtime.readiness().approval_executor.state,
            ComponentState::Ready
        );
        assert_eq!(
            runtime.readiness().approval_surfaces.state,
            ComponentState::Ready
        );
    }

    #[tokio::test(flavor = "current_thread")]
    #[allow(clippy::too_many_lines)]
    async fn console_approval_command_uses_production_authority_and_creates_no_model_turn() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let database_path = temp.path().join("tui-command-approval.db");
        let pool = SqlitePool::open(&database_path).expect("open TUI approval database");
        migrations::migrate(&pool.writer()).expect("migrate TUI approval database");
        let store = SqliteRunStore::new(pool.clone());
        let agent = store
            .create_agent(
                "approval-command-agent",
                None,
                &serde_json::json!({
                    "name": "approval-command-agent",
                    "description": null,
                    "model": "fake/default",
                    "tools": [],
                    "system_prompt": null,
                    "autonomy_level": "supervised",
                })
                .to_string(),
            )
            .expect("create approval command agent");
        let fixture = crate::commands::chat::tests::seed_chat_approval(
            &pool,
            agent.id.parse().expect("agent UUID"),
        )
        .await;

        let mut wrong_authority = fixture.authority.clone();
        wrong_authority.principal_id = PrincipalId::new();
        let mut wrong_options =
            tui_runtime_options_with_approval(&pool, None, Some(wrong_authority))
                .expect("wrong-principal runtime options");
        wrong_options.workdir = temp.path().to_path_buf();
        wrong_options.disable_harness = true;
        wrong_options.discover_environment_providers = false;
        let wrong_error = Box::pin(RuntimeFactory::build(wrong_options))
            .await
            .expect_err("wrong-principal runtime must fail closed during recovery");
        assert!(format!("{wrong_error:#}").contains("permission_denied"));

        let mut options =
            tui_runtime_options_with_approval(&pool, None, Some(fixture.authority.clone()))
                .expect("authorized runtime options");
        options.workdir = temp.path().to_path_buf();
        options.disable_harness = true;
        options.discover_environment_providers = false;
        drop(store);
        drop(pool);
        let runtime = Box::pin(RuntimeFactory::build(options))
            .await
            .expect("build authorized production runtime");
        let approval = runtime
            .interactions()
            .list_pending_approvals(fixture.conversation_id)
            .await
            .expect("list production pending approval")
            .into_iter()
            .next()
            .expect("seeded pending approval");
        let approval_id = approval.approval_id;
        let projection = ConsoleApprovalContext::new(fixture.conversation_id, vec![approval_id]);

        let initial_turn_count: i64 = runtime
            .pool()
            .writer()
            .query_row("SELECT COUNT(*) FROM interaction_turns", [], |row| {
                row.get(0)
            })
            .expect("count seeded interaction turns");
        let mut state = InteractionState::default();
        state.select_agent(agent.id.clone(), agent.name);
        state.conversation_id = Some(fixture.conversation_id.to_string());
        type_prompt(&mut state, &format!("/approve {approval_id}"));
        let ConsoleCommandSubmission::Execute(request) = state
            .submit_command_with_approval_context(Some(&projection))
            .expect("submit authorized production command")
        else {
            panic!("authorized production command was rejected")
        };
        let mut controller = RunController::new(runtime.clone());
        assert!(controller.approval_authority_bound());
        controller
            .execute_command(request)
            .expect("execute authorized production command");
        state.apply(wait_for_control_terminal(&mut controller).await);
        let result = state.command_result.as_ref().expect("approval result");
        assert_eq!(result.status, ConsoleCommandStatus::Completed);
        assert_eq!(
            result
                .approval_resolution
                .as_ref()
                .map(|resolution| resolution.approval_id),
            Some(approval_id)
        );
        assert!(runtime
            .interactions()
            .list_pending_approvals(fixture.conversation_id)
            .await
            .expect("refresh production approval queue")
            .is_empty());
        let final_turn_count: i64 = runtime
            .pool()
            .writer()
            .query_row("SELECT COUNT(*) FROM interaction_turns", [], |row| {
                row.get(0)
            })
            .expect("count final interaction turns");
        assert_eq!(final_turn_count, initial_turn_count);
        controller.shutdown().await;
    }

    #[tokio::test(flavor = "current_thread")]
    #[allow(clippy::too_many_lines)]
    async fn console_commands_create_prompt_resume_after_restart_and_report_status() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let database_path = temp.path().join("tui-command-runtime.db");
        let config_path = temp.path().join("selected.toml");
        std::fs::write(
            &config_path,
            r#"
[[providers]]
id = "fake"
provider_type = "fake"
api_key_env = "POLKAGENT_TEST_FAKE_KEY_UNSET"
default_model = "default-model"

[[models]]
slug = "model-a"
provider = "fake"
"#,
        )
        .expect("write config");
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
        let mut second_spec = spec.clone();
        second_spec["name"] = serde_json::Value::String("second-command-agent".to_owned());
        let second_agent = store
            .create_agent("second-command-agent", None, &second_spec.to_string())
            .expect("create second active agent");
        let mut options =
            tui_runtime_options(&seed_pool, Some(&config_path)).expect("TUI runtime options");
        options.workdir = temp.path().to_path_buf();
        options.disable_harness = true;
        options.discover_environment_providers = false;
        drop(store);
        drop(seed_pool);

        let restart_options = options.clone();
        let runtime = Box::pin(RuntimeFactory::build(options))
            .await
            .expect("build runtime");
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

        type_prompt(&mut state, "/model model-a");
        let ConsoleCommandSubmission::Execute(request) =
            state.submit_command().expect("model command")
        else {
            panic!("model command rejected")
        };
        controller
            .execute_command(request)
            .expect("execute model command");
        let (_, events) = wait_for_controller_terminal(&mut controller).await;
        for event in events {
            state.apply(event);
        }
        assert_eq!(state.selected_model.as_deref(), Some("fake/model-a"));
        assert_eq!(
            state
                .command_result
                .as_ref()
                .map(|result| result.title.as_str()),
            Some("Updated durable Console model")
        );

        type_prompt(&mut state, "/agents");
        let ConsoleCommandSubmission::Execute(request) =
            state.submit_command().expect("agents command")
        else {
            panic!("agents command rejected")
        };
        controller
            .execute_command(request)
            .expect("execute agents command");
        let (_, events) = wait_for_controller_terminal(&mut controller).await;
        for event in events {
            state.apply(event);
        }
        let agents = state.command_result.as_ref().expect("agents result");
        assert!(agents
            .lines
            .iter()
            .any(|line| line.contains("* command-agent")));
        assert!(agents
            .lines
            .iter()
            .any(|line| line.contains("second-command-agent")));
        assert!(agents
            .lines
            .iter()
            .any(|line| line.contains("readiness: not projected")));

        let agent_specs_before = {
            let connection = runtime.pool().writer();
            connection
                .query_row(
                    "SELECT group_concat(spec_json, '\n') FROM (SELECT spec_json FROM agents ORDER BY id)",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .expect("snapshot TUI agent specs")
        };
        type_prompt(&mut state, "/agent second-command-agent");
        let ConsoleCommandSubmission::Execute(request) =
            state.submit_command().expect("agent command")
        else {
            panic!("agent command rejected")
        };
        controller
            .execute_command(request)
            .expect("execute agent command");
        let (_, events) = wait_for_controller_terminal(&mut controller).await;
        for event in events {
            state.apply(event);
        }
        assert_eq!(state.agent_id.as_deref(), Some(second_agent.id.as_str()));
        assert_eq!(state.agent_name.as_deref(), Some("second-command-agent"));
        assert_eq!(
            state.conversation_id.as_deref(),
            Some(conversation_id.as_str())
        );
        assert_eq!(state.selected_model.as_deref(), Some("fake/model-a"));
        assert!(state.transcript.is_empty());
        assert!(state.run.is_none());
        let (target_id, turn_count, run_count, event_count, agent_specs_after) = {
            let connection = runtime.pool().writer();
            let config_json: String = connection
                .query_row(
                    "SELECT config_json FROM interaction_sessions WHERE conversation_id = ?1",
                    [&conversation_id],
                    |row| row.get(0),
                )
                .expect("load TUI target config");
            let config: serde_json::Value =
                serde_json::from_str(&config_json).expect("decode TUI target config");
            let counts = connection
                .query_row(
                    "SELECT (SELECT COUNT(*) FROM interaction_turns),
                            (SELECT COUNT(*) FROM runs),
                            (SELECT COUNT(*) FROM interaction_events)",
                    [],
                    |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, i64>(2)?,
                        ))
                    },
                )
                .expect("count command-only TUI work");
            let specs = connection
                .query_row(
                    "SELECT group_concat(spec_json, '\n') FROM (SELECT spec_json FROM agents ORDER BY id)",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .expect("reload TUI agent specs");
            (
                config["target"]["id"].as_str().unwrap().to_owned(),
                counts.0,
                counts.1,
                counts.2,
                specs,
            )
        };
        assert_eq!(target_id, second_agent.id);
        assert_eq!((turn_count, run_count, event_count), (0, 0, 0));
        assert_eq!(agent_specs_after, agent_specs_before);

        type_prompt(&mut state, "prompt in explicitly selected session");
        let prompt = state.submit().expect("prompt after new command");
        assert_eq!(
            prompt.conversation_id.as_deref(),
            Some(conversation_id.as_str())
        );
        assert_eq!(prompt.agent_id, second_agent.id);
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

        let restarted = Box::pin(RuntimeFactory::build(restart_options))
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
        assert_eq!(resumed.agent_id.as_deref(), Some(second_agent.id.as_str()));
        assert_eq!(resumed.agent_name.as_deref(), Some("second-command-agent"));
        assert_eq!(resumed.selected_model.as_deref(), Some("fake/model-a"));
        assert_eq!(
            resumed.run.as_ref().map(|run| run.prompt.as_str()),
            Some("prompt in explicitly selected session")
        );
        let resumed_run_id = resumed
            .run
            .as_ref()
            .and_then(|run| run.run_id.clone())
            .expect("resumed durable run ID");

        type_prompt(&mut resumed, "/runs");
        let ConsoleCommandSubmission::Execute(request) =
            resumed.submit_command().expect("runs after restart")
        else {
            panic!("runs after restart rejected")
        };
        controller
            .execute_command(request)
            .expect("execute runs after restart");
        let (_, events) = wait_for_controller_terminal(&mut controller).await;
        for event in events {
            resumed.apply(event);
        }
        let runs = resumed
            .command_result
            .as_ref()
            .expect("restarted runs result");
        assert_eq!(runs.status, ConsoleCommandStatus::Completed);
        assert_eq!(runs.title, "Durable conversation runs");
        assert!(runs.lines.join("\n").contains(&resumed_run_id));

        type_prompt(&mut resumed, &format!("/inspect {resumed_run_id}"));
        let ConsoleCommandSubmission::Execute(request) = resumed
            .submit_command()
            .expect("inspect durable run after restart")
        else {
            panic!("inspect after restart rejected")
        };
        controller
            .execute_command(request)
            .expect("execute inspect after restart");
        let (_, events) = wait_for_controller_terminal(&mut controller).await;
        for event in events {
            resumed.apply(event);
        }
        let inspection = resumed
            .command_result
            .as_ref()
            .expect("restarted inspection result");
        assert_eq!(inspection.status, ConsoleCommandStatus::Completed);
        assert_eq!(inspection.title, "Durable run inspection");
        let inspection_lines = inspection.lines.join("\n");
        assert!(inspection_lines.contains(&resumed_run_id));
        assert!(inspection_lines.contains("State: completed"));

        type_prompt(&mut resumed, "/model missing-model");
        let ConsoleCommandSubmission::Execute(request) =
            resumed.submit_command().expect("unknown model command")
        else {
            panic!("unknown model command rejected before service validation")
        };
        controller
            .execute_command(request)
            .expect("execute unknown model command");
        let (_, events) = wait_for_controller_terminal(&mut controller).await;
        for event in events {
            resumed.apply(event);
        }
        let refusal = resumed
            .command_result
            .as_ref()
            .expect("typed model refusal");
        assert_eq!(refusal.status, ConsoleCommandStatus::Failed);
        assert!(refusal.lines[0].contains("invalid_request"));
        assert!(refusal.lines[0].contains("unknown model"));
        assert_eq!(resumed.selected_model.as_deref(), Some("fake/model-a"));

        for command in ["/model", "/status", "/help"] {
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
        assert!(help.lines.iter().any(|line| line.starts_with("/model")));
        assert!(help.lines.iter().any(|line| line.starts_with("/agents")));
        assert!(help.lines.iter().any(|line| line.starts_with("/agent ")));
        assert!(help.lines.iter().any(|line| line.starts_with("/runs")));
        assert!(help.lines.iter().any(|line| line.starts_with("/inspect")));
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
    async fn session_selector_switches_between_two_sessions_after_restart() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let database_path = temp.path().join("tui-session-selector.db");
        let config_path = temp.path().join("selected.toml");
        std::fs::write(&config_path, "").expect("write config");
        let seed_pool = SqlitePool::open(&database_path).expect("open database");
        migrations::migrate(&seed_pool.writer()).expect("migrate database");
        let store = SqliteRunStore::new(seed_pool.clone());
        let timestamp = "2026-01-01T00:00:00Z";
        let spec = |name: &str| {
            serde_json::json!({
                "name": name,
                "description": null,
                "model": "fake/default-model",
                "tools": [],
                "system_prompt": null,
                "autonomy_level": "supervised",
                "created_at": timestamp,
                "updated_at": timestamp,
            })
        };
        let agent = store
            .create_agent("selector-agent", None, &spec("selector-agent").to_string())
            .expect("create selector agent");
        let other_agent = store
            .create_agent("other-agent", None, &spec("other-agent").to_string())
            .expect("create other agent");
        let mut options =
            tui_runtime_options(&seed_pool, Some(&config_path)).expect("TUI runtime options");
        options.workdir = temp.path().to_path_buf();
        options.disable_harness = true;
        options.discover_environment_providers = false;
        drop(store);
        drop(seed_pool);

        let restart_options = options.clone();
        let runtime = Box::pin(RuntimeFactory::build(options))
            .await
            .expect("build runtime");
        let service = runtime.interactions();
        let first = service
            .new_interaction(CreateInteractionRequest {
                title: Some("First durable session".to_owned()),
                config: InteractionConfig::new(InteractionTarget::Agent(
                    agent.id.parse().expect("agent ID"),
                )),
                client_context: tui_client_context(&runtime).expect("client context"),
            })
            .await
            .expect("create first session");
        let second = service
            .new_interaction(CreateInteractionRequest {
                title: Some("Second durable session".to_owned()),
                config: InteractionConfig::new(InteractionTarget::Agent(
                    agent.id.parse().expect("agent ID"),
                )),
                client_context: tui_client_context(&runtime).expect("client context"),
            })
            .await
            .expect("create second session");
        service
            .new_interaction(CreateInteractionRequest {
                title: Some("Foreign session".to_owned()),
                config: InteractionConfig::new(InteractionTarget::Agent(
                    other_agent.id.parse().expect("other agent ID"),
                )),
                client_context: tui_client_context(&runtime).expect("client context"),
            })
            .await
            .expect("create foreign session");

        let mut seed_controller = RunController::new(runtime.clone());
        for (interaction, prompt) in [
            (&first, "first session prompt"),
            (&second, "second session prompt"),
        ] {
            seed_controller
                .start(PromptRequest {
                    agent_id: agent.id.clone(),
                    agent_name: agent.name.clone(),
                    conversation_id: Some(interaction.conversation_id.to_string()),
                    prompt: prompt.to_owned(),
                })
                .expect("start seed prompt");
            let _ = wait_for_controller_terminal(&mut seed_controller).await;
        }
        drop(seed_controller);
        drop(runtime);

        let restarted = Box::pin(RuntimeFactory::build(restart_options))
            .await
            .expect("restart runtime");
        let mut controller = RunController::new(restarted.clone());
        let mut state = InteractionState::default();
        state.select_agent(agent.id.clone(), agent.name.clone());
        let list_request = state.begin_session_picker().expect("open session selector");
        controller
            .list_sessions(list_request)
            .expect("list sessions asynchronously");
        let (_, events) = wait_for_controller_terminal(&mut controller).await;
        for event in events {
            state.apply(event);
        }
        let picker = state.session_picker.as_ref().expect("ready selector");
        assert_eq!(picker.status, SessionPickerStatus::Ready);
        assert_eq!(
            picker.sessions.len(),
            2,
            "foreign agent session is excluded"
        );
        assert!(picker
            .sessions
            .iter()
            .all(|session| session.title != "Foreign session"));
        assert!(picker
            .sessions
            .iter()
            .all(|session| { session.state == "active" && !session.updated_at.is_empty() }));
        let first_id = first.conversation_id.to_string();
        let first_index = picker
            .sessions
            .iter()
            .position(|session| session.conversation_id == first_id)
            .expect("first session in selector");
        for _ in 0..first_index {
            state.session_picker_down();
        }

        let load_request = state
            .begin_session_selection()
            .expect("select first session");
        assert_eq!(load_request.conversation, first_id);
        controller
            .load_session(load_request)
            .expect("load selected transcript");
        let (_, events) = wait_for_controller_terminal(&mut controller).await;
        for event in events {
            state.apply(event);
        }
        assert!(state.session_picker.is_none());
        assert_eq!(state.conversation_id.as_deref(), Some(first_id.as_str()));
        assert_eq!(
            state.run.as_ref().map(|run| run.prompt.as_str()),
            Some("first session prompt")
        );

        type_prompt(&mut state, "follow up in selected first session");
        let prompt = state.submit().expect("submit selected-session prompt");
        assert_eq!(prompt.conversation_id.as_deref(), Some(first_id.as_str()));
        controller
            .start(prompt)
            .expect("start selected-session prompt");
        let (_, events) = wait_for_controller_terminal(&mut controller).await;
        for event in events {
            state.apply(event);
        }

        type_prompt(&mut state, "/status");
        let ConsoleCommandSubmission::Execute(status) =
            state.submit_command().expect("status command")
        else {
            panic!("status command rejected")
        };
        controller
            .execute_command(status)
            .expect("execute status command");
        let (_, events) = wait_for_controller_terminal(&mut controller).await;
        for event in events {
            state.apply(event);
        }
        let first_turns = restarted
            .interactions()
            .list_turns(first.conversation_id)
            .await
            .expect("list first-session turns");
        let second_turns = restarted
            .interactions()
            .list_turns(second.conversation_id)
            .await
            .expect("list second-session turns");
        assert_eq!(first_turns.len(), 2);
        assert_eq!(second_turns.len(), 1);
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
        let runtime = Box::pin(RuntimeFactory::build(options))
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
            "failed:recovered after restart"
        );

        drop(durable_store);
        drop(controller);
        drop(runtime);

        let restarted = Box::pin(RuntimeFactory::build(restart_options))
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
