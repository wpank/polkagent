//! `polkagent run` — submit a run and stream its output.
//!
//! When no daemon is running the one-shot command builds a shared production
//! runtime, starts the run, subscribes to its event bus, and streams events to
//! stdout until a terminal event arrives or the timeout expires. Interactive
//! surfaces retain the same process-wide runtime instead of composing a new
//! service for each prompt.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use tracing::info;

use polkagent_core::event::EventKind;
use polkagent_core::{AgentId, AgentSpec, RunId};
use polkagent_event::EventReceiver;
use polkagent_runtime::{
    AdapterPolicy, ComponentState, RuntimeFactory, RuntimeOptions, RuntimeReadiness, WarningCode,
};
use polkagent_service::AppService;
use polkagent_store_sqlite::{SqlitePool, SqliteRunStore};

use crate::cli::RunCmd;
use crate::commands::run_printer::RunPrinter;
use crate::tui::theme::Theme;

/// A started run plus the live resources needed by foreground surfaces.
///
/// The one-shot command obtains these resources from `RuntimeFactory`.
pub(crate) struct StartedRun {
    pub(crate) service: Arc<AppService>,
    pub(crate) events: EventReceiver,
    pub(crate) run_id: RunId,
    pub(crate) agent_id: String,
    pub(crate) agent_name: String,
    pub(crate) agent_model: String,
    pub(crate) notes: Vec<String>,
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Execute the `run` subcommand.
///
/// Builds the shared production runtime (no daemon required), starts the run,
/// then subscribes to its event bus and streams events to stdout until the run
/// reaches a terminal state or the timeout expires.
#[allow(
    clippy::too_many_lines,
    reason = "the foreground run lifecycle is kept contiguous so setup, event streaming, timeout, and cleanup cannot diverge"
)]
pub async fn run(
    cmd: &RunCmd,
    pool: &SqlitePool,
    config_path: Option<&Path>,
    dry_run: bool,
) -> Result<()> {
    // --dry-run: print what would happen and exit without starting a real run.
    if dry_run {
        println!("Dry run — no run will be started.");
        println!("  Agent:   {}", cmd.agent_id);
        println!("  Prompt:  {}", cmd.prompt);
        if let Some(ref provider) = cmd.provider {
            println!("  Provider: {provider}");
        }
        if let Some(ref model) = cmd.model {
            println!("  Model:   {model}");
        }
        if let Some(ref harness) = cmd.harness {
            println!("  Harness: {harness}");
        } else {
            println!("  Harness: (auto-detected or executor-only)");
        }
        return Ok(());
    }

    let started = start_one_shot_run(cmd, pool, config_path).await?;
    for note in &started.notes {
        eprintln!("{note}");
    }
    let StartedRun {
        service: app_service,
        events: mut event_rx,
        run_id,
        agent_id,
        agent_name,
        agent_model,
        ..
    } = started;

    if cmd.json {
        let out = serde_json::json!({
            "run_id":   run_id.to_string(),
            "agent_id": agent_id,
            "state":    "running",
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
    }

    // Accumulate streaming tokens in JSON mode (the printer handles this for
    // styled output; in JSON mode we collect manually so we can emit them in
    // the completion blob).
    let mut json_response = String::new();

    // Build styled printer for non-JSON output.
    let theme = Theme::from_env();
    let mut printer = RunPrinter::new(theme, cmd.stream);
    let mut stdout = std::io::stdout();

    if !cmd.json {
        printer.print_header(&mut stdout, &run_id, &agent_name, &agent_model)?;
    }

    // -----------------------------------------------------------------------
    // Timeout
    // -----------------------------------------------------------------------
    let timeout_duration = if cmd.timeout == 0 {
        None
    } else {
        Some(Duration::from_secs(cmd.timeout))
    };

    // -----------------------------------------------------------------------
    // Ctrl-C / SIGINT handler
    // -----------------------------------------------------------------------
    let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
    {
        let cancelled_clone = Arc::clone(&cancelled);
        let run_id_copy = run_id;
        let app_service_for_cancel = Arc::clone(&app_service);
        tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                cancelled_clone.store(true, std::sync::atomic::Ordering::SeqCst);
                // Best-effort cancel; ignore errors.
                let _ = app_service_for_cancel.cancel_run(run_id_copy).await;
            }
        });
    }

    // -----------------------------------------------------------------------
    // Event streaming loop
    // -----------------------------------------------------------------------
    // `stream` is true by default; --no-stream sets it to false.

    let result: Result<()> = async {
        // Set up the timeout future — either a real sleep or one that never
        // fires (pending forever).
        let timed_out = async {
            if let Some(dur) = timeout_duration {
                tokio::time::sleep(dur).await;
            } else {
                std::future::pending::<()>().await;
            }
        };
        tokio::pin!(timed_out);

        loop {
            // Check for cancellation set by the Ctrl-C handler.
            if cancelled.load(std::sync::atomic::Ordering::SeqCst) {
                println!("\nRun cancelled");
                return Ok(());
            }

            let event = tokio::select! {
                () = &mut timed_out => {
                    eprintln!(
                        "\nRun timed out after {} seconds",
                        cmd.timeout
                    );
                    let _ = app_service.timeout_run(run_id).await;
                    return Err(anyhow::anyhow!(
                        "run timed out after {} seconds",
                        cmd.timeout
                    ));
                }
                recv_result = event_rx.recv() => {
                    match recv_result {
                        Ok(ev) => ev,
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            eprintln!(
                                "[warning: {n} events dropped due to slow consumer]"
                            );
                            continue;
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            // Bus closed — execution finished.
                            break;
                        }
                    }
                }
            };

            // Only process events that belong to our run.
            if event.run_id != run_id {
                continue;
            }

            if cmd.json {
                // JSON mode: accumulate streaming tokens and emit a single
                // completion JSON blob when the run reaches a terminal state.
                match &event.kind {
                    EventKind::StreamingToken { text } => {
                        json_response.push_str(text);
                    }
                    EventKind::RunCompleted {
                        input_tokens,
                        output_tokens,
                        ..
                    } => {
                        let out = serde_json::json!({
                            "ok":           true,
                            "run_id":       run_id.to_string(),
                            "agent_id":     agent_id,
                            "response":     json_response,
                            "input_tokens": input_tokens,
                            "output_tokens": output_tokens,
                        });
                        println!("{}", serde_json::to_string_pretty(&out)?);
                        break;
                    }
                    EventKind::RunFailed { reason } => {
                        let out = serde_json::json!({
                            "ok":     false,
                            "run_id": run_id.to_string(),
                            "error":  reason,
                        });
                        println!("{}", serde_json::to_string_pretty(&out)?);
                        break;
                    }
                    EventKind::RunCancelled { reason } => {
                        let out = serde_json::json!({
                            "ok":     false,
                            "run_id": run_id.to_string(),
                            "error":  format!("cancelled: {reason}"),
                        });
                        println!("{}", serde_json::to_string_pretty(&out)?);
                        break;
                    }
                    EventKind::RunTimedOut => {
                        let out = serde_json::json!({
                            "ok":     false,
                            "run_id": run_id.to_string(),
                            "error":  "run timed out",
                        });
                        println!("{}", serde_json::to_string_pretty(&out)?);
                        break;
                    }
                    _ => {}
                }
            } else {
                let flow = printer.handle_event(&mut stdout, &event);
                if flow.is_break() {
                    break;
                }
            }
        }

        Ok(())
    }
    .await;

    result
}

/// Start a one-shot command through the shared production runtime.
async fn start_one_shot_run(
    cmd: &RunCmd,
    pool: &SqlitePool,
    config_path: Option<&Path>,
) -> Result<StartedRun> {
    validate_prompt(&cmd.prompt)?;
    let agent = find_runnable_agent(pool, &cmd.agent_id)?;
    let typed_agent_id: AgentId = agent
        .id
        .parse()
        .with_context(|| format!("invalid agent ID in database: {}", agent.id))?;
    let agent_spec = build_agent_spec(
        typed_agent_id,
        &agent.name,
        &agent.spec_json,
        cmd.model.clone(),
    );
    let agent_model = agent_spec.model;

    let runtime_options = one_shot_runtime_options(cmd, pool, config_path)?;
    let runtime = RuntimeFactory::build(runtime_options)
        .await
        .context("building shared Polkagent runtime")?;
    let notes = one_shot_runtime_notes(runtime.readiness(), cmd.no_harness);
    let events = runtime.subscribe_events();
    let service = Arc::clone(runtime.app());
    let run_id = service
        .start_run(typed_agent_id, &cmd.prompt)
        .await
        .context("starting run")?;

    info!(%run_id, agent_id = %agent.id, "run started through shared runtime");
    Ok(StartedRun {
        service,
        events,
        run_id,
        agent_id: agent.id,
        agent_name: agent.name,
        agent_model,
        notes,
    })
}

fn one_shot_runtime_options(
    cmd: &RunCmd,
    pool: &SqlitePool,
    config_path: Option<&Path>,
) -> Result<RuntimeOptions> {
    let workdir = std::env::current_dir().context("resolving current working directory")?;
    let mut options = RuntimeOptions::new(workdir);
    options.config_path = config_path.map(Path::to_path_buf);
    // `main` still owns the command-family pool until all database commands
    // migrate. Pin the runtime to the already-resolved file so config and env
    // path resolution cannot select a different database mid-command.
    options.database_path = Some(pool.path().to_path_buf());
    options.provider_override.clone_from(&cmd.provider);
    options.model_override.clone_from(&cmd.model);
    options.harness_override.clone_from(&cmd.harness);
    options.disable_harness = cmd.no_harness;
    // Preserve the established local-first CLI behavior: without credentials
    // the command executes a deterministic simulated response and reports it.
    options.adapter_policy = AdapterPolicy::AllowSimulated;
    Ok(options)
}

fn one_shot_runtime_notes(readiness: &RuntimeReadiness, no_harness: bool) -> Vec<String> {
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

    if !no_harness {
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
    }
    notes
}

fn validate_prompt(prompt: &str) -> Result<()> {
    if prompt.trim().is_empty() {
        anyhow::bail!("prompt cannot be empty");
    }
    Ok(())
}

fn find_runnable_agent(
    pool: &SqlitePool,
    agent_reference: &str,
) -> Result<polkagent_store_sqlite::AgentRow> {
    let store = SqliteRunStore::new(pool.clone());
    let agent = store
        .get_agent_by_name_or_id(agent_reference)
        .map_err(|_| anyhow::anyhow!("Agent not found or not active: {agent_reference}"))?;

    if matches!(
        agent.state.as_str(),
        "archived" | "deactivated" | "paused" | "stopped" | "configured"
    ) {
        anyhow::bail!(
            "Agent '{}' is in state '{}' and cannot accept runs. \
             Only agents in the 'active' state can be run.",
            agent_reference,
            agent.state
        );
    }
    Ok(agent)
}

// ---------------------------------------------------------------------------
// AgentSpec reconstruction
// ---------------------------------------------------------------------------

/// Build an [`AgentSpec`] for use with [`AppService::create_agent`].
///
/// Tries to deserialise the stored `spec_json`. Falls back to a minimal spec
/// constructed from the agent's name. CLI model override always wins.
pub(crate) fn build_agent_spec(
    agent_id: AgentId,
    name: &str,
    spec_json: &str,
    model_override: Option<String>,
) -> AgentSpec {
    let mut spec: AgentSpec = serde_json::from_str(spec_json)
        .unwrap_or_else(|_| AgentSpec::new(agent_id, name, "fake/default-model"));

    // Force the ID to match what is stored in the database.
    spec.id = agent_id;

    // Apply CLI model override if provided.
    if let Some(model) = model_override {
        spec.model = model;
    }

    spec
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(unsafe_code)]
mod tests {
    use super::*;

    #[test]
    fn build_agent_spec_from_valid_json() {
        let id = AgentId::new();
        let json = serde_json::json!({
            "id": id.to_string(),
            "name": "test-agent",
            "model": "anthropic/claude-opus-4-6",
            "description": null,
            "tools": [],
            "system_prompt": null,
            "autonomy_level": "supervised",
            "created_at": "2024-01-01T00:00:00Z",
            "updated_at": "2024-01-01T00:00:00Z",
        })
        .to_string();

        let spec = build_agent_spec(id, "test-agent", &json, None);
        assert_eq!(spec.id, id);
        assert_eq!(spec.model, "anthropic/claude-opus-4-6");
    }

    #[test]
    fn build_agent_spec_fallback_on_bad_json() {
        let id = AgentId::new();
        let spec = build_agent_spec(id, "test-agent", "{invalid", None);
        assert_eq!(spec.id, id);
        assert_eq!(spec.name, "test-agent");
    }

    #[test]
    fn build_agent_spec_model_override_wins() {
        let id = AgentId::new();
        let json = serde_json::json!({
            "id": id.to_string(),
            "name": "test-agent",
            "model": "anthropic/claude-opus-4-6",
            "description": null,
            "tools": [],
            "system_prompt": null,
            "autonomy_level": "supervised",
            "created_at": "2024-01-01T00:00:00Z",
            "updated_at": "2024-01-01T00:00:00Z",
        })
        .to_string();

        let spec = build_agent_spec(id, "test-agent", &json, Some("openai/gpt-4o".to_string()));
        assert_eq!(spec.model, "openai/gpt-4o");
    }

    /// Mutex to serialize tests that mutate process-wide environment variables.
    /// Without this, parallel tests race on `std::env::set_var`/`remove_var`.
    static ENV_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Helper: save, clear, and restore executor-related env vars so that
    /// individual tests are isolated from the host environment.
    struct EnvGuard {
        vars: Vec<(&'static str, Option<String>)>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl EnvGuard {
        const KEYS: &'static [&'static str] = &[
            "ANTHROPIC_API_KEY",
            "OPENAI_API_KEY",
            "GEMINI_API_KEY",
            "OPENROUTER_API_KEY",
            "PERPLEXITY_API_KEY",
            "CEREBRAS_API_KEY",
            "OLLAMA_URL",
            "OLLAMA_MODEL",
        ];

        fn new() -> Self {
            let lock = ENV_MUTEX
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let vars: Vec<_> = Self::KEYS
                .iter()
                .map(|&k| (k, std::env::var(k).ok()))
                .collect();
            for &k in Self::KEYS {
                // SAFETY: tests are serialised by ENV_MUTEX so no concurrent mutation.
                unsafe { std::env::remove_var(k) };
            }
            Self { vars, _lock: lock }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (k, v) in &self.vars {
                // SAFETY: tests are serialised by ENV_MUTEX so no concurrent mutation.
                match v {
                    Some(val) => unsafe { std::env::set_var(k, val) },
                    None => unsafe { std::env::remove_var(k) },
                }
            }
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn one_shot_run_uses_shared_runtime_and_durable_stores() {
        let _guard = EnvGuard::new();
        let temp = tempfile::TempDir::new().expect("tempdir");
        let database_path = temp.path().join("runtime.db");
        let config_path = temp.path().join("polkagent.toml");
        std::fs::write(&config_path, "").expect("write config");

        let pool = SqlitePool::open(&database_path).expect("open database");
        polkagent_store_sqlite::migrations::migrate(&pool.writer()).expect("migrate database");
        let store = SqliteRunStore::new(pool.clone());
        let timestamp = "2026-01-01T00:00:00Z";
        let spec = serde_json::json!({
            "name": "runtime-agent",
            "description": null,
            "model": "fake/default-model",
            "tools": [],
            "autonomy_level": "supervised",
            "created_at": timestamp,
            "updated_at": timestamp,
        });
        let agent = store
            .create_agent("runtime-agent", None, &spec.to_string())
            .expect("create agent");
        let command = RunCmd {
            agent_id: agent.id.clone(),
            prompt: "hello through RuntimeFactory".to_owned(),
            json: false,
            wait: true,
            provider: None,
            model: None,
            harness: None,
            stream: true,
            no_stream: false,
            no_harness: true,
            timeout: 10,
        };

        let mut started = start_one_shot_run(&command, &pool, Some(&config_path))
            .await
            .expect("start runtime-backed run");
        assert!(started.service.has_timeout_enforcer());
        assert!(started.service.conversation_store().is_some());
        assert!(started.service.payment_store().is_some());
        assert!(started
            .notes
            .iter()
            .any(|note| note.contains("simulated responses")));

        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let event = started.events.recv().await.expect("run event");
                if event.run_id != started.run_id {
                    continue;
                }
                match event.kind {
                    EventKind::RunCompleted { .. } => break,
                    EventKind::RunFailed { reason } => panic!("run failed: {reason}"),
                    EventKind::RunCancelled { reason } => panic!("run cancelled: {reason}"),
                    EventKind::RunTimedOut => panic!("run timed out"),
                    _ => {}
                }
            }
        })
        .await
        .expect("runtime-backed fake run should finish");

        let durable = store
            .get_run(&started.run_id.to_string())
            .expect("durable run");
        assert_eq!(durable.agent_id, agent.id);
        assert_eq!(durable.state, "completed");
    }

    #[test]
    fn one_shot_runtime_options_preserve_command_selection() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let database_path = temp.path().join("runtime-options.db");
        let pool = SqlitePool::open(&database_path).expect("open database");
        let config_path = temp.path().join("selected.toml");
        let command = RunCmd {
            agent_id: "agent".to_owned(),
            prompt: "prompt".to_owned(),
            json: false,
            wait: true,
            provider: Some("anthropic".to_owned()),
            model: Some("claude-opus-4-6".to_owned()),
            harness: Some("codex".to_owned()),
            stream: true,
            no_stream: false,
            no_harness: false,
            timeout: 10,
        };

        let options =
            one_shot_runtime_options(&command, &pool, Some(&config_path)).expect("runtime options");
        assert_eq!(options.config_path.as_deref(), Some(config_path.as_path()));
        assert_eq!(
            options.database_path.as_deref(),
            Some(database_path.as_path())
        );
        assert_eq!(options.provider_override.as_deref(), Some("anthropic"));
        assert_eq!(options.model_override.as_deref(), Some("claude-opus-4-6"));
        assert_eq!(options.harness_override.as_deref(), Some("codex"));
        assert_eq!(options.adapter_policy, AdapterPolicy::AllowSimulated);
    }
}
