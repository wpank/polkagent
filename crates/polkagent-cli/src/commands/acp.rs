//! `polkagent acp` — expose Polkagent as an ACP stdio agent.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use polkagent_core::event::EventKind;
use polkagent_core::{AgentId, RunId};
use polkagent_runtime::{
    AdapterPolicy, PolkagentRuntime, RuntimeError, RuntimeFactory, RuntimeOptions, WarningCode,
};
use polkagent_store_sqlite::SqliteRunStore;
use polkagent_surface_acp::{AcpBackend, AgentSummary, BackendError, BackendTurn, ServerConfig};
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
    model_override: Option<String>,
    prompt_timeout: Option<Duration>,
    active_runs: Mutex<HashMap<String, RunId>>,
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
            model_override: cmd.model.clone(),
            prompt_timeout: (cmd.timeout > 0).then(|| Duration::from_secs(cmd.timeout)),
            active_runs: Mutex::new(HashMap::new()),
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
        self.runtime
            .app()
            .create_agent(spec)
            .context("registering ACP agent with AppService")?;

        let mut events = self.runtime.subscribe_events();
        let run_id = self
            .runtime
            .app()
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
                let _ = self.runtime.app().timeout_run(run_id).await;
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
            self.runtime
                .app()
                .cancel_run(run_id)
                .await
                .map_err(|error| {
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
