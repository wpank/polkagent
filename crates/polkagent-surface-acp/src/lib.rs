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
    AgentCapabilities, AvailableCommand, AvailableCommandsUpdate, CancelNotification, ContentBlock,
    ContentChunk, Implementation, InitializeRequest, InitializeResponse, NewSessionRequest,
    NewSessionResponse, PromptRequest, PromptResponse, ResourceLink, SessionId,
    SessionNotification, SessionUpdate, StopReason, TextContent,
};
use agent_client_protocol::{Agent, Stdio};
use async_trait::async_trait;
use futures::FutureExt as _;
use tokio::sync::Mutex;

const BACKEND_PANIC_DETAIL: &str = "Polkagent's ACP backend panicked; the request was stopped";

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

    let outcome = if let Some(command) = SlashCommand::parse(&prompt) {
        handle_slash_command(command, &session_id, snapshot, &sessions, backend.as_ref()).await?
    } else if let Some(agent) = snapshot.selected_agent.as_deref() {
        {
            let mut all_sessions = sessions.lock().await;
            let current = all_sessions.get_mut(&session_id).ok_or_else(|| {
                agent_client_protocol::Error::invalid_params()
                    .data(format!("unknown session: {session_id}"))
            })?;
            if current.busy {
                return Err(agent_client_protocol::Error::invalid_request()
                    .data(format!("session {session_id} already has an active prompt")));
            }
            current.busy = true;
        }
        let turn = call_backend(backend.prompt(
            session_id.0.as_ref(),
            &snapshot.cwd,
            agent,
            prompt.trim(),
        ))
        .await
        .map_err(|error| backend_protocol_error(&error));
        if let Some(current) = sessions.lock().await.get_mut(&session_id) {
            current.busy = false;
        }
        turn?
    } else {
        BackendTurn::completed(
            "No agent is selected. Use /agents to list active agents, then /agent <name-or-id> to select one.",
        )
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

#[derive(Debug, Clone, Copy)]
struct SlashCommand<'a> {
    name: &'a str,
    argument: &'a str,
}

impl<'a> SlashCommand<'a> {
    fn parse(prompt: &'a str) -> Option<Self> {
        let trimmed = prompt.trim();
        let command = trimmed.strip_prefix('/')?;
        let (name, argument) = command
            .split_once(char::is_whitespace)
            .map_or((command, ""), |(name, argument)| (name, argument.trim()));
        Some(Self { name, argument })
    }
}

async fn handle_slash_command(
    command: SlashCommand<'_>,
    session_id: &SessionId,
    session: EditorSession,
    sessions: &Sessions,
    backend: &dyn AcpBackend,
) -> Result<BackendTurn, agent_client_protocol::Error> {
    match command.name {
        "help" => Ok(BackendTurn::completed(help_text())),
        "status" => Ok(BackendTurn::completed(format!(
            "Session: {session_id}\nWorkspace: {}\nAgent: {}",
            session.cwd.display(),
            session.selected_agent.as_deref().unwrap_or("not selected")
        ))),
        "agents" => {
            let agents = call_backend(backend.list_agents())
                .await
                .map_err(|error| backend_protocol_error(&error))?;
            Ok(BackendTurn::completed(format_agents(&agents)))
        }
        "agent" if command.argument.is_empty() => Ok(BackendTurn::completed(
            "Usage: /agent <name-or-id>. Use /agents to list active agents.",
        )),
        "agent" => match find_agent(backend, command.argument)
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
                "Active agent '{}' was not found. Use /agents to list choices.",
                command.argument
            ))),
        },
        unknown => Ok(BackendTurn::completed(format!(
            "Unknown command '/{unknown}'.\n\n{}",
            help_text()
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
    vec![
        AvailableCommand::new("help", "Show Polkagent ACP commands"),
        AvailableCommand::new(
            "status",
            "Show the current editor session and selected agent",
        ),
        AvailableCommand::new("agents", "List active Polkagent agents"),
        AvailableCommand::new("agent", "Select an active agent by name or ID"),
    ]
}

fn help_text() -> &'static str {
    "Polkagent ACP commands:\n/help — show this help\n/status — show session and agent selection\n/agents — list active agents\n/agent <name-or-id> — select the agent used for normal prompts"
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
    fn parses_slash_command_and_argument() {
        let parsed = SlashCommand::parse("  /agent research  ");
        assert!(parsed.is_some());
        let Some(command) = parsed else {
            return;
        };
        assert_eq!(command.name, "agent");
        assert_eq!(command.argument, "research");
    }

    #[test]
    fn formats_empty_agent_list_actionably() {
        assert!(format_agents(&[]).contains("Create and start"));
    }

    #[test]
    fn advertises_supported_commands() {
        let names = available_commands()
            .into_iter()
            .map(|command| command.name)
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["help", "status", "agents", "agent"]);
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
