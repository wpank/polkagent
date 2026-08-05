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
    InitializeResponse, NewSessionRequest, NewSessionResponse, PromptRequest, PromptResponse,
    ResourceLink, SessionConfigOption, SessionConfigOptionCategory, SessionConfigSelectOption,
    SessionId, SessionNotification, SessionUpdate, SetSessionConfigOptionRequest,
    SetSessionConfigOptionResponse, StopReason, TextContent, UnstructuredCommandInput,
};
use agent_client_protocol::{Agent, Stdio};
use async_trait::async_trait;
use futures::FutureExt as _;
use polkagent_interaction::{
    CancelTarget, CommandName, CommandRegistry, CommandSpec, InteractionCommand, ParsedLine,
};
use tokio::sync::Mutex;

const BACKEND_PANIC_DETAIL: &str = "Polkagent's ACP backend panicked; the request was stopped";
const AGENT_CONFIG_ID: &str = "polkagent.agent";
const MODEL_CONFIG_ID: &str = "model";
const NO_AGENT_VALUE: &str = "_polkagent_no_agent";
const INHERIT_MODEL_VALUE: &str = "_polkagent_agent_model";
const ACP_COMMANDS: [CommandName; 6] = [
    CommandName::Help,
    CommandName::Status,
    CommandName::Agents,
    CommandName::Agent,
    CommandName::Model,
    CommandName::Cancel,
];

/// A backend failure that is safe to return through an ACP error response.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct BackendError {
    message: String,
}

impl BackendError {
    /// Construct a backend error with a user-facing message.
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
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

/// The result of one backend prompt turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendTurn {
    /// Text to stream to the ACP client.
    pub text: String,
    /// Whether cancellation ended the turn.
    pub cancelled: bool,
}

impl BackendTurn {
    /// Construct a successful turn.
    #[must_use]
    pub fn completed(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            cancelled: false,
        }
    }

    /// Construct a cancelled turn.
    #[must_use]
    pub fn cancelled(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            cancelled: true,
        }
    }
}

/// Runtime operations required by the ACP surface.
#[async_trait]
pub trait AcpBackend: Send + Sync + 'static {
    /// List agents that an editor session may select.
    async fn list_agents(&self) -> Result<Vec<AgentSummary>, BackendError>;

    /// List configured models that an editor session may select.
    async fn list_models(&self) -> Result<Vec<ModelSummary>, BackendError>;

    /// Execute one prompt through Polkagent's orchestration runtime.
    async fn prompt(
        &self,
        session_id: &str,
        cwd: &Path,
        agent: &str,
        model: Option<&str>,
        prompt: &str,
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
pub async fn serve_stdio(
    backend: Arc<dyn AcpBackend>,
    config: ServerConfig,
) -> Result<(), agent_client_protocol::Error> {
    let sessions: Sessions = Arc::new(Mutex::new(HashMap::new()));
    let new_sessions = Arc::clone(&sessions);
    let prompt_sessions = Arc::clone(&sessions);
    let cancel_sessions = Arc::clone(&sessions);
    let config_sessions = Arc::clone(&sessions);
    let new_backend = Arc::clone(&backend);
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
                    .agent_capabilities(AgentCapabilities::new())
                    .agent_info(
                        Implementation::new("polkagent", env!("CARGO_PKG_VERSION"))
                            .title("Polkagent"),
                    );
                responder.respond(response)
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
                        available_commands(),
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
                        Ok(stop_reason) => {
                            let _ = responder.respond(PromptResponse::new(stop_reason));
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
    if !request.cwd.is_absolute() {
        return Err(agent_client_protocol::Error::invalid_params()
            .data("session cwd must be an absolute path"));
    }
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

    let session_id = SessionId::new(format!("polkagent-{}", uuid::Uuid::now_v7()));
    let session = EditorSession {
        cwd: request.cwd,
        selected_agent,
        selected_model: default_model.map(str::to_owned),
        busy: false,
    };
    let response = NewSessionResponse::new(session_id.clone())
        .config_options(session_config_options(&session, &agents, &models));
    sessions.lock().await.insert(session_id.clone(), session);
    Ok((session_id, response))
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
        AGENT_CONFIG_ID if value == NO_AGENT_VALUE => session.selected_agent = None,
        AGENT_CONFIG_ID => {
            let Some(agent) = agents.iter().find(|agent| agent.id == value) else {
                return Err(agent_config_error(value));
            };
            session.selected_agent = Some(agent.id.clone());
        }
        MODEL_CONFIG_ID if value == INHERIT_MODEL_VALUE => session.selected_model = None,
        MODEL_CONFIG_ID => {
            let Some(model) = models.iter().find(|model| model.id == value) else {
                return Err(model_config_error(value));
            };
            session.selected_model = Some(model.id.clone());
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
    let mut agent_options = vec![SessionConfigSelectOption::new(
        NO_AGENT_VALUE,
        "No agent selected",
    )];
    agent_options.extend(
        agents
            .iter()
            .map(|agent| SessionConfigSelectOption::new(agent.id.clone(), agent.name.clone())),
    );
    let agent = SessionConfigOption::select(
        AGENT_CONFIG_ID,
        "Polkagent agent",
        session
            .selected_agent
            .clone()
            .unwrap_or_else(|| NO_AGENT_VALUE.to_owned()),
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
) -> Result<StopReason, agent_client_protocol::Error> {
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
    let outcome = match registry.parse(&prompt) {
        Ok(ParsedLine::Command(invocation)) => {
            handle_slash_command(
                invocation.command,
                &session_id,
                &snapshot,
                &sessions,
                backend.as_ref(),
                &registry,
            )
            .await?
        }
        Ok(ParsedLine::Prompt(prompt)) => {
            handle_regular_prompt(&prompt, &session_id, &sessions, backend.as_ref()).await?
        }
        Err(error) => BackendTurn::completed(format!(
            "Command error: {error}\n\n{}",
            help_text(&registry, None)
        )),
    };

    if !outcome.text.is_empty() {
        connection.send_notification(SessionNotification::new(
            session_id,
            SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
                TextContent::new(outcome.text),
            ))),
        ))?;
    }

    Ok(if outcome.cancelled {
        StopReason::Cancelled
    } else {
        StopReason::EndTurn
    })
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
    session_id: &SessionId,
    sessions: &Sessions,
    backend: &dyn AcpBackend,
) -> Result<BackendTurn, agent_client_protocol::Error> {
    let (cwd, agent, model) = {
        let mut all_sessions = sessions.lock().await;
        let current = all_sessions.get_mut(session_id).ok_or_else(|| {
            agent_client_protocol::Error::invalid_params()
                .data(format!("unknown session: {session_id}"))
        })?;
        let Some(agent) = current.selected_agent.clone() else {
            return Ok(BackendTurn::completed(
                "No agent is selected. Use /agents to list active agents, then /agent <name-or-id> to select one.",
            ));
        };
        if current.busy {
            return Err(agent_client_protocol::Error::invalid_request()
                .data(format!("session {session_id} already has an active prompt")));
        }
        current.busy = true;
        (current.cwd.clone(), agent, current.selected_model.clone())
    };
    let turn = call_backend(backend.prompt(
        session_id.0.as_ref(),
        &cwd,
        &agent,
        model.as_deref(),
        prompt.trim(),
    ))
    .await
    .map_err(|error| backend_protocol_error(&error));
    if let Some(current) = sessions.lock().await.get_mut(session_id) {
        current.busy = false;
    }
    turn
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
            Ok(BackendTurn::completed(help_text(registry, command)))
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
                current.selected_agent = Some(agent.id.clone());
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
            current.selected_model.clone_from(&selected);
            Ok(BackendTurn::completed(format!(
                "Selected model '{}'.",
                selected.as_deref().unwrap_or("agent default")
            )))
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
            "ACP supports only /cancel without arguments (or /stop) for the current editor prompt; run-scoped and all-session cancellation require durable interaction support.",
        )),
        unsupported => Ok(BackendTurn::completed(format!(
            "/{} is part of the shared Polkagent command registry but is not supported by ACP yet. This ACP slice does not expose durable interactions, run history, or approvals.",
            unsupported.name().as_str()
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

fn available_commands() -> Vec<AvailableCommand> {
    let registry = CommandRegistry::mvp();
    ACP_COMMANDS
        .iter()
        .filter_map(|name| registry.resolve(name.as_str()))
        .map(available_command)
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

fn help_text(registry: &CommandRegistry, command: Option<CommandName>) -> String {
    if let Some(command) = command {
        if !ACP_COMMANDS.contains(&command) {
            return format!(
                "/{} is part of the shared Polkagent command registry but is not supported by ACP yet. Durable interactions, run history, and approvals are not exposed by this ACP slice.",
                command.as_str()
            );
        }
        return registry.resolve(command.as_str()).map_or_else(
            || format!("No help is available for /{}.", command.as_str()),
            command_help,
        );
    }

    let commands = ACP_COMMANDS
        .iter()
        .filter_map(|name| registry.resolve(name.as_str()))
        .map(command_help)
        .collect::<Vec<_>>()
        .join("\n");
    format!("Polkagent ACP commands:\n{commands}")
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
    let detail = match error {
        BackendCallError::Rejected(error) => polkagent_telemetry::redact_string(&error.to_string()),
        BackendCallError::Panicked => BACKEND_PANIC_DETAIL.to_owned(),
    };
    agent_client_protocol::Error::internal_error().data(detail)
}

#[cfg(test)]
mod tests {
    use super::*;

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

        async fn prompt(
            &self,
            _session_id: &str,
            _cwd: &Path,
            _agent: &str,
            _model: Option<&str>,
            _prompt: &str,
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

        async fn prompt(
            &self,
            _session_id: &str,
            _cwd: &Path,
            _agent: &str,
            _model: Option<&str>,
            _prompt: &str,
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
    fn formats_empty_agent_list_actionably() {
        assert!(format_agents(&[]).contains("Create and start"));
    }

    #[test]
    fn advertises_supported_commands() {
        let commands = available_commands();
        let names = commands
            .iter()
            .map(|command| command.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            vec!["help", "status", "agents", "agent", "model", "cancel"]
        );
        let Some(agent) = commands.iter().find(|command| command.name == "agent") else {
            panic!("agent command was not advertised");
        };
        let Some(AvailableCommandInput::Unstructured(input)) = agent.input.as_ref() else {
            panic!("agent command should expose its registry input hint");
        };
        assert_eq!(input.hint, "<name-or-id>");
    }

    #[test]
    fn help_uses_registry_aliases_without_claiming_durable_acp_support() {
        let registry = CommandRegistry::mvp();
        let help = help_text(&registry, None);
        assert!(help.contains("/agent <name-or-id>"));
        assert!(help.contains("/use"));
        assert!(help.contains("/model [id]"));
        assert!(help.contains("/cancel"));

        let runs = help_text(&registry, Some(CommandName::Runs));
        assert!(runs.contains("shared Polkagent command registry"));
        assert!(runs.contains("not supported by ACP yet"));
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
