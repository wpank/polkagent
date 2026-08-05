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
    ResourceLink, SessionId, SessionNotification, SessionUpdate, StopReason, TextContent,
    UnstructuredCommandInput,
};
use agent_client_protocol::{Agent, Stdio};
use async_trait::async_trait;
use futures::FutureExt as _;
use polkagent_interaction::{
    CancelTarget, CommandName, CommandRegistry, CommandSpec, InteractionCommand, ParsedLine,
};
use tokio::sync::Mutex;

const BACKEND_PANIC_DETAIL: &str = "Polkagent's ACP backend panicked; the request was stopped";
const ACP_COMMANDS: [CommandName; 5] = [
    CommandName::Help,
    CommandName::Status,
    CommandName::Agents,
    CommandName::Agent,
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

    /// Execute one prompt through Polkagent's orchestration runtime.
    async fn prompt(
        &self,
        session_id: &str,
        cwd: &Path,
        agent: &str,
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
}

#[derive(Debug, Clone)]
struct EditorSession {
    cwd: PathBuf,
    selected_agent: Option<String>,
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
    let new_backend = Arc::clone(&backend);
    let prompt_backend = Arc::clone(&backend);
    let cancel_backend = Arc::clone(&backend);
    let default_agent = config.default_agent;

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
                if !request.cwd.is_absolute() {
                    return responder.respond_with_error(
                        agent_client_protocol::Error::invalid_params()
                            .data("session cwd must be an absolute path"),
                    );
                }
                if !request.mcp_servers.is_empty() {
                    return responder.respond_with_error(
                        agent_client_protocol::Error::invalid_params().data(
                            "session MCP servers are not supported by this Polkagent ACP slice yet",
                        ),
                    );
                }
                if request
                    .additional_directories
                    .iter()
                    .any(|directory| !directory.is_absolute())
                {
                    return responder.respond_with_error(
                        agent_client_protocol::Error::invalid_params()
                            .data("all additional session directories must be absolute paths"),
                    );
                }
                if !request.additional_directories.is_empty() {
                    return responder.respond_with_error(
                        agent_client_protocol::Error::invalid_params().data(
                            "additional session directories are not supported by this Polkagent ACP slice yet",
                        ),
                    );
                }

                if let Some(selector) = default_agent.as_deref() {
                    match find_agent(new_backend.as_ref(), selector).await {
                        Ok(Some(_)) => {}
                        Ok(None) => {
                            return responder.respond_with_error(
                                agent_client_protocol::Error::invalid_params().data(format!(
                                    "configured ACP agent '{selector}' was not found or is not active"
                                )),
                            );
                        }
                        Err(error) => {
                            return responder.respond_with_error(backend_protocol_error(&error));
                        }
                    }
                }

                let session_id = SessionId::new(format!("polkagent-{}", uuid::Uuid::now_v7()));
                new_sessions.lock().await.insert(
                    session_id.clone(),
                    EditorSession {
                        cwd: request.cwd,
                        selected_agent: default_agent.clone(),
                        busy: false,
                    },
                );

                responder.respond(NewSessionResponse::new(session_id.clone()))?;
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
            handle_regular_prompt(&prompt, &session_id, &snapshot, &sessions, backend.as_ref())
                .await?
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
    session: &EditorSession,
    sessions: &Sessions,
    backend: &dyn AcpBackend,
) -> Result<BackendTurn, agent_client_protocol::Error> {
    let Some(agent) = session.selected_agent.as_deref() else {
        return Ok(BackendTurn::completed(
            "No agent is selected. Use /agents to list active agents, then /agent <name-or-id> to select one.",
        ));
    };
    {
        let mut all_sessions = sessions.lock().await;
        let current = all_sessions.get_mut(session_id).ok_or_else(|| {
            agent_client_protocol::Error::invalid_params()
                .data(format!("unknown session: {session_id}"))
        })?;
        if current.busy {
            return Err(agent_client_protocol::Error::invalid_request()
                .data(format!("session {session_id} already has an active prompt")));
        }
        current.busy = true;
    }
    let turn =
        call_backend(backend.prompt(session_id.0.as_ref(), &session.cwd, agent, prompt.trim()))
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
            "Session: {session_id}\nWorkspace: {}\nAgent: {}\nActive prompt: {}",
            session.cwd.display(),
            session.selected_agent.as_deref().unwrap_or("not selected"),
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
            Some(agent) => {
                if let Some(current) = sessions.lock().await.get_mut(session_id) {
                    current.selected_agent = Some(agent.id.clone());
                }
                Ok(BackendTurn::completed(format!(
                    "Selected agent '{}' ({}).",
                    agent.name, agent.id
                )))
            }
            None => Ok(BackendTurn::completed(format!(
                "Active agent '{selector}' was not found. Use /agents to list choices."
            ))),
        },
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
            "/{} is part of the shared Polkagent command registry but is not supported by ACP yet. This ACP slice does not expose durable interactions, run history, approvals, or model changes.",
            unsupported.name().as_str()
        ))),
    }
}

async fn find_agent(
    backend: &dyn AcpBackend,
    selector: &str,
) -> Result<Option<AgentSummary>, BackendCallError> {
    Ok(call_backend(backend.list_agents())
        .await?
        .into_iter()
        .find(|agent| agent.id == selector || agent.name == selector))
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
                "/{} is part of the shared Polkagent command registry but is not supported by ACP yet. Durable interactions, run history, approvals, and model changes are not exposed by this ACP slice.",
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

    #[async_trait]
    impl AcpBackend for PanickingBackend {
        async fn list_agents(&self) -> Result<Vec<AgentSummary>, BackendError> {
            panic!("synthetic backend panic payload");
        }

        async fn prompt(
            &self,
            _session_id: &str,
            _cwd: &Path,
            _agent: &str,
            _prompt: &str,
        ) -> Result<BackendTurn, BackendError> {
            panic!("synthetic backend panic payload");
        }

        async fn cancel(&self, _session_id: &str) -> Result<(), BackendError> {
            panic!("synthetic backend panic payload");
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
        assert_eq!(names, vec!["help", "status", "agents", "agent", "cancel"]);
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
}
