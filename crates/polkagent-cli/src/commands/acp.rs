//! `polkagent acp` — expose Polkagent as an ACP stdio agent.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use polkagent_chain_trait::ChainClient;
use polkagent_core::event::EventKind;
use polkagent_core::{AgentId, RunId};
use polkagent_event::{EventBus, EventRecorder};
use polkagent_service::AppService;
use polkagent_store_sqlite::{SqlitePool, SqliteRunStore};
use polkagent_surface_acp::{AcpBackend, AgentSummary, BackendError, BackendTurn, ServerConfig};
use tokio::sync::Mutex;

use crate::acp_diagnostics::AcpDiagnostics;
use crate::cli::AcpCmd;
use crate::commands::run::{
    build_agent_spec, build_chain_client, build_provider_registry, load_config_from_path,
    resolve_provider,
};

/// Start a protocol-safe ACP stdio server.
pub async fn run(
    cmd: &AcpCmd,
    pool: SqlitePool,
    config_path: Option<&Path>,
    diagnostics: AcpDiagnostics,
) -> Result<()> {
    diagnostics.record(
        "info",
        "acp.backend_initializing",
        "ACP runtime backend initialization started",
    );
    let backend = PolkagentAcpBackend::build(
        pool,
        config_path,
        cmd.provider.as_deref(),
        cmd.model.clone(),
        cmd.timeout,
        diagnostics.clone(),
    )
    .inspect_err(|_| {
        diagnostics.record(
            "error",
            "acp.backend_initialization_failed",
            "ACP runtime backend initialization failed",
        );
    })?;
    diagnostics.record(
        "info",
        "acp.server_ready",
        "ACP stdio server is ready for protocol input",
    );
    let result = polkagent_surface_acp::serve_stdio(
        Arc::new(backend),
        ServerConfig {
            default_agent: cmd.agent.clone(),
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
    pool: SqlitePool,
    app: Arc<AppService>,
    event_bus: EventBus,
    model_override: Option<String>,
    prompt_timeout: Option<Duration>,
    active_runs: Mutex<HashMap<String, RunId>>,
    diagnostics: AcpDiagnostics,
}

impl PolkagentAcpBackend {
    fn build(
        pool: SqlitePool,
        config_path: Option<&Path>,
        provider: Option<&str>,
        model_override: Option<String>,
        timeout_secs: u64,
        diagnostics: AcpDiagnostics,
    ) -> Result<Self> {
        let config = load_config_from_path(config_path).context("loading ACP configuration")?;
        let registry = build_provider_registry(&config);
        let (executor, provider_note) =
            resolve_provider(provider, model_override.as_deref(), &config, &registry)?;
        if let Some(note) = provider_note {
            eprintln!("{note}");
        }

        let event_bus = EventBus::with_default_capacity();
        let recorder = EventRecorder::new(Arc::new(pool.clone()), event_bus.clone());
        let chain_client: Arc<dyn ChainClient> = build_chain_client();
        let mut tool_registry = polkagent_tool::ToolRegistry::new();
        polkagent_tool_governance::register_governance_tools(
            &mut tool_registry,
            Arc::clone(&chain_client),
        );
        polkagent_tool_treasury::register_treasury_tools(
            &mut tool_registry,
            Arc::clone(&chain_client),
        );

        let app = AppService::builder()
            .with_config(config)
            .with_run_store(Arc::new(pool.clone()))
            .with_effect_store(Arc::new(pool.clone()))
            .with_event_bus(event_bus.clone())
            .with_event_recorder(recorder)
            .with_executor(executor)
            .with_provider_registry(registry)
            .with_chain_client(chain_client)
            .with_tool_registry(Arc::new(tool_registry))
            .build()
            .context("building ACP AppService")?;

        Ok(Self {
            pool,
            app: Arc::new(app),
            event_bus,
            model_override,
            prompt_timeout: (timeout_secs > 0).then(|| Duration::from_secs(timeout_secs)),
            active_runs: Mutex::new(HashMap::new()),
            diagnostics,
        })
    }

    fn active_agent(&self, selector: &str) -> Result<polkagent_store_sqlite::AgentRow> {
        let store = SqliteRunStore::new(self.pool.clone());
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

    async fn execute_prompt(
        &self,
        session_id: &str,
        selector: &str,
        prompt: &str,
    ) -> Result<BackendTurn> {
        self.diagnostics.record(
            "info",
            "acp.prompt_started",
            "Editor prompt execution started",
        );
        let agent = self.active_agent(selector)?;
        let agent_id: AgentId = agent
            .id
            .parse()
            .with_context(|| format!("invalid stored agent ID: {}", agent.id))?;
        let spec = build_agent_spec(
            agent_id,
            &agent.name,
            &agent.spec_json,
            self.model_override.clone(),
        );
        self.app
            .create_agent(spec)
            .context("registering ACP agent with AppService")?;

        let mut events = self.event_bus.subscribe();
        let run_id = self
            .app
            .start_run(agent_id, prompt)
            .await
            .context("starting ACP-backed run")?;
        self.active_runs
            .lock()
            .await
            .insert(session_id.to_owned(), run_id);

        let wait_for_result = async {
            let mut response = String::new();
            loop {
                let event = match events.recv().await {
                    Ok(event) => event,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        anyhow::bail!("run event stream closed before completion")
                    }
                };
                if event.run_id != run_id {
                    continue;
                }
                match event.kind {
                    EventKind::StreamingToken { text } => response.push_str(&text),
                    EventKind::RunCompleted { .. } => {
                        if response.is_empty() {
                            response.push_str("Run completed without text output.");
                        }
                        return Ok(BackendTurn::completed(response));
                    }
                    EventKind::RunFailed { reason } => anyhow::bail!("run failed: {reason}"),
                    EventKind::RunCancelled { reason } => {
                        return Ok(BackendTurn::cancelled(format!("Run cancelled: {reason}")));
                    }
                    EventKind::RunTimedOut => anyhow::bail!("run timed out"),
                    _ => {}
                }
            }
        };

        let result = if let Some(timeout) = self.prompt_timeout {
            if let Ok(result) = tokio::time::timeout(timeout, wait_for_result).await {
                result
            } else {
                let _ = self.app.timeout_run(run_id).await;
                Err(anyhow::anyhow!(
                    "editor prompt timed out after {} seconds",
                    timeout.as_secs()
                ))
            }
        } else {
            wait_for_result.await
        };
        self.active_runs.lock().await.remove(session_id);
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

#[async_trait]
impl AcpBackend for PolkagentAcpBackend {
    async fn list_agents(&self) -> Result<Vec<AgentSummary>, BackendError> {
        SqliteRunStore::new(self.pool.clone())
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

    async fn prompt(
        &self,
        session_id: &str,
        _cwd: &Path,
        agent: &str,
        prompt: &str,
    ) -> Result<BackendTurn, BackendError> {
        self.execute_prompt(session_id, agent, prompt)
            .await
            .map_err(|error| {
                self.diagnostics.record(
                    "error",
                    "acp.prompt_failed",
                    "Editor prompt execution failed",
                );
                BackendError::new(format!("{error:#}"))
            })
    }

    async fn cancel(&self, session_id: &str) -> Result<(), BackendError> {
        let run_id = self.active_runs.lock().await.get(session_id).copied();
        if let Some(run_id) = run_id {
            self.app.cancel_run(run_id).await.map_err(|error| {
                self.diagnostics.record(
                    "error",
                    "acp.cancel_failed",
                    "Cancelling the active editor prompt failed",
                );
                BackendError::new(format!("cancelling run: {error}"))
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
