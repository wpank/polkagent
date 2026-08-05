//! `polkagent acp` — expose Polkagent as an ACP stdio agent.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use polkagent_config::model_registry::{BuiltInModelCatalog, ModelCatalog as _};
use polkagent_core::{AgentId, ConversationId};
use polkagent_interaction::{
    ClientContext, ConfigOption, ConfigOptionValue, ConfigUpdate, CreateInteractionRequest,
    InteractionConfig, InteractionContent, InteractionError, InteractionErrorCode,
    InteractionEvent, InteractionOverrides, InteractionService as _, InteractionSummary,
    InteractionTarget, InteractionTurnId, PromptRequest as InteractionPromptRequest, StreamError,
    SubscriptionRequest, TranscriptRequest,
};
use polkagent_runtime::{
    AdapterPolicy, PolkagentRuntime, RuntimeError, RuntimeFactory, RuntimeOptions, WarningCode,
};
use polkagent_store_sqlite::SqliteRunStore;
use polkagent_surface_acp::{
    AcpBackend, AgentSummary, BackendError, BackendPromptUpdate, BackendSession,
    BackendTranscriptTurn, BackendTurn, ModelSummary, ServerConfig,
};
use tokio::sync::Mutex;

use crate::acp_diagnostics::AcpDiagnostics;
use crate::cli::AcpCmd;
use crate::commands::run::build_agent_spec;

/// Start a protocol-safe ACP stdio server.
pub async fn run(
    cmd: &AcpCmd,
    database_path: &Path,
    config_path: Option<&Path>,
    diagnostics: AcpDiagnostics,
) -> Result<()> {
    diagnostics.record(
        "info",
        "acp.backend_initializing",
        "ACP runtime backend initialization started",
    );
    let backend = PolkagentAcpBackend::build(cmd, database_path, config_path, diagnostics.clone())
        .await
        .inspect_err(|error| {
            if is_database_initialization_error(error) {
                diagnostics.record(
                    "error",
                    "acp.database_open_failed",
                    "ACP database initialization failed",
                );
            }
            diagnostics.record(
                "error",
                "acp.backend_initialization_failed",
                "ACP runtime backend initialization failed",
            );
        })?;
    diagnostics.record("info", "acp.runtime_ready", "ACP shared runtime is ready");
    diagnostics.record(
        "info",
        "acp.server_ready",
        "ACP stdio server is ready for protocol input",
    );
    let result = polkagent_surface_acp::serve_stdio(
        Arc::new(backend),
        ServerConfig {
            default_agent: cmd.agent.clone(),
            default_model: cmd.model.clone(),
        },
    )
    .await
    .map_err(|error| anyhow::anyhow!("ACP transport failed: {error}"));
    if result.is_ok() {
        diagnostics.record(
            "info",
            "acp.server_stopped",
            "ACP client disconnected and the stdio server stopped",
        );
    }
    result
}

struct PolkagentAcpBackend {
    runtime: PolkagentRuntime,
    startup_model: Option<String>,
    prompt_timeout: Option<Duration>,
    active_turns: Mutex<HashMap<String, InteractionTurnId>>,
    diagnostics: AcpDiagnostics,
}

impl PolkagentAcpBackend {
    async fn build(
        cmd: &AcpCmd,
        database_path: &Path,
        config_path: Option<&Path>,
        diagnostics: AcpDiagnostics,
    ) -> Result<Self> {
        let workdir = std::env::current_dir().context("resolving ACP runtime workdir")?;
        let mut options = RuntimeOptions::new(workdir);
        options.config_path = config_path.map(Path::to_path_buf);
        options.database_path = Some(database_path.to_path_buf());
        options.provider_override.clone_from(&cmd.provider);
        options.model_override.clone_from(&cmd.model);
        options.disable_harness = true;
        // Preserve the established local-first ACP behavior while keeping an
        // explicitly selected provider fail-closed in RuntimeFactory.
        options.adapter_policy = AdapterPolicy::AllowSimulated;

        let runtime = RuntimeFactory::build(options)
            .await
            .map_err(|error| explicit_provider_error(cmd, error))
            .context("building shared ACP runtime")?;
        if !runtime.readiness().operational {
            anyhow::bail!("shared ACP runtime is not operational");
        }
        report_executor_selection(&runtime);

        Ok(Self {
            runtime,
            startup_model: cmd.model.clone(),
            prompt_timeout: (cmd.timeout > 0).then(|| Duration::from_secs(cmd.timeout)),
            active_turns: Mutex::new(HashMap::new()),
            diagnostics,
        })
    }

    fn active_agent(&self, selector: &str) -> Result<polkagent_store_sqlite::AgentRow> {
        let store = SqliteRunStore::new(self.runtime.pool().clone());
        let agent = store
            .get_agent_by_name_or_id(selector)
            .with_context(|| format!("active agent not found: {selector}"))?;
        if agent.state != "active" {
            anyhow::bail!(
                "agent '{}' is in state '{}' and cannot accept editor prompts",
                agent.name,
                agent.state
            );
        }
        Ok(agent)
    }

    fn configured_models(&self) -> Result<Vec<ModelSummary>> {
        let mut models = BTreeSet::new();
        if let Some(model) = self
            .startup_model
            .as_deref()
            .filter(|model| !model.is_empty())
        {
            models.insert(model.to_owned());
        }
        for provider in self.runtime.app().provider_registry().list_providers() {
            for model in provider
                .models
                .into_iter()
                .filter(|model| !model.is_empty())
            {
                if !model.contains('/') {
                    models.insert(format!("{}/{model}", provider.id));
                }
                models.insert(model);
            }
        }
        for model in &self.runtime.config().models {
            if model.slug.is_empty() {
                continue;
            }
            if !model.slug.contains('/') {
                models.insert(format!("{}/{}", model.provider, model.slug));
            }
            models.insert(model.slug.clone());
        }
        let store = SqliteRunStore::new(self.runtime.pool().clone());
        for agent in store
            .list_agents(Some("active"), false)
            .context("reading active agents for model discovery")?
        {
            let agent_id: AgentId = agent
                .id
                .parse()
                .with_context(|| format!("invalid stored agent ID: {}", agent.id))?;
            let model = stored_agent_model(&agent.spec_json).unwrap_or_else(|| {
                build_agent_spec(agent_id, &agent.name, &agent.spec_json, None).model
            });
            if !model.is_empty() {
                models.insert(model);
            }
        }
        Ok(models
            .into_iter()
            .map(|model| ModelSummary {
                name: model.clone(),
                id: model,
            })
            .collect())
    }

    fn context_window(&self, model: &str) -> Option<u64> {
        self.runtime
            .config()
            .models
            .iter()
            .find(|candidate| {
                candidate.slug == model
                    || model
                        .strip_prefix(&candidate.provider)
                        .and_then(|suffix| suffix.strip_prefix('/'))
                        .is_some_and(|slug| slug == candidate.slug)
            })
            .and_then(|candidate| candidate.context_window)
            .or_else(|| {
                let catalog = BuiltInModelCatalog::new();
                catalog
                    .get(model)
                    .or_else(|| {
                        let (provider, slug) = model.split_once('/')?;
                        catalog
                            .get(slug)
                            .filter(|descriptor| descriptor.provider == provider)
                    })
                    .map(|descriptor| descriptor.context_window)
            })
    }

    fn agent_default_model(&self, target: &InteractionTarget) -> Result<Option<String>> {
        let InteractionTarget::Agent(agent_id) = target else {
            anyhow::bail!("ACP durable interaction does not have a single-agent target");
        };
        let agent = SqliteRunStore::new(self.runtime.pool().clone())
            .get_agent(&agent_id.to_string())
            .context("loading ACP interaction agent")?;
        Ok(stored_agent_model(&agent.spec_json))
    }

    async fn load_full_transcript(
        &self,
        conversation_id: ConversationId,
        turn_count: u32,
    ) -> Result<Vec<BackendTranscriptTurn>, InteractionError> {
        let mut transcript = Vec::new();
        let mut offset = 0_u32;
        while offset < turn_count {
            let limit = turn_count.saturating_sub(offset).min(1_000);
            let page = self
                .runtime
                .interactions()
                .load_transcript(TranscriptRequest {
                    conversation_id,
                    limit,
                    offset,
                })
                .await?;
            if page.is_empty() {
                break;
            }
            let page_len = u32::try_from(page.len()).map_err(|_| {
                InteractionError::new(
                    InteractionErrorCode::Internal,
                    "ACP transcript page exceeds supported range",
                )
            })?;
            transcript.extend(page.into_iter().map(|turn| BackendTranscriptTurn {
                user_text: turn.user_text,
                assistant_text: turn.assistant_text,
            }));
            offset = offset.saturating_add(page_len);
        }
        Ok(transcript)
    }

    #[allow(
        clippy::too_many_lines,
        reason = "the durable event loop keeps prompt activation, checkpoint replay, exact terminal projection, timeout, and cleanup in one auditable lifecycle"
    )]
    async fn execute_prompt(
        &self,
        session_id: &str,
        cwd: &Path,
        requested_turn_id: &str,
        prompt: &str,
        updates: tokio::sync::mpsc::Sender<BackendPromptUpdate>,
    ) -> Result<BackendTurn> {
        self.diagnostics.record(
            "info",
            "acp.prompt_started",
            "Editor prompt execution started",
        );
        let conversation_id = parse_conversation_id(session_id)?;
        let turn_id: InteractionTurnId = requested_turn_id
            .parse()
            .with_context(|| format!("invalid ACP turn ID: {requested_turn_id}"))?;
        let interaction = self
            .runtime
            .interactions()
            .load_interaction(conversation_id)
            .await
            .map_err(interaction_error)?;
        let effective_model = interaction.config.model.clone().or_else(|| {
            self.agent_default_model(&interaction.config.target)
                .ok()
                .flatten()
        });
        let context_window = effective_model
            .as_deref()
            .and_then(|model| self.context_window(model));
        let mut client_context =
            ClientContext::new(cwd.to_path_buf()).map_err(interaction_error)?;
        client_context.client_name = Some("acp".to_owned());
        client_context.client_session_id = Some(session_id.to_owned());
        let started = self
            .runtime
            .interactions()
            .prompt(InteractionPromptRequest {
                turn_id: Some(turn_id),
                conversation_id,
                content: vec![InteractionContent::Text {
                    text: prompt.to_owned(),
                }],
                config_overrides: InteractionOverrides::default(),
                client_context,
            })
            .await
            .map_err(interaction_error)?;
        let handle = started.handle.clone();
        let mut events = started.events;
        self.active_turns
            .lock()
            .await
            .insert(session_id.to_owned(), turn_id);

        let wait_for_result = async {
            let mut response = String::new();
            let mut checkpoint = handle.first_event_sequence.saturating_sub(1);
            loop {
                let envelope = match events.recv().await {
                    Ok(event) => event,
                    Err(StreamError::Lagged {
                        resume_after_sequence,
                        ..
                    }) => {
                        checkpoint = resume_after_sequence;
                        events = self
                            .runtime
                            .interactions()
                            .subscribe(SubscriptionRequest {
                                conversation_id,
                                turn_id: Some(turn_id),
                                after_sequence: Some(resume_after_sequence),
                                capacity: 256,
                            })
                            .await
                            .map_err(interaction_error)?;
                        continue;
                    }
                    Err(StreamError::Closed) => {
                        events = self
                            .runtime
                            .interactions()
                            .subscribe(SubscriptionRequest {
                                conversation_id,
                                turn_id: Some(turn_id),
                                after_sequence: Some(checkpoint),
                                capacity: 256,
                            })
                            .await
                            .map_err(interaction_error)?;
                        continue;
                    }
                    Err(StreamError::Backend(error)) => return Err(interaction_error(error)),
                };
                if envelope.turn_id != turn_id {
                    continue;
                }
                checkpoint = envelope.sequence;
                match envelope.event {
                    InteractionEvent::AgentMessageDelta { text, .. } => {
                        response.push_str(&text);
                        updates
                            .send(BackendPromptUpdate::TextDelta(text))
                            .await
                            .context("forwarding runtime text update to ACP surface")?;
                    }
                    InteractionEvent::TurnCompleted { result } => {
                        if let Some(size) = context_window {
                            let used = result.usage.total_tokens();
                            if used > 0 {
                                updates
                                    .send(BackendPromptUpdate::Usage { used, size })
                                    .await
                                    .context("forwarding runtime usage update to ACP surface")?;
                            }
                        }
                        return Ok(BackendTurn::completed(result.text).durable(
                            turn_id.to_string(),
                            result.run_ids.iter().map(ToString::to_string).collect(),
                            checkpoint,
                        ));
                    }
                    InteractionEvent::TurnFailed { error } => {
                        return Err(interaction_error(error));
                    }
                    InteractionEvent::TurnCancelled { reason } => {
                        if !response.is_empty() {
                            response.push_str("\n\n");
                        }
                        response.push_str("Run cancelled: ");
                        response.push_str(reason.as_deref().unwrap_or("cancelled"));
                        return Ok(BackendTurn::cancelled(response).durable(
                            turn_id.to_string(),
                            handle.run_ids.iter().map(ToString::to_string).collect(),
                            checkpoint,
                        ));
                    }
                    InteractionEvent::TurnTimedOut => anyhow::bail!("run timed out"),
                    _ => {}
                }
            }
        };

        let result = if let Some(timeout) = self.prompt_timeout {
            if let Ok(result) = tokio::time::timeout(timeout, wait_for_result).await {
                result
            } else {
                let _ = self.runtime.interactions().cancel_turn(turn_id).await;
                Err(anyhow::anyhow!(
                    "editor prompt timed out after {} seconds",
                    timeout.as_secs()
                ))
            }
        } else {
            wait_for_result.await
        };
        self.active_turns.lock().await.remove(session_id);
        if let Ok(turn) = &result {
            let (event, detail) = if turn.cancelled {
                (
                    "acp.prompt_cancelled",
                    "Editor prompt execution was cancelled",
                )
            } else {
                ("acp.prompt_completed", "Editor prompt execution completed")
            };
            self.diagnostics.record("info", event, detail);
        }
        result
    }
}

fn stored_agent_model(spec_json: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(spec_json)
        .ok()?
        .get("model")?
        .as_str()
        .filter(|model| !model.is_empty())
        .map(str::to_owned)
}

fn parse_conversation_id(value: &str) -> Result<ConversationId> {
    value
        .parse()
        .with_context(|| format!("invalid durable ACP session ID: {value}"))
}

fn backend_session(
    summary: InteractionSummary,
    transcript: Vec<BackendTranscriptTurn>,
) -> Result<BackendSession> {
    let InteractionTarget::Agent(agent_id) = summary.config.target else {
        anyhow::bail!("ACP interaction does not resolve to one active agent");
    };
    Ok(BackendSession {
        session_id: summary.conversation_id.to_string(),
        selected_agent: agent_id.to_string(),
        selected_model: summary.config.model,
        transcript,
    })
}

fn interaction_error(error: InteractionError) -> anyhow::Error {
    anyhow::Error::new(error)
}

fn backend_error(error: anyhow::Error) -> BackendError {
    let message = format!("{error:#}");
    drop(error);
    BackendError::new(message)
}

fn invalid_backend_error(error: anyhow::Error) -> BackendError {
    let message = format!("{error:#}");
    drop(error);
    BackendError::invalid_request(message)
}

fn execution_backend_error(error: anyhow::Error) -> BackendError {
    let interaction = error.downcast_ref::<InteractionError>().cloned();
    if let Some(interaction) = interaction {
        drop(error);
        interaction_backend_error(interaction)
    } else {
        backend_error(error)
    }
}

fn interaction_backend_error(error: InteractionError) -> BackendError {
    let code = error.code;
    let message = error.to_string();
    drop(error);
    match code {
        InteractionErrorCode::InvalidRequest | InteractionErrorCode::InvalidConfig => {
            BackendError::invalid_request(message)
        }
        InteractionErrorCode::NotFound => BackendError::not_found(message),
        InteractionErrorCode::Conflict | InteractionErrorCode::Busy => {
            BackendError::conflict(message)
        }
        InteractionErrorCode::Unsupported | InteractionErrorCode::PermissionDenied => {
            BackendError::unsupported(message)
        }
        InteractionErrorCode::Unavailable | InteractionErrorCode::Internal => {
            BackendError::new(message)
        }
    }
}

#[async_trait]
impl AcpBackend for PolkagentAcpBackend {
    async fn list_agents(&self) -> Result<Vec<AgentSummary>, BackendError> {
        SqliteRunStore::new(self.runtime.pool().clone())
            .list_agents(Some("active"), false)
            .map(|agents| {
                agents
                    .into_iter()
                    .map(|agent| AgentSummary {
                        id: agent.id,
                        name: agent.name,
                        state: agent.state,
                    })
                    .collect()
            })
            .map_err(|error| {
                self.diagnostics.record(
                    "error",
                    "acp.agent_list_failed",
                    "Listing active agents failed",
                );
                BackendError::new(format!("listing active agents: {error}"))
            })
    }

    async fn list_models(&self) -> Result<Vec<ModelSummary>, BackendError> {
        self.configured_models().map_err(|error| {
            self.diagnostics.record(
                "error",
                "acp.model_list_failed",
                "Listing configured models failed",
            );
            BackendError::new(format!("listing configured models: {error:#}"))
        })
    }

    async fn new_session(
        &self,
        cwd: &Path,
        agent: Option<&str>,
        model: Option<&str>,
    ) -> Result<BackendSession, BackendError> {
        let target = match agent {
            Some(selector) => {
                let agent = self.active_agent(selector).map_err(invalid_backend_error)?;
                let agent_id = agent
                    .id
                    .parse()
                    .with_context(|| format!("invalid stored agent ID: {}", agent.id))
                    .map_err(invalid_backend_error)?;
                InteractionTarget::Agent(agent_id)
            }
            None => InteractionTarget::Auto,
        };
        let mut config = InteractionConfig::new(target);
        config.model = model.map(str::to_owned);
        let mut context =
            ClientContext::new(cwd.to_path_buf()).map_err(interaction_backend_error)?;
        context.client_name = Some("acp".to_owned());
        let summary = self
            .runtime
            .interactions()
            .new_interaction(CreateInteractionRequest {
                title: Some("ACP editor session".to_owned()),
                config,
                client_context: context,
            })
            .await
            .map_err(interaction_backend_error)?;
        backend_session(summary, Vec::new()).map_err(backend_error)
    }

    async fn load_session(
        &self,
        session_id: &str,
        _cwd: &Path,
        include_transcript: bool,
    ) -> Result<BackendSession, BackendError> {
        let conversation_id = parse_conversation_id(session_id).map_err(invalid_backend_error)?;
        let summary = self
            .runtime
            .interactions()
            .load_interaction(conversation_id)
            .await
            .map_err(interaction_backend_error)?;
        let transcript = if include_transcript {
            self.load_full_transcript(conversation_id, summary.turn_count)
                .await
                .map_err(interaction_backend_error)?
        } else {
            Vec::new()
        };
        backend_session(summary, transcript).map_err(backend_error)
    }

    async fn set_agent(
        &self,
        session_id: &str,
        agent_id: &str,
    ) -> Result<BackendSession, BackendError> {
        let conversation_id = parse_conversation_id(session_id).map_err(invalid_backend_error)?;
        let agent_id: AgentId = agent_id
            .parse()
            .with_context(|| format!("invalid ACP agent ID: {agent_id}"))
            .map_err(invalid_backend_error)?;
        self.runtime
            .interactions()
            .set_config_option(
                conversation_id,
                ConfigUpdate {
                    option: ConfigOption::Target,
                    value: ConfigOptionValue::Target(InteractionTarget::Agent(agent_id)),
                },
            )
            .await
            .map_err(interaction_backend_error)?;
        let summary = self
            .runtime
            .interactions()
            .load_interaction(conversation_id)
            .await
            .map_err(interaction_backend_error)?;
        backend_session(summary, Vec::new()).map_err(backend_error)
    }

    async fn set_model(
        &self,
        session_id: &str,
        model: Option<&str>,
    ) -> Result<BackendSession, BackendError> {
        let conversation_id = parse_conversation_id(session_id).map_err(invalid_backend_error)?;
        self.runtime
            .interactions()
            .set_config_option(
                conversation_id,
                ConfigUpdate {
                    option: ConfigOption::Model,
                    value: ConfigOptionValue::Model(model.map(str::to_owned)),
                },
            )
            .await
            .map_err(interaction_backend_error)?;
        let summary = self
            .runtime
            .interactions()
            .load_interaction(conversation_id)
            .await
            .map_err(interaction_backend_error)?;
        backend_session(summary, Vec::new()).map_err(backend_error)
    }

    async fn prompt(
        &self,
        session_id: &str,
        cwd: &Path,
        turn_id: &str,
        prompt: &str,
        updates: tokio::sync::mpsc::Sender<BackendPromptUpdate>,
    ) -> Result<BackendTurn, BackendError> {
        self.execute_prompt(session_id, cwd, turn_id, prompt, updates)
            .await
            .map_err(|error| {
                self.diagnostics.record(
                    "error",
                    "acp.prompt_failed",
                    "Editor prompt execution failed",
                );
                execution_backend_error(error)
            })
    }

    async fn cancel(&self, session_id: &str) -> Result<(), BackendError> {
        let turn_id = self.active_turns.lock().await.get(session_id).copied();
        if let Some(turn_id) = turn_id {
            self.runtime
                .interactions()
                .cancel_turn(turn_id)
                .await
                .map_err(|error| {
                    self.diagnostics.record(
                        "error",
                        "acp.cancel_failed",
                        "Cancelling the active editor prompt failed",
                    );
                    interaction_backend_error(error)
                })?;
            self.diagnostics.record(
                "info",
                "acp.cancel_requested",
                "Cancellation was requested for the active editor prompt",
            );
        }
        Ok(())
    }
}

fn explicit_provider_error(cmd: &AcpCmd, error: RuntimeError) -> anyhow::Error {
    if let RuntimeError::ProviderUnavailable { provider, reason } = error {
        if cmd.provider.as_deref() == Some(provider.as_str()) {
            return anyhow::anyhow!(
                "Provider '{provider}' was explicitly requested (--provider flag) but could not be found: {reason}"
            );
        }
        return anyhow::Error::new(RuntimeError::ProviderUnavailable { provider, reason });
    }
    anyhow::Error::new(error)
}

fn is_database_initialization_error(error: &anyhow::Error) -> bool {
    error.downcast_ref::<RuntimeError>().is_some_and(|error| {
        matches!(
            error,
            RuntimeError::Database { .. }
                | RuntimeError::DatabaseDirectory { .. }
                | RuntimeError::InMemoryDatabase
        )
    })
}

fn report_executor_selection(runtime: &PolkagentRuntime) {
    if runtime
        .readiness()
        .warnings
        .iter()
        .any(|warning| warning.code == WarningCode::SimulatedExecutor)
    {
        eprintln!("No API key or local model configured. Using simulated responses.");
    } else {
        eprintln!("{}", runtime.readiness().executor.detail);
    }
}
