//! Agent Client Protocol server surface for editor integrations.
//!
//! This crate owns ACP wire semantics and session-local editor state. The
//! application runtime is injected through [`AcpBackend`], which keeps the
//! transport independent of Polkagent's CLI and storage adapters.

#![warn(clippy::pedantic)]

use std::collections::HashMap;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use agent_client_protocol::schema::v1::{
    AgentCapabilities, AvailableCommand, AvailableCommandInput, AvailableCommandsUpdate,
    CancelNotification, ContentBlock, ContentChunk, Implementation, InitializeRequest,
    InitializeResponse, LoadSessionRequest, LoadSessionResponse, NewSessionRequest,
    NewSessionResponse, PermissionOption, PermissionOptionKind, PromptRequest, PromptResponse,
    RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse, ResourceLink,
    ResumeSessionRequest, ResumeSessionResponse, SessionCapabilities, SessionConfigOption,
    SessionConfigOptionCategory, SessionConfigSelectOption, SessionId, SessionNotification,
    SessionResumeCapabilities, SessionUpdate, SetSessionConfigOptionRequest,
    SetSessionConfigOptionResponse, StopReason, TextContent, ToolCall as AcpToolCall,
    ToolCallContent, ToolCallStatus as AcpToolCallStatus, ToolCallUpdate as AcpToolCallUpdate,
    ToolCallUpdateFields, ToolKind as AcpToolKind, UnstructuredCommandInput, UsageUpdate,
};
use agent_client_protocol::{Agent, Stdio};
use async_trait::async_trait;
use futures::FutureExt as _;
use polkagent_core::ConversationId;
use polkagent_interaction::{
    format_run_inspection, format_run_list, validate_working_directory, CancelTarget,
    CommandContext, CommandName, CommandRegistry, CommandSpec, InteractionCommand, ParsedLine,
    RunDetailView, RunSummaryView, ToolCallKind, ToolCallStatus, ToolCallView,
};
use tokio::sync::Mutex;

const BACKEND_PANIC_DETAIL: &str = "Polkagent's ACP backend panicked; the request was stopped";
const AGENT_CONFIG_ID: &str = "polkagent.agent";
const MODEL_CONFIG_ID: &str = "model";
const INHERIT_MODEL_VALUE: &str = "_polkagent_agent_model";
const TURN_ID_META_KEY: &str = "polkagent.turnId";
const TURN_RESULT_META_KEY: &str = "polkagent";
const PROMPT_UPDATE_BUFFER: usize = 32;
const ALLOW_ONCE_OPTION_ID: &str = "polkagent.allow_once";
const REJECT_ONCE_OPTION_ID: &str = "polkagent.reject_once";
const ACP_COMMANDS: [CommandName; 8] = [
    CommandName::Help,
    CommandName::Status,
    CommandName::Agents,
    CommandName::Agent,
    CommandName::Runs,
    CommandName::Inspect,
    CommandName::Model,
    CommandName::Cancel,
];

/// A backend failure that is safe to return through an ACP error response.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct BackendError {
    message: String,
    kind: BackendErrorKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BackendErrorKind {
    InvalidRequest,
    NotFound,
    Conflict,
    Unsupported,
    Internal,
}

impl BackendError {
    /// Construct a backend error with a user-facing message.
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind: BackendErrorKind::Internal,
        }
    }

    /// Construct a caller-input failure.
    #[must_use]
    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind: BackendErrorKind::InvalidRequest,
        }
    }

    /// Construct a durable identity lookup failure.
    #[must_use]
    pub fn not_found(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind: BackendErrorKind::NotFound,
        }
    }

    /// Construct a durable state-conflict failure.
    #[must_use]
    pub fn conflict(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind: BackendErrorKind::Conflict,
        }
    }

    /// Construct a truthful unsupported-capability failure.
    #[must_use]
    pub fn unsupported(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind: BackendErrorKind::Unsupported,
        }
    }
}

/// A Polkagent agent exposed for editor-side selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSummary {
    /// Stable agent identifier.
    pub id: String,
    /// Human-readable agent name.
    pub name: String,
    /// Current lifecycle state.
    pub state: String,
}

/// A configured model exposed for editor-side session selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelSummary {
    /// Model identifier passed to the next real inference request.
    pub id: String,
    /// Human-readable model label.
    pub name: String,
}

/// Durable configuration and transcript returned when an ACP session is
/// created or attached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendSession {
    /// Exact durable conversation identity, also used as the ACP session ID.
    pub session_id: String,
    /// Resolved active agent identity.
    pub selected_agent: String,
    /// Persisted model override; `None` inherits the agent default.
    pub selected_model: Option<String>,
    /// Durable transcript returned only by full `session/load`.
    pub transcript: Vec<BackendTranscriptTurn>,
}

/// One exact durable user/assistant pair replayed during `session/load`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendTranscriptTurn {
    /// Exact persisted user text.
    pub user_text: String,
    /// Exact persisted assistant text when the turn reached a durable terminal projection.
    pub assistant_text: Option<String>,
}

/// A progressive update emitted while a backend prompt is still running.
#[derive(Debug, Clone, PartialEq)]
pub enum BackendPromptUpdate {
    /// Text appended to the final assistant response.
    TextDelta(String),
    /// Real cumulative run-token usage with a known model context window.
    Usage {
        /// Input plus output tokens reported by the runtime terminal event.
        used: u64,
        /// Configured context-window size for the effective model.
        size: u64,
    },
    /// First complete projection of one durable tool call.
    ToolCallStarted(ToolCallView),
    /// Replacement state for the same durable tool call identity.
    ToolCallUpdated(ToolCallView),
}

/// A once-only decision returned by an ACP permission surface.
///
/// Polkagent deliberately does not expose ACP's remembered allow/reject
/// options until durable mandate scope, expiry, revocation, and audit exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionDecision {
    /// Permit only the exact effect represented by the request.
    AllowOnce,
    /// Reject only the exact effect represented by the request.
    RejectOnce,
    /// The prompt turn was cancelled before a decision was made.
    Cancelled,
}

/// Build the exact ACP permission request for a durable tool projection.
///
/// Only once-only choices are offered. Raw arguments and output remain absent
/// because the shared interaction projection exposes safety-reviewed summaries
/// rather than unclassified effect payloads.
#[must_use]
pub fn permission_request(
    session_id: impl Into<SessionId>,
    call: ToolCallView,
) -> RequestPermissionRequest {
    RequestPermissionRequest::new(
        session_id,
        acp_tool_update(call),
        vec![
            PermissionOption::new(
                ALLOW_ONCE_OPTION_ID,
                "Allow once",
                PermissionOptionKind::AllowOnce,
            ),
            PermissionOption::new(
                REJECT_ONCE_OPTION_ID,
                "Reject once",
                PermissionOptionKind::RejectOnce,
            ),
        ],
    )
}

/// Decode an ACP permission response without accepting unadvertised choices.
///
/// # Errors
///
/// Returns an invalid-parameters protocol error when a client selects an
/// unknown or remembered option. Such a response never grants authority.
pub fn permission_decision(
    response: &RequestPermissionResponse,
) -> Result<PermissionDecision, agent_client_protocol::Error> {
    match &response.outcome {
        RequestPermissionOutcome::Cancelled => Ok(PermissionDecision::Cancelled),
        RequestPermissionOutcome::Selected(selected)
            if selected.option_id.0.as_ref() == ALLOW_ONCE_OPTION_ID =>
        {
            Ok(PermissionDecision::AllowOnce)
        }
        RequestPermissionOutcome::Selected(selected)
            if selected.option_id.0.as_ref() == REJECT_ONCE_OPTION_ID =>
        {
            Ok(PermissionDecision::RejectOnce)
        }
        RequestPermissionOutcome::Selected(_) => {
            Err(agent_client_protocol::Error::invalid_params()
                .data("ACP client selected an unadvertised permission option"))
        }
        _ => Err(agent_client_protocol::Error::invalid_params()
            .data("ACP client returned an unsupported permission outcome")),
    }
}

/// The result of one backend prompt turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendTurn {
    /// Exact final text, including any prefix already sent as progressive updates.
    pub text: String,
    /// Whether cancellation ended the turn.
    pub cancelled: bool,
    /// Exact durable interaction turn identity.
    pub turn_id: String,
    /// Exact durable run identities linked to the turn.
    pub run_ids: Vec<String>,
    /// Last durable interaction event sequence consumed for the turn.
    pub checkpoint: u64,
}

impl BackendTurn {
    /// Construct a successful turn.
    #[must_use]
    pub fn completed(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            cancelled: false,
            turn_id: String::new(),
            run_ids: Vec::new(),
            checkpoint: 0,
        }
    }

    /// Construct a cancelled turn.
    #[must_use]
    pub fn cancelled(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            cancelled: true,
            turn_id: String::new(),
            run_ids: Vec::new(),
            checkpoint: 0,
        }
    }

    /// Attach exact durable correlation identities to a runtime-backed turn.
    #[must_use]
    pub fn durable(
        mut self,
        turn_id: impl Into<String>,
        run_ids: Vec<String>,
        checkpoint: u64,
    ) -> Self {
        self.turn_id = turn_id.into();
        self.run_ids = run_ids;
        self.checkpoint = checkpoint;
        self
    }
}

/// Runtime operations required by the ACP surface.
#[async_trait]
pub trait AcpBackend: Send + Sync + 'static {
    /// List agents that an editor session may select.
    async fn list_agents(&self) -> Result<Vec<AgentSummary>, BackendError>;

    /// List configured models that an editor session may select.
    async fn list_models(&self) -> Result<Vec<ModelSummary>, BackendError>;

    /// Create exactly one durable interaction for a new ACP session.
    async fn new_session(
        &self,
        cwd: &Path,
        agent: Option<&str>,
        model: Option<&str>,
    ) -> Result<BackendSession, BackendError>;

    /// Attach an existing durable interaction and optionally load its transcript.
    async fn load_session(
        &self,
        session_id: &str,
        cwd: &Path,
        include_transcript: bool,
    ) -> Result<BackendSession, BackendError>;

    /// Persist a new single-agent target for an ACP interaction.
    async fn set_agent(
        &self,
        session_id: &str,
        agent_id: &str,
    ) -> Result<BackendSession, BackendError>;

    /// Persist or clear the model override for an ACP interaction.
    async fn set_model(
        &self,
        session_id: &str,
        model: Option<&str>,
    ) -> Result<BackendSession, BackendError>;

    /// List bounded, safe run projections for one durable ACP session.
    async fn list_runs(&self, _session_id: &str) -> Result<Vec<RunSummaryView>, BackendError> {
        Err(BackendError::unsupported(
            "run listing is unavailable from this backend",
        ))
    }

    /// Inspect one run after proving it belongs to the durable ACP session.
    async fn inspect_run(
        &self,
        _session_id: &str,
        _run_id: &str,
    ) -> Result<RunDetailView, BackendError> {
        Err(BackendError::unsupported(
            "run inspection is unavailable from this backend",
        ))
    }

    /// Execute one prompt through Polkagent's orchestration runtime.
    ///
    /// Implementations send progressive updates through the bounded channel.
    /// The returned text must equal the concatenated text deltas plus any
    /// unstreamed terminal suffix.
    async fn prompt(
        &self,
        session_id: &str,
        cwd: &Path,
        turn_id: &str,
        prompt: &str,
        updates: tokio::sync::mpsc::Sender<BackendPromptUpdate>,
    ) -> Result<BackendTurn, BackendError>;

    /// Cancel the active run, if any, for an ACP session.
    async fn cancel(&self, session_id: &str) -> Result<(), BackendError>;
}

#[derive(Debug)]
enum BackendCallError {
    Rejected(BackendError),
    Panicked,
}

async fn call_backend<T, F>(future: F) -> Result<T, BackendCallError>
where
    F: Future<Output = Result<T, BackendError>>,
{
    match AssertUnwindSafe(future).catch_unwind().await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err(BackendCallError::Rejected(error)),
        Err(_) => Err(BackendCallError::Panicked),
    }
}

/// Configuration for one ACP stdio server process.
#[derive(Debug, Clone, Default)]
pub struct ServerConfig {
    /// Agent name or ID selected for newly-created sessions.
    pub default_agent: Option<String>,
    /// Model override selected for newly-created sessions.
    pub default_model: Option<String>,
}

#[derive(Debug, Clone)]
struct EditorSession {
    cwd: PathBuf,
    selected_agent: Option<String>,
    selected_model: Option<String>,
    busy: bool,
}

type Sessions = Arc<Mutex<HashMap<SessionId, EditorSession>>>;

/// Run the ACP v1 server over stdin/stdout until the client disconnects.
///
/// Stdout is owned exclusively by the official ACP transport. Callers must
/// configure any diagnostics to use stderr or a file before invoking this.
///
/// # Errors
///
/// Returns an ACP transport error when framing, dispatch, or stdio I/O fails.
#[allow(
    clippy::too_many_lines,
    reason = "keeping every stable ACP request handler in one builder makes advertised capabilities auditable against registered methods"
)]
pub async fn serve_stdio(
    backend: Arc<dyn AcpBackend>,
    config: ServerConfig,
) -> Result<(), agent_client_protocol::Error> {
    let sessions: Sessions = Arc::new(Mutex::new(HashMap::new()));
    let new_sessions = Arc::clone(&sessions);
    let load_sessions = Arc::clone(&sessions);
    let resume_sessions = Arc::clone(&sessions);
    let prompt_sessions = Arc::clone(&sessions);
    let cancel_sessions = Arc::clone(&sessions);
    let config_sessions = Arc::clone(&sessions);
    let new_backend = Arc::clone(&backend);
    let load_backend = Arc::clone(&backend);
    let resume_backend = Arc::clone(&backend);
    let prompt_backend = Arc::clone(&backend);
    let cancel_backend = Arc::clone(&backend);
    let config_backend = Arc::clone(&backend);
    let default_agent = config.default_agent;
    let default_model = config.default_model;

    Agent
        .builder()
        .name("polkagent-acp")
        .on_receive_request(
            async move |request: InitializeRequest, responder, _connection| {
                let response = InitializeResponse::new(request.protocol_version)
                    .agent_capabilities(
                        AgentCapabilities::new()
                            .load_session(true)
                            .session_capabilities(
                                SessionCapabilities::new().resume(SessionResumeCapabilities::new()),
                            ),
                    )
                    .agent_info(
                        Implementation::new("polkagent", env!("CARGO_PKG_VERSION"))
                            .title("Polkagent"),
                    );
                responder.respond(response)
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: LoadSessionRequest, responder, connection| {
                let (session_id, session, transcript) = match attach_editor_session(
                    request.session_id,
                    request.cwd,
                    request.mcp_servers.is_empty(),
                    request.additional_directories.is_empty(),
                    &load_sessions,
                    load_backend.as_ref(),
                    true,
                )
                .await
                {
                    Ok(loaded) => loaded,
                    Err(error) => return responder.respond_with_error(error),
                };
                replay_transcript(&connection, &session_id, &transcript)?;
                let (agents, models) = discover_options(load_backend.as_ref()).await?;
                responder.respond(
                    LoadSessionResponse::new()
                        .config_options(session_config_options(&session, &agents, &models)),
                )?;
                send_available_commands(&connection, session_id, false)
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: ResumeSessionRequest, responder, connection| {
                let (session_id, session, _) = match attach_editor_session(
                    request.session_id,
                    request.cwd,
                    request.mcp_servers.is_empty(),
                    request.additional_directories.is_empty(),
                    &resume_sessions,
                    resume_backend.as_ref(),
                    false,
                )
                .await
                {
                    Ok(resumed) => resumed,
                    Err(error) => return responder.respond_with_error(error),
                };
                let (agents, models) = discover_options(resume_backend.as_ref()).await?;
                responder.respond(
                    ResumeSessionResponse::new()
                        .config_options(session_config_options(&session, &agents, &models)),
                )?;
                send_available_commands(&connection, session_id, false)
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: NewSessionRequest, responder, connection| {
                let (session_id, response) = match create_editor_session(
                    request,
                    &new_sessions,
                    new_backend.as_ref(),
                    default_agent.as_deref(),
                    default_model.as_deref(),
                )
                .await
                {
                    Ok(created) => created,
                    Err(error) => return responder.respond_with_error(error),
                };
                responder.respond(response)?;
                connection.send_notification(SessionNotification::new(
                    session_id,
                    SessionUpdate::AvailableCommandsUpdate(AvailableCommandsUpdate::new(
                        available_commands(false),
                    )),
                ))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: SetSessionConfigOptionRequest, responder, _connection| {
                match set_session_config_option(request, &config_sessions, config_backend.as_ref())
                    .await
                {
                    Ok(options) => responder.respond(SetSessionConfigOptionResponse::new(options)),
                    Err(error) => responder.respond_with_error(error),
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: PromptRequest, responder, connection| {
                let sessions = Arc::clone(&prompt_sessions);
                let backend = Arc::clone(&prompt_backend);
                tokio::spawn(async move {
                    let result = handle_prompt(request, sessions, backend, &connection).await;
                    match result {
                        Ok(response) => {
                            let _ = responder.respond(response);
                        }
                        Err(error) => {
                            let _ = responder.respond_with_error(error);
                        }
                    }
                });
                Ok(())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_notification(
            async move |notification: CancelNotification, _connection| {
                if cancel_sessions
                    .lock()
                    .await
                    .contains_key(&notification.session_id)
                {
                    call_backend(cancel_backend.cancel(notification.session_id.0.as_ref()))
                        .await
                        .map_err(|error| backend_protocol_error(&error))?;
                }
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .connect_to(Stdio::new())
        .await
}

async fn create_editor_session(
    request: NewSessionRequest,
    sessions: &Sessions,
    backend: &dyn AcpBackend,
    default_agent: Option<&str>,
    default_model: Option<&str>,
) -> Result<(SessionId, NewSessionResponse), agent_client_protocol::Error> {
    validate_session_cwd(&request.cwd)?;
    if !request.mcp_servers.is_empty() {
        return Err(agent_client_protocol::Error::invalid_params()
            .data("session MCP servers are not supported by this Polkagent ACP slice yet"));
    }
    if request
        .additional_directories
        .iter()
        .any(|directory| !directory.is_absolute())
    {
        return Err(agent_client_protocol::Error::invalid_params()
            .data("all additional session directories must be absolute paths"));
    }
    if !request.additional_directories.is_empty() {
        return Err(agent_client_protocol::Error::invalid_params().data(
            "additional session directories are not supported by this Polkagent ACP slice yet",
        ));
    }

    let agents = call_backend(backend.list_agents())
        .await
        .map_err(|error| backend_protocol_error(&error))?;
    let selected_agent = match default_agent {
        Some(selector) => Some(
            find_agent_in(&agents, selector)
                .ok_or_else(|| {
                    agent_client_protocol::Error::invalid_params().data(format!(
                        "configured ACP agent '{selector}' was not found or is not active"
                    ))
                })?
                .id
                .clone(),
        ),
        None => None,
    };
    let models = call_backend(backend.list_models())
        .await
        .map_err(|error| backend_protocol_error(&error))?;
    if let Some(model) = default_model {
        if !models.iter().any(|candidate| candidate.id == model) {
            return Err(agent_client_protocol::Error::invalid_params()
                .data(format!("configured ACP model '{model}' is not available")));
        }
    }

    let durable =
        call_backend(backend.new_session(&request.cwd, selected_agent.as_deref(), default_model))
            .await
            .map_err(|error| backend_protocol_error(&error))?;
    let session_id = SessionId::new(durable.session_id);
    let session = EditorSession {
        cwd: request.cwd,
        selected_agent: Some(durable.selected_agent),
        selected_model: durable.selected_model,
        busy: false,
    };
    let response = NewSessionResponse::new(session_id.clone())
        .config_options(session_config_options(&session, &agents, &models));
    sessions.lock().await.insert(session_id.clone(), session);
    Ok((session_id, response))
}

async fn attach_editor_session(
    session_id: SessionId,
    cwd: PathBuf,
    no_mcp_servers: bool,
    no_additional_directories: bool,
    sessions: &Sessions,
    backend: &dyn AcpBackend,
    include_transcript: bool,
) -> Result<(SessionId, EditorSession, Vec<BackendTranscriptTurn>), agent_client_protocol::Error> {
    validate_session_roots(&cwd, no_mcp_servers, no_additional_directories)?;
    let durable =
        call_backend(backend.load_session(session_id.0.as_ref(), &cwd, include_transcript))
            .await
            .map_err(|error| backend_protocol_error(&error))?;
    if durable.session_id != session_id.0.as_ref() {
        return Err(agent_client_protocol::Error::internal_error()
            .data("durable ACP session identity changed while loading"));
    }
    let session = EditorSession {
        cwd,
        selected_agent: Some(durable.selected_agent),
        selected_model: durable.selected_model,
        busy: false,
    };
    sessions
        .lock()
        .await
        .insert(session_id.clone(), session.clone());
    Ok((session_id, session, durable.transcript))
}

fn validate_session_roots(
    cwd: &Path,
    no_mcp_servers: bool,
    no_additional_directories: bool,
) -> Result<(), agent_client_protocol::Error> {
    validate_session_cwd(cwd)?;
    if !no_mcp_servers {
        return Err(agent_client_protocol::Error::invalid_params()
            .data("session MCP servers are not supported by Polkagent ACP"));
    }
    if !no_additional_directories {
        return Err(agent_client_protocol::Error::invalid_params()
            .data("additional session directories are not supported by Polkagent ACP"));
    }
    Ok(())
}

fn validate_session_cwd(cwd: &Path) -> Result<(), agent_client_protocol::Error> {
    validate_working_directory(cwd).map_err(|_| {
        agent_client_protocol::Error::invalid_params()
            .data("session cwd must be an absolute, lexically normalized UTF-8 path")
    })
}

async fn discover_options(
    backend: &dyn AcpBackend,
) -> Result<(Vec<AgentSummary>, Vec<ModelSummary>), agent_client_protocol::Error> {
    let agents = call_backend(backend.list_agents())
        .await
        .map_err(|error| backend_protocol_error(&error))?;
    let models = call_backend(backend.list_models())
        .await
        .map_err(|error| backend_protocol_error(&error))?;
    Ok((agents, models))
}

fn replay_transcript(
    connection: &agent_client_protocol::ConnectionTo<agent_client_protocol::Client>,
    session_id: &SessionId,
    transcript: &[BackendTranscriptTurn],
) -> Result<(), agent_client_protocol::Error> {
    for turn in transcript {
        connection.send_notification(SessionNotification::new(
            session_id.clone(),
            SessionUpdate::UserMessageChunk(ContentChunk::new(ContentBlock::Text(
                TextContent::new(turn.user_text.clone()),
            ))),
        ))?;
        if let Some(text) = &turn.assistant_text {
            connection.send_notification(SessionNotification::new(
                session_id.clone(),
                SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
                    TextContent::new(text.clone()),
                ))),
            ))?;
        }
    }
    Ok(())
}

fn send_available_commands(
    connection: &agent_client_protocol::ConnectionTo<agent_client_protocol::Client>,
    session_id: SessionId,
    has_active_prompt: bool,
) -> Result<(), agent_client_protocol::Error> {
    connection.send_notification(SessionNotification::new(
        session_id,
        SessionUpdate::AvailableCommandsUpdate(AvailableCommandsUpdate::new(available_commands(
            has_active_prompt,
        ))),
    ))
}

async fn set_session_config_option(
    request: SetSessionConfigOptionRequest,
    sessions: &Sessions,
    backend: &dyn AcpBackend,
) -> Result<Vec<SessionConfigOption>, agent_client_protocol::Error> {
    let snapshot = sessions
        .lock()
        .await
        .get(&request.session_id)
        .cloned()
        .ok_or_else(|| {
            agent_client_protocol::Error::invalid_params()
                .data(format!("unknown session: {}", request.session_id))
        })?;
    if snapshot.busy {
        return Err(agent_client_protocol::Error::invalid_request()
            .data("session configuration cannot change during an active prompt"));
    }
    let value = request.value.as_value_id().ok_or_else(|| {
        agent_client_protocol::Error::invalid_params()
            .data("agent and model configuration values must be select-option IDs")
    })?;
    let value = value.0.as_ref();
    let agents = call_backend(backend.list_agents())
        .await
        .map_err(|error| backend_protocol_error(&error))?;
    let models = call_backend(backend.list_models())
        .await
        .map_err(|error| backend_protocol_error(&error))?;

    let mut all_sessions = sessions.lock().await;
    let session = all_sessions.get_mut(&request.session_id).ok_or_else(|| {
        agent_client_protocol::Error::invalid_params()
            .data(format!("unknown session: {}", request.session_id))
    })?;
    if session.busy {
        return Err(agent_client_protocol::Error::invalid_request()
            .data("session configuration cannot change during an active prompt"));
    }
    match request.config_id.0.as_ref() {
        AGENT_CONFIG_ID => {
            let Some(agent) = agents.iter().find(|agent| agent.id == value) else {
                return Err(agent_config_error(value));
            };
            let durable = call_backend(backend.set_agent(request.session_id.0.as_ref(), &agent.id))
                .await
                .map_err(|error| backend_protocol_error(&error))?;
            session.selected_agent = Some(durable.selected_agent);
            session.selected_model = durable.selected_model;
        }
        MODEL_CONFIG_ID if value == INHERIT_MODEL_VALUE => {
            let durable = call_backend(backend.set_model(request.session_id.0.as_ref(), None))
                .await
                .map_err(|error| backend_protocol_error(&error))?;
            session.selected_agent = Some(durable.selected_agent);
            session.selected_model = durable.selected_model;
        }
        MODEL_CONFIG_ID => {
            let Some(model) = models.iter().find(|model| model.id == value) else {
                return Err(model_config_error(value));
            };
            let durable =
                call_backend(backend.set_model(request.session_id.0.as_ref(), Some(&model.id)))
                    .await
                    .map_err(|error| backend_protocol_error(&error))?;
            session.selected_agent = Some(durable.selected_agent);
            session.selected_model = durable.selected_model;
        }
        unsupported => {
            return Err(agent_client_protocol::Error::invalid_params().data(format!(
                "unsupported session config option '{unsupported}'; supported options are '{AGENT_CONFIG_ID}' and '{MODEL_CONFIG_ID}'"
            )));
        }
    }
    Ok(session_config_options(session, &agents, &models))
}

fn session_config_options(
    session: &EditorSession,
    agents: &[AgentSummary],
    models: &[ModelSummary],
) -> Vec<SessionConfigOption> {
    let agent_options = agents
        .iter()
        .map(|agent| SessionConfigSelectOption::new(agent.id.clone(), agent.name.clone()))
        .collect::<Vec<_>>();
    let agent = SessionConfigOption::select(
        AGENT_CONFIG_ID,
        "Polkagent agent",
        session.selected_agent.clone().unwrap_or_default(),
        agent_options,
    )
    .description("Active agent used for the next editor prompt".to_owned())
    .category(SessionConfigOptionCategory::Other(
        "_polkagent_agent".to_owned(),
    ));

    let mut model_options = vec![SessionConfigSelectOption::new(
        INHERIT_MODEL_VALUE,
        "Use selected agent model",
    )];
    model_options.extend(
        models
            .iter()
            .map(|model| SessionConfigSelectOption::new(model.id.clone(), model.name.clone())),
    );
    let model = SessionConfigOption::select(
        MODEL_CONFIG_ID,
        "Model",
        session
            .selected_model
            .clone()
            .unwrap_or_else(|| INHERIT_MODEL_VALUE.to_owned()),
        model_options,
    )
    .description("Session-scoped model override for the next prompt".to_owned())
    .category(SessionConfigOptionCategory::Model);
    vec![agent, model]
}

fn agent_config_error(value: &str) -> agent_client_protocol::Error {
    agent_client_protocol::Error::invalid_params().data(format!(
        "active agent config value '{value}' is unavailable; refresh the advertised session options"
    ))
}

fn model_config_error(value: &str) -> agent_client_protocol::Error {
    agent_client_protocol::Error::invalid_params().data(format!(
        "model config value '{value}' is unavailable; choose an advertised configured model"
    ))
}

async fn handle_prompt(
    request: PromptRequest,
    sessions: Sessions,
    backend: Arc<dyn AcpBackend>,
    connection: &agent_client_protocol::ConnectionTo<agent_client_protocol::Client>,
) -> Result<PromptResponse, agent_client_protocol::Error> {
    let turn_id = prompt_turn_id(&request)?;
    let prompt = prompt_text(&request.prompt)?;
    let session_id = request.session_id.clone();
    let snapshot = sessions
        .lock()
        .await
        .get(&session_id)
        .cloned()
        .ok_or_else(|| {
            agent_client_protocol::Error::invalid_params()
                .data(format!("unknown session: {session_id}"))
        })?;

    let registry = CommandRegistry::mvp();
    let (outcome, text_already_forwarded) = match registry.parse(&prompt) {
        Ok(ParsedLine::Command(invocation)) => (
            handle_slash_command(
                invocation.command,
                &session_id,
                &snapshot,
                &sessions,
                backend.as_ref(),
                &registry,
            )
            .await?,
            false,
        ),
        Ok(ParsedLine::Prompt(prompt)) => (
            handle_regular_prompt(
                &prompt,
                &turn_id,
                &session_id,
                &sessions,
                backend.as_ref(),
                connection,
            )
            .await?,
            true,
        ),
        Err(error) => (
            BackendTurn::completed(format!(
                "Command error: {error}\n\n{}",
                help_text(&registry, None, snapshot.busy)
            )),
            false,
        ),
    };

    if !text_already_forwarded && !outcome.text.is_empty() {
        send_text_update(connection, session_id.clone(), outcome.text)?;
    }

    let stop_reason = if outcome.cancelled {
        StopReason::Cancelled
    } else {
        StopReason::EndTurn
    };
    let mut response = PromptResponse::new(stop_reason);
    if !outcome.turn_id.is_empty() {
        let mut meta = serde_json::Map::new();
        meta.insert(
            TURN_RESULT_META_KEY.to_owned(),
            serde_json::json!({
                "conversationId": session_id.0.as_ref(),
                "turnId": outcome.turn_id,
                "runIds": outcome.run_ids,
                "checkpoint": outcome.checkpoint,
            }),
        );
        response = response.meta(meta);
    }
    Ok(response)
}

fn prompt_turn_id(request: &PromptRequest) -> Result<String, agent_client_protocol::Error> {
    let Some(value) = request
        .meta
        .as_ref()
        .and_then(|meta| meta.get(TURN_ID_META_KEY))
    else {
        return Ok(uuid::Uuid::now_v7().to_string());
    };
    let value = value.as_str().ok_or_else(|| {
        agent_client_protocol::Error::invalid_params()
            .data(format!("'{TURN_ID_META_KEY}' must be a UUID string"))
    })?;
    uuid::Uuid::parse_str(value).map_err(|_| {
        agent_client_protocol::Error::invalid_params()
            .data(format!("'{TURN_ID_META_KEY}' must be a valid UUID"))
    })?;
    Ok(value.to_owned())
}

fn prompt_text(blocks: &[ContentBlock]) -> Result<String, agent_client_protocol::Error> {
    let mut parts = Vec::new();
    for block in blocks {
        match block {
            ContentBlock::Text(text) => parts.push(text.text.clone()),
            ContentBlock::ResourceLink(ResourceLink { name, uri, .. }) => {
                parts.push(format!("[context: {name} ({uri})]"));
            }
            _ => {
                return Err(agent_client_protocol::Error::invalid_params().data(
                    "this ACP surface currently accepts text and resource-link prompt blocks",
                ));
            }
        }
    }
    let prompt = parts.join("\n");
    if prompt.trim().is_empty() {
        return Err(agent_client_protocol::Error::invalid_params().data("prompt cannot be empty"));
    }
    Ok(prompt)
}

async fn handle_regular_prompt(
    prompt: &str,
    turn_id: &str,
    session_id: &SessionId,
    sessions: &Sessions,
    backend: &dyn AcpBackend,
    connection: &agent_client_protocol::ConnectionTo<agent_client_protocol::Client>,
) -> Result<BackendTurn, agent_client_protocol::Error> {
    let cwd = {
        let mut all_sessions = sessions.lock().await;
        let current = all_sessions.get_mut(session_id).ok_or_else(|| {
            agent_client_protocol::Error::invalid_params()
                .data(format!("unknown session: {session_id}"))
        })?;
        if current.selected_agent.is_none() {
            return Ok(BackendTurn::completed(
                "No agent is selected. Use /agents to list active agents, then /agent <name-or-id> to select one.",
            ));
        }
        if current.busy {
            return Err(agent_client_protocol::Error::invalid_request()
                .data(format!("session {session_id} already has an active prompt")));
        }
        current.busy = true;
        current.cwd.clone()
    };
    if let Err(error) = send_available_commands(connection, session_id.clone(), true) {
        if let Some(current) = sessions.lock().await.get_mut(session_id) {
            current.busy = false;
        }
        return Err(error);
    }
    let (updates_tx, mut updates_rx) = tokio::sync::mpsc::channel(PROMPT_UPDATE_BUFFER);
    let backend_call = call_backend(backend.prompt(
        session_id.0.as_ref(),
        &cwd,
        turn_id,
        prompt.trim(),
        updates_tx,
    ));
    tokio::pin!(backend_call);
    let mut streamed_text = String::new();
    let mut forwarding_error = None;
    let backend_result = loop {
        tokio::select! {
            result = &mut backend_call => {
                while let Some(update) = updates_rx.recv().await {
                    record_and_forward_update(
                        connection,
                        session_id,
                        update,
                        &mut streamed_text,
                        &mut forwarding_error,
                    );
                }
                break result;
            }
            update = updates_rx.recv() => {
                let Some(update) = update else {
                    break backend_call.as_mut().await;
                };
                record_and_forward_update(
                    connection,
                    session_id,
                    update,
                    &mut streamed_text,
                    &mut forwarding_error,
                );
            }
        }
    };
    if let Some(current) = sessions.lock().await.get_mut(session_id) {
        current.busy = false;
    }
    let availability_refresh = send_available_commands(connection, session_id.clone(), false);
    let turn = backend_result.map_err(|error| backend_protocol_error(&error))?;
    availability_refresh?;
    if let Some(error) = forwarding_error {
        return Err(error);
    }
    let suffix = turn.text.strip_prefix(&streamed_text).ok_or_else(|| {
        agent_client_protocol::Error::internal_error()
            .data("backend terminal text did not preserve its progressive text prefix")
    })?;
    if !suffix.is_empty() {
        send_text_update(connection, session_id.clone(), suffix.to_owned())?;
    }
    Ok(turn)
}

fn record_and_forward_update(
    connection: &agent_client_protocol::ConnectionTo<agent_client_protocol::Client>,
    session_id: &SessionId,
    update: BackendPromptUpdate,
    streamed_text: &mut String,
    forwarding_error: &mut Option<agent_client_protocol::Error>,
) {
    if forwarding_error.is_some() {
        if let BackendPromptUpdate::TextDelta(text) = update {
            streamed_text.push_str(&text);
        }
        return;
    }
    let result = match update {
        BackendPromptUpdate::TextDelta(text) => {
            streamed_text.push_str(&text);
            send_text_update(connection, session_id.clone(), text)
        }
        BackendPromptUpdate::Usage { used, size } => {
            connection.send_notification(SessionNotification::new(
                session_id.clone(),
                SessionUpdate::UsageUpdate(UsageUpdate::new(used, size)),
            ))
        }
        BackendPromptUpdate::ToolCallStarted(call) => connection.send_notification(
            SessionNotification::new(session_id.clone(), acp_tool_started(call)),
        ),
        BackendPromptUpdate::ToolCallUpdated(call) => connection.send_notification(
            SessionNotification::new(session_id.clone(), acp_tool_updated(call)),
        ),
    };
    if let Err(error) = result {
        *forwarding_error = Some(error);
    }
}

fn acp_tool_started(call: ToolCallView) -> SessionUpdate {
    let content = safe_tool_content(&call);
    let mut projected = AcpToolCall::new(call.call_id.to_string(), call.title)
        .kind(acp_tool_kind(call.kind))
        .status(acp_tool_status(call.status));
    if let Some(content) = content {
        projected = projected.content(vec![content]);
    }
    // Raw input/output remain absent by construction: the interaction domain
    // exposes only safety-policy summaries for durable registered tools.
    SessionUpdate::ToolCall(projected)
}

fn acp_tool_updated(call: ToolCallView) -> SessionUpdate {
    SessionUpdate::ToolCallUpdate(acp_tool_update(call))
}

fn acp_tool_update(call: ToolCallView) -> AcpToolCallUpdate {
    let content = safe_tool_content(&call);
    let mut fields = ToolCallUpdateFields::new()
        .title(call.title)
        .kind(acp_tool_kind(call.kind))
        .status(acp_tool_status(call.status));
    if let Some(content) = content {
        fields = fields.content(vec![content]);
    }
    AcpToolCallUpdate::new(call.call_id.to_string(), fields)
}

fn safe_tool_content(call: &ToolCallView) -> Option<ToolCallContent> {
    let text = match (&call.summary, &call.error) {
        (Some(summary), Some(error)) => Some(format!("{summary}. {error}")),
        (Some(summary), None) => Some(summary.clone()),
        (None, Some(error)) => Some(error.clone()),
        (None, None) => None,
    }?;
    Some(ToolCallContent::from(ContentBlock::Text(TextContent::new(
        text,
    ))))
}

const fn acp_tool_kind(kind: ToolCallKind) -> AcpToolKind {
    match kind {
        ToolCallKind::Read => AcpToolKind::Read,
        ToolCallKind::Write => AcpToolKind::Edit,
        ToolCallKind::Execute => AcpToolKind::Execute,
        ToolCallKind::Network => AcpToolKind::Fetch,
        ToolCallKind::Chain | ToolCallKind::Other => AcpToolKind::Other,
    }
}

const fn acp_tool_status(status: ToolCallStatus) -> AcpToolCallStatus {
    match status {
        ToolCallStatus::Pending | ToolCallStatus::AwaitingApproval => AcpToolCallStatus::Pending,
        ToolCallStatus::InProgress => AcpToolCallStatus::InProgress,
        ToolCallStatus::Succeeded => AcpToolCallStatus::Completed,
        // ACP protocol v1 has no cancelled or unknown variants. Keep the safe
        // detail in content and use its only non-success terminal state.
        ToolCallStatus::Failed | ToolCallStatus::Cancelled | ToolCallStatus::Unknown => {
            AcpToolCallStatus::Failed
        }
    }
}

fn send_text_update(
    connection: &agent_client_protocol::ConnectionTo<agent_client_protocol::Client>,
    session_id: SessionId,
    text: String,
) -> Result<(), agent_client_protocol::Error> {
    connection.send_notification(SessionNotification::new(
        session_id,
        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(TextContent::new(
            text,
        )))),
    ))
}

async fn handle_slash_command(
    command: InteractionCommand,
    session_id: &SessionId,
    session: &EditorSession,
    sessions: &Sessions,
    backend: &dyn AcpBackend,
    registry: &CommandRegistry,
) -> Result<BackendTurn, agent_client_protocol::Error> {
    match command {
        InteractionCommand::Help { command } => {
            Ok(BackendTurn::completed(help_text(
                registry,
                command,
                session.busy,
            )))
        }
        InteractionCommand::Status => Ok(BackendTurn::completed(format!(
            "Session: {session_id}\nWorkspace: {}\nAgent: {}\nModel: {}\nActive prompt: {}",
            session.cwd.display(),
            session.selected_agent.as_deref().unwrap_or("not selected"),
            session.selected_model.as_deref().unwrap_or("agent default"),
            if session.busy { "yes" } else { "no" }
        ))),
        InteractionCommand::Agents => {
            let agents = call_backend(backend.list_agents())
                .await
                .map_err(|error| backend_protocol_error(&error))?;
            Ok(BackendTurn::completed(format_agents(&agents)))
        }
        InteractionCommand::Agent { selector } => match find_agent(backend, &selector)
            .await
            .map_err(|error| backend_protocol_error(&error))?
        {
            Some(_) if session.busy => Ok(BackendTurn::completed(
                "The selected agent cannot change during an active editor prompt.",
            )),
            Some(agent) => {
                let mut all_sessions = sessions.lock().await;
                let current = all_sessions.get_mut(session_id).ok_or_else(|| {
                    agent_client_protocol::Error::invalid_params()
                        .data(format!("unknown session: {session_id}"))
                })?;
                if current.busy {
                    return Ok(BackendTurn::completed(
                        "The selected agent cannot change during an active editor prompt.",
                    ));
                }
                let durable = call_backend(
                    backend.set_agent(session_id.0.as_ref(), &agent.id),
                )
                .await
                .map_err(|error| backend_protocol_error(&error))?;
                current.selected_agent = Some(durable.selected_agent);
                current.selected_model = durable.selected_model;
                Ok(BackendTurn::completed(format!(
                    "Selected agent '{}' ({}).",
                    agent.name, agent.id
                )))
            }
            None => Ok(BackendTurn::completed(format!(
                "Active agent '{selector}' was not found. Use /agents to list choices."
            ))),
        },
        InteractionCommand::Model { model: None } => Ok(BackendTurn::completed(format!(
            "Model: {}",
            session.selected_model.as_deref().unwrap_or("agent default")
        ))),
        InteractionCommand::Model { model: Some(_) } if session.busy => Ok(BackendTurn::completed(
            "The model cannot change during an active editor prompt.",
        )),
        InteractionCommand::Model { model: Some(model) } => {
            let models = call_backend(backend.list_models())
                .await
                .map_err(|error| backend_protocol_error(&error))?;
            let selected = if matches!(model.as_str(), "default" | "inherit") {
                None
            } else if models.iter().any(|candidate| candidate.id == model) {
                Some(model)
            } else {
                return Ok(BackendTurn::completed(format!(
                    "Configured model '{model}' is unavailable. Choose one of: {}.",
                    format_model_choices(&models)
                )));
            };
            let mut all_sessions = sessions.lock().await;
            let current = all_sessions.get_mut(session_id).ok_or_else(|| {
                agent_client_protocol::Error::invalid_params()
                    .data(format!("unknown session: {session_id}"))
            })?;
            if current.busy {
                return Ok(BackendTurn::completed(
                    "The model cannot change during an active editor prompt.",
                ));
            }
            let durable = call_backend(
                backend.set_model(session_id.0.as_ref(), selected.as_deref()),
            )
            .await
            .map_err(|error| backend_protocol_error(&error))?;
            current.selected_agent = Some(durable.selected_agent);
            current.selected_model = durable.selected_model;
            Ok(BackendTurn::completed(format!(
                "Selected model '{}'.",
                current
                    .selected_model
                    .as_deref()
                    .unwrap_or("agent default")
            )))
        }
        InteractionCommand::Runs => {
            let runs = call_backend(backend.list_runs(session_id.0.as_ref()))
                .await
                .map_err(|error| backend_protocol_error(&error))?;
            Ok(BackendTurn::completed(format_run_list(&runs)))
        }
        InteractionCommand::Inspect { run_id } => {
            let run = call_backend(
                backend.inspect_run(session_id.0.as_ref(), &run_id.to_string()),
            )
            .await
            .map_err(|error| backend_protocol_error(&error))?;
            Ok(BackendTurn::completed(format_run_inspection(&run)))
        }
        InteractionCommand::Cancel {
            target: CancelTarget::CurrentTurn,
        } if session.busy => {
            call_backend(backend.cancel(session_id.0.as_ref()))
                .await
                .map_err(|error| backend_protocol_error(&error))?;
            Ok(BackendTurn::completed(
                "Cancellation requested for the active editor prompt.",
            ))
        }
        InteractionCommand::Cancel {
            target: CancelTarget::CurrentTurn,
        } => Ok(BackendTurn::completed(
            "There is no active editor prompt to cancel in this session.",
        )),
        InteractionCommand::Cancel { .. } => Ok(BackendTurn::completed(
            "ACP supports only /cancel without arguments (or /stop) for the current durable turn; run-ID and all-session cancellation are not exposed by this adapter.",
        )),
        unsupported => Ok(BackendTurn::completed(unsupported_command_help(
            unsupported.name(),
        ))),
    }
}

async fn find_agent(
    backend: &dyn AcpBackend,
    selector: &str,
) -> Result<Option<AgentSummary>, BackendCallError> {
    Ok(find_agent_in(&call_backend(backend.list_agents()).await?, selector).cloned())
}

fn find_agent_in<'a>(agents: &'a [AgentSummary], selector: &str) -> Option<&'a AgentSummary> {
    agents
        .iter()
        .find(|agent| agent.id == selector || agent.name == selector)
}

fn available_commands(has_active_prompt: bool) -> Vec<AvailableCommand> {
    let registry = CommandRegistry::mvp();
    available_command_specs(&registry, has_active_prompt)
        .into_iter()
        .map(available_command)
        .collect()
}

fn available_command_specs(
    registry: &CommandRegistry,
    has_active_prompt: bool,
) -> Vec<&CommandSpec> {
    // An ACP session is always backed by a durable conversation; availability only
    // needs to know that one is present, not its persisted identifier.
    let context = CommandContext {
        conversation_id: Some(ConversationId::new()),
        has_active_turn: has_active_prompt,
        pending_approval_count: 0,
        can_mutate: true,
    };
    registry
        .available(&context)
        .into_iter()
        .filter(|spec| ACP_COMMANDS.contains(&spec.command))
        .collect()
}

fn available_command(spec: &CommandSpec) -> AvailableCommand {
    let mut command = AvailableCommand::new(&spec.name, acp_description(spec));
    if let Some(hint) = acp_input_hint(spec) {
        command = command.input(AvailableCommandInput::Unstructured(
            UnstructuredCommandInput::new(hint),
        ));
    }
    command
}

fn acp_description(spec: &CommandSpec) -> &str {
    match spec.command {
        CommandName::Status => "Show the editor workspace, selected agent, and prompt activity",
        CommandName::Agents => "List active Polkagent agents",
        CommandName::Model => "Show or select the model for this editor session",
        CommandName::Cancel => "Cancel the active prompt in this editor session",
        _ => &spec.description,
    }
}

fn acp_input_hint(spec: &CommandSpec) -> Option<&str> {
    if spec.command == CommandName::Cancel {
        None
    } else {
        spec.input_hint.as_deref()
    }
}

fn help_text(
    registry: &CommandRegistry,
    command: Option<CommandName>,
    has_active_prompt: bool,
) -> String {
    if let Some(command) = command {
        if !ACP_COMMANDS.contains(&command) {
            return unsupported_command_help(command);
        }
        let detail = registry.resolve(command.as_str()).map_or_else(
            || format!("No help is available for /{}.", command.as_str()),
            command_help,
        );
        if command == CommandName::Cancel && !has_active_prompt {
            return format!(
                "{detail}\nCurrently unavailable: this editor session has no active prompt."
            );
        }
        return detail;
    }

    let commands = available_command_specs(registry, has_active_prompt)
        .into_iter()
        .map(command_help)
        .collect::<Vec<_>>()
        .join("\n");
    format!("Polkagent ACP commands currently available:\n{commands}")
}

fn unsupported_command_help(command: CommandName) -> String {
    match command {
        CommandName::New => concat!(
            "/new is represented by ACP session/new. Create a new editor thread instead; ",
            "a slash command cannot replace the current ACP session ID."
        )
        .to_owned(),
        CommandName::Resume => concat!(
            "/resume is represented by ACP session/load and session/resume. Reopen the durable ",
            "editor thread with its conversation ID and exact original workspace."
        )
        .to_owned(),
        CommandName::Approve | CommandName::Deny => format!(
            "/{} is not exposed by ACP until APR-07 binds durable permission coordination.",
            command.as_str()
        ),
        _ => format!(
            "/{} is part of the shared Polkagent registry but is not supported by ACP.",
            command.as_str()
        ),
    }
}

fn command_help(spec: &CommandSpec) -> String {
    let hint = acp_input_hint(spec).map_or(String::new(), |hint| format!(" {hint}"));
    let aliases = if spec.aliases.is_empty() {
        String::new()
    } else {
        format!(
            " (aliases: {})",
            spec.aliases
                .iter()
                .map(|alias| format!("/{alias}"))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    format!("/{}{hint} — {}{aliases}", spec.name, acp_description(spec))
}

fn format_agents(agents: &[AgentSummary]) -> String {
    if agents.is_empty() {
        return "No active agents. Create and start one with the Polkagent CLI first.".to_owned();
    }
    let rows = agents
        .iter()
        .map(|agent| format!("- {} ({}) [{}]", agent.name, agent.id, agent.state))
        .collect::<Vec<_>>()
        .join("\n");
    format!("Active agents:\n{rows}")
}

fn format_model_choices(models: &[ModelSummary]) -> String {
    if models.is_empty() {
        return "agent default (no explicit configured models)".to_owned();
    }
    models
        .iter()
        .map(|model| model.id.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

fn backend_protocol_error(error: &BackendCallError) -> agent_client_protocol::Error {
    match error {
        BackendCallError::Rejected(error) => {
            let detail = polkagent_telemetry::redact_string(&error.to_string());
            match error.kind {
                BackendErrorKind::InvalidRequest | BackendErrorKind::NotFound => {
                    agent_client_protocol::Error::invalid_params().data(detail)
                }
                BackendErrorKind::Conflict | BackendErrorKind::Unsupported => {
                    agent_client_protocol::Error::invalid_request().data(detail)
                }
                BackendErrorKind::Internal => {
                    agent_client_protocol::Error::internal_error().data(detail)
                }
            }
        }
        BackendCallError::Panicked => {
            agent_client_protocol::Error::internal_error().data(BACKEND_PANIC_DETAIL)
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "protocol fixtures fail fast at exact official SDK or synchronization boundaries"
)]
mod tests {
    use super::*;

    fn tool_view(status: ToolCallStatus) -> ToolCallView {
        ToolCallView {
            call_id: polkagent_interaction::ToolCallId::new(),
            run_id: polkagent_core::RunId::new(),
            name: "test.observe".to_owned(),
            title: "test.observe".to_owned(),
            kind: ToolCallKind::Other,
            status,
            arguments: None,
            summary: Some("Output withheld by interaction safety policy".to_owned()),
            output: None,
            locations: Vec::new(),
            diff: None,
            error: None,
        }
    }

    struct PanickingBackend;

    struct BlockingDiscoveryBackend {
        started: Arc<tokio::sync::Notify>,
        release: Arc<tokio::sync::Notify>,
    }

    #[async_trait]
    impl AcpBackend for PanickingBackend {
        async fn list_agents(&self) -> Result<Vec<AgentSummary>, BackendError> {
            panic!("synthetic backend panic payload");
        }

        async fn list_models(&self) -> Result<Vec<ModelSummary>, BackendError> {
            panic!("synthetic backend panic payload");
        }

        async fn new_session(
            &self,
            _cwd: &Path,
            _agent: Option<&str>,
            _model: Option<&str>,
        ) -> Result<BackendSession, BackendError> {
            panic!("synthetic backend panic payload");
        }

        async fn load_session(
            &self,
            _session_id: &str,
            _cwd: &Path,
            _include_transcript: bool,
        ) -> Result<BackendSession, BackendError> {
            panic!("synthetic backend panic payload");
        }

        async fn set_agent(
            &self,
            _session_id: &str,
            _agent_id: &str,
        ) -> Result<BackendSession, BackendError> {
            panic!("synthetic backend panic payload");
        }

        async fn set_model(
            &self,
            _session_id: &str,
            _model: Option<&str>,
        ) -> Result<BackendSession, BackendError> {
            panic!("synthetic backend panic payload");
        }

        async fn prompt(
            &self,
            _session_id: &str,
            _cwd: &Path,
            _turn_id: &str,
            _prompt: &str,
            _updates: tokio::sync::mpsc::Sender<BackendPromptUpdate>,
        ) -> Result<BackendTurn, BackendError> {
            panic!("synthetic backend panic payload");
        }

        async fn cancel(&self, _session_id: &str) -> Result<(), BackendError> {
            panic!("synthetic backend panic payload");
        }
    }

    #[async_trait]
    impl AcpBackend for BlockingDiscoveryBackend {
        async fn list_agents(&self) -> Result<Vec<AgentSummary>, BackendError> {
            self.started.notify_one();
            self.release.notified().await;
            Ok(vec![AgentSummary {
                id: "new-agent-id".to_owned(),
                name: "new-agent".to_owned(),
                state: "active".to_owned(),
            }])
        }

        async fn list_models(&self) -> Result<Vec<ModelSummary>, BackendError> {
            self.started.notify_one();
            self.release.notified().await;
            Ok(vec![ModelSummary {
                id: "new-model".to_owned(),
                name: "new-model".to_owned(),
            }])
        }

        async fn new_session(
            &self,
            _cwd: &Path,
            _agent: Option<&str>,
            _model: Option<&str>,
        ) -> Result<BackendSession, BackendError> {
            panic!("new session is not expected in configuration race test");
        }

        async fn load_session(
            &self,
            _session_id: &str,
            _cwd: &Path,
            _include_transcript: bool,
        ) -> Result<BackendSession, BackendError> {
            panic!("load session is not expected in configuration race test");
        }

        async fn set_agent(
            &self,
            _session_id: &str,
            _agent_id: &str,
        ) -> Result<BackendSession, BackendError> {
            panic!("agent update is not expected after busy recheck");
        }

        async fn set_model(
            &self,
            _session_id: &str,
            _model: Option<&str>,
        ) -> Result<BackendSession, BackendError> {
            panic!("model update is not expected after busy recheck");
        }

        async fn prompt(
            &self,
            _session_id: &str,
            _cwd: &Path,
            _turn_id: &str,
            _prompt: &str,
            _updates: tokio::sync::mpsc::Sender<BackendPromptUpdate>,
        ) -> Result<BackendTurn, BackendError> {
            panic!("prompt is not expected in configuration race test");
        }

        async fn cancel(&self, _session_id: &str) -> Result<(), BackendError> {
            panic!("cancel is not expected in configuration race test");
        }
    }

    #[test]
    fn shared_registry_parses_aliases_and_quoted_arguments() {
        let registry = CommandRegistry::mvp();
        let Ok(ParsedLine::Command(agent)) = registry.parse("  /use \"research bot\"  ") else {
            panic!("expected a parsed agent command");
        };
        assert_eq!(
            agent.command,
            InteractionCommand::Agent {
                selector: "research bot".to_owned()
            }
        );
        let Ok(ParsedLine::Command(cancel)) = registry.parse("/STOP") else {
            panic!("expected a parsed cancellation command");
        };
        assert_eq!(
            cancel.command,
            InteractionCommand::Cancel {
                target: CancelTarget::CurrentTurn
            }
        );
    }

    #[test]
    fn official_schema_keeps_tool_identity_and_replacement_status_without_raw_payloads() {
        let started = tool_view(ToolCallStatus::InProgress);
        let call_id = started.call_id.to_string();
        let SessionUpdate::ToolCall(projected_start) = acp_tool_started(started) else {
            panic!("expected official ACP tool-call notification");
        };
        assert_eq!(projected_start.tool_call_id.0.as_ref(), call_id);
        assert_eq!(projected_start.status, AcpToolCallStatus::InProgress);
        assert!(projected_start.raw_input.is_none());
        assert!(projected_start.raw_output.is_none());

        let mut completed = tool_view(ToolCallStatus::Succeeded);
        completed.call_id = call_id.parse().expect("reuse durable interaction ID");
        let SessionUpdate::ToolCallUpdate(projected_update) = acp_tool_updated(completed) else {
            panic!("expected official ACP tool-update notification");
        };
        assert_eq!(projected_update.tool_call_id.0.as_ref(), call_id);
        assert_eq!(
            projected_update.fields.status,
            Some(AcpToolCallStatus::Completed)
        );
        assert!(projected_update.fields.raw_input.is_none());
        assert!(projected_update.fields.raw_output.is_none());
        let wire = serde_json::to_value(SessionUpdate::ToolCallUpdate(projected_update))
            .expect("encode official ACP notification");
        assert_eq!(wire["sessionUpdate"], "tool_call_update");
        assert_eq!(wire["toolCallId"], call_id);
        assert_eq!(wire["status"], "completed");
        assert!(wire.get("rawInput").is_none());
        assert!(wire.get("rawOutput").is_none());
    }

    #[test]
    fn permission_contract_offers_only_once_choices_and_safe_tool_data() {
        let call = tool_view(ToolCallStatus::AwaitingApproval);
        let call_id = call.call_id.to_string();
        let request = permission_request(SessionId::new("permission-session"), call);

        assert_eq!(request.session_id.0.as_ref(), "permission-session");
        assert_eq!(request.tool_call.tool_call_id.0.as_ref(), call_id);
        assert_eq!(request.options.len(), 2);
        assert_eq!(
            request.options[0].option_id.0.as_ref(),
            ALLOW_ONCE_OPTION_ID
        );
        assert_eq!(request.options[0].kind, PermissionOptionKind::AllowOnce);
        assert_eq!(
            request.options[1].option_id.0.as_ref(),
            REJECT_ONCE_OPTION_ID
        );
        assert_eq!(request.options[1].kind, PermissionOptionKind::RejectOnce);

        let wire = serde_json::to_value(&request).expect("encode permission request");
        assert_eq!(wire["toolCall"]["toolCallId"], call_id);
        assert_eq!(wire["toolCall"]["status"], "pending");
        assert!(wire["toolCall"].get("rawInput").is_none());
        assert!(wire["toolCall"].get("rawOutput").is_none());
        assert_eq!(wire["options"].as_array().map(Vec::len), Some(2));
    }

    #[test]
    fn permission_response_rejects_unadvertised_or_remembered_choices() {
        use agent_client_protocol::schema::v1::SelectedPermissionOutcome;

        let response = RequestPermissionResponse::new(RequestPermissionOutcome::Selected(
            SelectedPermissionOutcome::new("polkagent.allow_always"),
        ));
        let error = permission_decision(&response)
            .expect_err("an unadvertised remembered decision must fail closed");
        assert_eq!(
            error.code,
            agent_client_protocol::Error::invalid_params().code
        );
    }

    #[tokio::test]
    async fn official_client_round_trips_allow_reject_and_cancel_permission_outcomes() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        use agent_client_protocol::schema::v1::SelectedPermissionOutcome;

        let observed = Arc::new(std::sync::Mutex::new(Vec::new()));
        let observed_client = Arc::clone(&observed);
        let response_index = Arc::new(AtomicUsize::new(0));
        let response_index_client = Arc::clone(&response_index);
        let decisions = Arc::new(std::sync::Mutex::new(Vec::new()));
        let decisions_agent = Arc::clone(&decisions);
        let completion = Arc::new(tokio::sync::Notify::new());
        let completion_client = Arc::clone(&completion);
        let (client_transport, agent_transport) = agent_client_protocol::Channel::duplex();
        let calls = [
            tool_view(ToolCallStatus::AwaitingApproval),
            tool_view(ToolCallStatus::AwaitingApproval),
            tool_view(ToolCallStatus::AwaitingApproval),
        ];
        let expected_call_ids = calls
            .iter()
            .map(|call| call.call_id.to_string())
            .collect::<Vec<_>>();

        let client = agent_client_protocol::Client
            .builder()
            .on_receive_request(
                async move |request: RequestPermissionRequest, responder, _connection| {
                    observed_client
                        .lock()
                        .expect("permission observation lock")
                        .push(request.clone());
                    let index = response_index_client.fetch_add(1, Ordering::SeqCst);
                    let outcome = match index {
                        0 => RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(
                            request.options[0].option_id.clone(),
                        )),
                        1 => RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(
                            request.options[1].option_id.clone(),
                        )),
                        _ => RequestPermissionOutcome::Cancelled,
                    };
                    responder.respond(RequestPermissionResponse::new(outcome))
                },
                agent_client_protocol::on_receive_request!(),
            )
            .connect_with(client_transport, async move |_connection| {
                completion_client.notified().await;
                Ok(())
            });
        let agent = agent_client_protocol::Agent.builder().connect_with(
            agent_transport,
            async move |connection| {
                for call in calls {
                    let response = connection
                        .send_request(permission_request(
                            SessionId::new("permission-session"),
                            call,
                        ))
                        .block_task()
                        .await?;
                    decisions_agent
                        .lock()
                        .expect("permission decision lock")
                        .push(permission_decision(&response)?);
                }
                completion.notify_one();
                Ok(())
            },
        );
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            futures::try_join!(client, agent)
        })
        .await
        .expect("official ACP permission harness timed out")
        .expect("official ACP permission harness failed");

        assert_eq!(
            *decisions.lock().expect("final decision lock"),
            vec![
                PermissionDecision::AllowOnce,
                PermissionDecision::RejectOnce,
                PermissionDecision::Cancelled,
            ]
        );
        let observed = observed.lock().expect("final permission observation lock");
        assert_eq!(observed.len(), 3);
        let observed_call_ids = observed
            .iter()
            .map(|request| request.tool_call.tool_call_id.0.to_string())
            .collect::<Vec<_>>();
        assert_eq!(observed_call_ids, expected_call_ids);
        for request in observed.iter() {
            assert_eq!(request.options.len(), 2);
            assert_eq!(
                request.options[0].option_id.0.as_ref(),
                ALLOW_ONCE_OPTION_ID
            );
            assert_eq!(request.options[0].kind, PermissionOptionKind::AllowOnce);
            assert_eq!(
                request.options[1].option_id.0.as_ref(),
                REJECT_ONCE_OPTION_ID
            );
            assert_eq!(request.options[1].kind, PermissionOptionKind::RejectOnce);
            let wire = serde_json::to_value(request).expect("encode observed permission request");
            assert!(wire["toolCall"].get("rawInput").is_none());
            assert!(wire["toolCall"].get("rawOutput").is_none());
        }
    }

    #[tokio::test]
    async fn official_client_session_cancel_resolves_pending_permission_as_cancelled() {
        let session_id = SessionId::new("cancelled-permission-session");
        let (pending_tx, mut pending_rx) = tokio::sync::mpsc::unbounded_channel::<
            agent_client_protocol::Responder<RequestPermissionResponse>,
        >();
        let cancellation_observed = Arc::new(tokio::sync::Notify::new());
        let cancellation_observed_by_client = Arc::clone(&cancellation_observed);
        let observed_session = Arc::new(std::sync::Mutex::new(None));
        let observed_session_by_agent = Arc::clone(&observed_session);
        let agent_finished = Arc::new(tokio::sync::Notify::new());
        let agent_finished_by_client = Arc::clone(&agent_finished);
        let decision = Arc::new(std::sync::Mutex::new(None));
        let decision_by_agent = Arc::clone(&decision);
        let client_session_id = session_id.clone();
        let agent_session_id = session_id.clone();
        let (client_transport, agent_transport) = agent_client_protocol::Channel::duplex();

        let client = agent_client_protocol::Client
            .builder()
            .on_receive_request(
                async move |_request: RequestPermissionRequest, responder, _connection| {
                    pending_tx.send(responder).map_err(|_| {
                        agent_client_protocol::Error::internal_error()
                            .data("permission responder receiver dropped")
                    })
                },
                agent_client_protocol::on_receive_request!(),
            )
            .connect_with(client_transport, async move |connection| {
                let responder = pending_rx.recv().await.ok_or_else(|| {
                    agent_client_protocol::Error::internal_error()
                        .data("permission request was not observed")
                })?;
                connection.send_notification(CancelNotification::new(client_session_id))?;
                cancellation_observed_by_client.notified().await;
                responder.respond(RequestPermissionResponse::new(
                    RequestPermissionOutcome::Cancelled,
                ))?;
                agent_finished_by_client.notified().await;
                Ok(())
            });
        let agent = agent_client_protocol::Agent
            .builder()
            .on_receive_notification(
                async move |notification: CancelNotification, _connection| {
                    *observed_session_by_agent
                        .lock()
                        .expect("cancel observation lock") = Some(notification.session_id);
                    cancellation_observed.notify_one();
                    Ok(())
                },
                agent_client_protocol::on_receive_notification!(),
            )
            .connect_with(agent_transport, async move |connection| {
                let response = connection
                    .send_request(permission_request(
                        agent_session_id,
                        tool_view(ToolCallStatus::AwaitingApproval),
                    ))
                    .block_task()
                    .await?;
                *decision_by_agent.lock().expect("cancel decision lock") =
                    Some(permission_decision(&response)?);
                agent_finished.notify_one();
                Ok(())
            });

        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            futures::try_join!(client, agent)
        })
        .await
        .expect("official ACP cancellation harness timed out")
        .expect("official ACP cancellation harness failed");

        assert_eq!(
            *observed_session
                .lock()
                .expect("final cancel observation lock"),
            Some(session_id)
        );
        assert_eq!(
            *decision.lock().expect("final cancel decision lock"),
            Some(PermissionDecision::Cancelled)
        );
    }

    #[tokio::test]
    async fn official_client_unknown_permission_option_and_protocol_error_fail_closed() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        use agent_client_protocol::schema::v1::SelectedPermissionOutcome;

        let response_index = Arc::new(AtomicUsize::new(0));
        let response_index_by_client = Arc::clone(&response_index);
        let agent_finished = Arc::new(tokio::sync::Notify::new());
        let agent_finished_by_client = Arc::clone(&agent_finished);
        let observed_error_codes = Arc::new(std::sync::Mutex::new(Vec::new()));
        let observed_error_codes_by_agent = Arc::clone(&observed_error_codes);
        let (client_transport, agent_transport) = agent_client_protocol::Channel::duplex();

        let client = agent_client_protocol::Client
            .builder()
            .on_receive_request(
                async move |_request: RequestPermissionRequest, responder, _connection| {
                    match response_index_by_client.fetch_add(1, Ordering::SeqCst) {
                        0 => responder.respond(RequestPermissionResponse::new(
                            RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(
                                "polkagent.allow_always",
                            )),
                        )),
                        _ => responder.respond_with_error(
                            agent_client_protocol::Error::invalid_request()
                                .data("synthetic client permission failure"),
                        ),
                    }
                },
                agent_client_protocol::on_receive_request!(),
            )
            .connect_with(client_transport, async move |_connection| {
                agent_finished_by_client.notified().await;
                Ok(())
            });
        let agent = agent_client_protocol::Agent.builder().connect_with(
            agent_transport,
            async move |connection| {
                let unknown_response = connection
                    .send_request(permission_request(
                        SessionId::new("unknown-option-session"),
                        tool_view(ToolCallStatus::AwaitingApproval),
                    ))
                    .block_task()
                    .await?;
                let unknown_error = permission_decision(&unknown_response)
                    .expect_err("an unknown option must not become an approval");

                let protocol_error = connection
                    .send_request(permission_request(
                        SessionId::new("permission-error-session"),
                        tool_view(ToolCallStatus::AwaitingApproval),
                    ))
                    .block_task()
                    .await
                    .expect_err("a client protocol error must remain an error");
                observed_error_codes_by_agent
                    .lock()
                    .expect("permission error code lock")
                    .extend([unknown_error.code, protocol_error.code]);
                agent_finished.notify_one();
                Ok(())
            },
        );

        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            futures::try_join!(client, agent)
        })
        .await
        .expect("official ACP permission error harness timed out")
        .expect("official ACP permission error harness failed");

        assert_eq!(response_index.load(Ordering::SeqCst), 2);
        assert_eq!(
            *observed_error_codes
                .lock()
                .expect("final permission error code lock"),
            vec![
                agent_client_protocol::Error::invalid_params().code,
                agent_client_protocol::Error::invalid_request().code,
            ]
        );
    }

    #[tokio::test]
    async fn official_client_disconnect_fails_pending_permission_without_a_decision() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let request_observed = Arc::new(tokio::sync::Notify::new());
        let request_observed_by_client = Arc::clone(&request_observed);
        let request_failed_closed = Arc::new(AtomicBool::new(false));
        let request_failed_closed_by_agent = Arc::clone(&request_failed_closed);
        let agent_finished = Arc::new(tokio::sync::Notify::new());
        let agent_finished_by_client = Arc::clone(&agent_finished);
        let (client_transport, agent_transport) = agent_client_protocol::Channel::duplex();
        let close_client_output = client_transport.tx.clone();

        let client = agent_client_protocol::Client
            .builder()
            .on_receive_request(
                async move |_request: RequestPermissionRequest, _responder, _connection| {
                    request_observed_by_client.notify_one();
                    Ok(())
                },
                agent_client_protocol::on_receive_request!(),
            )
            .connect_with(client_transport, async move |_connection| {
                request_observed.notified().await;
                close_client_output.close_channel();
                agent_finished_by_client.notified().await;
                Ok(())
            });
        let agent = agent_client_protocol::Agent.builder().connect_with(
            agent_transport,
            async move |connection| {
                let error = connection
                    .send_request(permission_request(
                        SessionId::new("disconnected-permission-session"),
                        tool_view(ToolCallStatus::AwaitingApproval),
                    ))
                    .block_task()
                    .await
                    .expect_err("disconnect must fail a pending permission request");
                request_failed_closed_by_agent.store(
                    agent_client_protocol::is_incoming_transport_closed(&error),
                    Ordering::SeqCst,
                );
                agent_finished.notify_one();
                Ok(())
            },
        );

        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            futures::try_join!(client, agent)
        })
        .await
        .expect("official ACP permission disconnect harness timed out")
        .expect("official ACP permission disconnect harness failed");

        assert!(request_failed_closed.load(Ordering::SeqCst));
    }

    #[test]
    fn editor_session_roots_require_exact_lexical_cwd_and_no_expansion() {
        assert!(validate_session_roots(Path::new("/workspace/alpha"), true, true).is_ok());
        for invalid in [
            Path::new("relative/workspace"),
            Path::new("/workspace/alpha/../beta"),
            Path::new("/workspace/./alpha"),
            Path::new("/workspace//alpha"),
        ] {
            let error = validate_session_roots(invalid, true, true)
                .expect_err("unstable cwd spelling must fail");
            assert_eq!(
                error.code,
                agent_client_protocol::Error::invalid_params().code
            );
        }
        assert!(validate_session_roots(Path::new("/workspace/alpha"), false, true).is_err());
        assert!(validate_session_roots(Path::new("/workspace/alpha"), true, false).is_err());
    }

    #[tokio::test]
    async fn official_client_receives_structured_tool_replacement_with_exact_identity() {
        let started = tool_view(ToolCallStatus::InProgress);
        let call_id = started.call_id.to_string();
        let mut completed = tool_view(ToolCallStatus::Succeeded);
        completed.call_id = started.call_id;
        let notifications = [
            SessionNotification::new(SessionId::new("tool-session"), acp_tool_started(started)),
            SessionNotification::new(SessionId::new("tool-session"), acp_tool_updated(completed)),
        ];
        let observed = Arc::new(std::sync::Mutex::new(Vec::new()));
        let observed_client = Arc::clone(&observed);
        let completion = Arc::new(tokio::sync::Notify::new());
        let completion_client = Arc::clone(&completion);
        let (ack_tx, ack_rx) = futures::channel::oneshot::channel();
        let ack_tx = Arc::new(std::sync::Mutex::new(Some(ack_tx)));
        let ack_client = Arc::clone(&ack_tx);
        let (client_transport, agent_transport) = agent_client_protocol::Channel::duplex();

        let client = agent_client_protocol::Client
            .builder()
            .on_receive_notification(
                async move |notification: SessionNotification, _connection| {
                    let mut observed = observed_client.lock().expect("observation lock");
                    if matches!(
                        notification.update,
                        SessionUpdate::ToolCall(_) | SessionUpdate::ToolCallUpdate(_)
                    ) {
                        observed.push(notification.update);
                    }
                    if observed.len() == 2 {
                        if let Some(ack) = ack_client.lock().expect("ack lock").take() {
                            let _ = ack.send(());
                        }
                        completion_client.notify_one();
                    }
                    Ok(())
                },
                agent_client_protocol::on_receive_notification!(),
            )
            .connect_with(client_transport, async move |_connection| {
                completion.notified().await;
                Ok(())
            });
        let agent = agent_client_protocol::Agent.builder().connect_with(
            agent_transport,
            async move |connection| {
                for notification in notifications {
                    connection.send_notification(notification)?;
                }
                ack_rx
                    .await
                    .map_err(|_| agent_client_protocol::Error::internal_error())?;
                Ok(())
            },
        );
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            futures::try_join!(client, agent)
        })
        .await
        .expect("official ACP client timed out")
        .expect("official ACP connection failed");

        let observed = observed.lock().expect("final observation lock");
        let SessionUpdate::ToolCall(started) = &observed[0] else {
            panic!("official client did not receive tool_call first");
        };
        let SessionUpdate::ToolCallUpdate(completed) = &observed[1] else {
            panic!("official client did not receive tool_call_update second");
        };
        assert_eq!(started.tool_call_id.0.as_ref(), call_id);
        assert_eq!(completed.tool_call_id.0.as_ref(), call_id);
        assert_eq!(started.status, AcpToolCallStatus::InProgress);
        assert_eq!(completed.fields.status, Some(AcpToolCallStatus::Completed));
    }

    #[test]
    fn formats_empty_agent_list_actionably() {
        assert!(format_agents(&[]).contains("Create and start"));
    }

    #[test]
    fn advertises_only_registry_commands_available_in_the_editor_state() {
        let commands = available_commands(false);
        let names = commands
            .iter()
            .map(|command| command.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            vec!["help", "status", "agents", "agent", "runs", "inspect", "model"]
        );
        assert!(commands.iter().all(|command| command.name != "cancel"));
        let active_names = available_commands(true)
            .into_iter()
            .map(|command| command.name)
            .collect::<Vec<_>>();
        assert_eq!(
            active_names,
            vec!["help", "status", "agents", "agent", "runs", "inspect", "cancel", "model"]
        );
        let Some(agent) = commands.iter().find(|command| command.name == "agent") else {
            panic!("agent command was not advertised");
        };
        let Some(AvailableCommandInput::Unstructured(input)) = agent.input.as_ref() else {
            panic!("agent command should expose its registry input hint");
        };
        assert_eq!(input.hint, "<name-or-id>");

        let registry = CommandRegistry::mvp();
        for name in [CommandName::Runs, CommandName::Inspect] {
            let spec = registry.resolve(name.as_str()).expect("registry command");
            let advertised = commands
                .iter()
                .find(|command| command.name == spec.name)
                .expect("run command is advertised");
            assert_eq!(advertised.description, spec.description);
            assert_eq!(
                advertised.input.as_ref().and_then(|input| match input {
                    AvailableCommandInput::Unstructured(input) => Some(input.hint.as_str()),
                    _ => None,
                }),
                spec.input_hint.as_deref()
            );
        }
    }

    #[test]
    fn help_uses_registry_aliases_and_exposes_run_commands() {
        let registry = CommandRegistry::mvp();
        let help = help_text(&registry, None, false);
        assert!(help.contains("/agent <name-or-id>"));
        assert!(help.contains("/use"));
        assert!(help.contains("/model [id]"));
        assert!(!help.contains("/cancel"));
        assert!(help.contains("/runs"));
        assert!(help.contains("/inspect <run-id>"));

        let active_help = help_text(&registry, None, true);
        assert!(active_help.contains("/cancel"));
        assert!(active_help.contains("/stop"));

        let inactive_cancel = help_text(&registry, Some(CommandName::Cancel), false);
        assert!(inactive_cancel.contains("Currently unavailable"));
        assert!(inactive_cancel.contains("/stop"));

        let runs = help_text(&registry, Some(CommandName::Runs), false);
        assert!(runs.contains("List recent and active runs for the interaction"));
    }

    #[test]
    fn help_maps_session_lifecycle_commands_to_native_acp_operations() {
        let registry = CommandRegistry::mvp();
        let new = help_text(&registry, Some(CommandName::New), false);
        let resume = help_text(&registry, Some(CommandName::Resume), false);
        let approve = help_text(&registry, Some(CommandName::Approve), false);
        assert!(new.contains("ACP session/new"));
        assert!(new.contains("cannot replace the current ACP session ID"));
        assert!(resume.contains("ACP session/load and session/resume"));
        assert!(approve.contains("APR-07"));
        assert!(approve.contains("durable permission coordination"));
        assert!(!new.contains("Approval commands"));
        assert!(!resume.contains("Approval commands"));
    }

    #[test]
    fn redacts_backend_error_details_before_json_rpc() {
        let raw_secret = "sk-acpProtocolSafetyFixture123";
        let error = BackendCallError::Rejected(BackendError::new(format!(
            "provider rejected credential {raw_secret}"
        )));
        let protocol_error = backend_protocol_error(&error);
        let detail = protocol_error
            .data
            .as_ref()
            .and_then(|value| value.as_str())
            .unwrap_or_default();

        assert!(detail.contains("sk-***REDACTED***"));
        assert!(!detail.contains(raw_secret));
    }

    #[tokio::test]
    async fn contains_backend_panics_without_exposing_payload() {
        let result = call_backend(PanickingBackend.list_agents()).await;
        let Err(error) = result else {
            panic!("backend panic unexpectedly crossed the safety boundary");
        };
        let protocol_error = backend_protocol_error(&error);
        let detail = protocol_error
            .data
            .as_ref()
            .and_then(|value| value.as_str())
            .unwrap_or_default();

        assert_eq!(detail, BACKEND_PANIC_DETAIL);
        assert!(!detail.contains("synthetic backend panic payload"));
    }

    #[tokio::test]
    async fn slash_configuration_rechecks_busy_after_discovery() {
        assert_slash_config_race_is_refused(InteractionCommand::Agent {
            selector: "new-agent".to_owned(),
        })
        .await;
        assert_slash_config_race_is_refused(InteractionCommand::Model {
            model: Some("new-model".to_owned()),
        })
        .await;
    }

    async fn assert_slash_config_race_is_refused(command: InteractionCommand) {
        let session_id = SessionId::new("config-race");
        let session = EditorSession {
            cwd: PathBuf::from("/fixture"),
            selected_agent: Some("old-agent-id".to_owned()),
            selected_model: Some("old-model".to_owned()),
            busy: false,
        };
        let sessions: Sessions = Arc::new(Mutex::new(HashMap::from([(
            session_id.clone(),
            session.clone(),
        )])));
        let backend = Arc::new(BlockingDiscoveryBackend {
            started: Arc::new(tokio::sync::Notify::new()),
            release: Arc::new(tokio::sync::Notify::new()),
        });
        let task_sessions = Arc::clone(&sessions);
        let task_backend = Arc::clone(&backend);
        let task_session_id = session_id.clone();
        let task = tokio::spawn(async move {
            handle_slash_command(
                command,
                &task_session_id,
                &session,
                &task_sessions,
                task_backend.as_ref(),
                &CommandRegistry::mvp(),
            )
            .await
        });

        backend.started.notified().await;
        let mut locked_sessions = sessions.lock().await;
        let Some(current) = locked_sessions.get_mut(&session_id) else {
            panic!("race test session disappeared");
        };
        current.busy = true;
        drop(locked_sessions);
        backend.release.notify_one();
        let outcome = match task.await {
            Ok(Ok(outcome)) => outcome,
            Ok(Err(error)) => panic!("configuration command failed: {error}"),
            Err(error) => panic!("configuration task failed to join: {error}"),
        };
        assert!(outcome.text.contains("cannot change"));
        let locked_sessions = sessions.lock().await;
        let Some(current) = locked_sessions.get(&session_id) else {
            panic!("race test session disappeared");
        };
        assert_eq!(current.selected_agent.as_deref(), Some("old-agent-id"));
        assert_eq!(current.selected_model.as_deref(), Some("old-model"));
    }
}
