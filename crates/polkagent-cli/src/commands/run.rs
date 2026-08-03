//! `polkagent run` — submit a run and stream its output.
//!
//! When no daemon is running this command builds an inline [`AppService`]
//! using the available executor, starts the run, subscribes to the event bus,
//! and streams events to stdout until a terminal event arrives or the timeout
//! expires.

use std::io::Write as _;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use tracing::info;

use polkagent_config::model_registry::synthesize_providers_from_env;
use polkagent_config::Config;
use polkagent_core::event::EventKind;
use polkagent_core::{AgentId, AgentSpec};
use polkagent_event::{EventBus, EventRecorder};
use polkagent_executor_anthropic::AnthropicExecutor;
use polkagent_executor_fake::FakeExecutor;
use polkagent_executor_local::LocalExecutor;
use polkagent_executor_openai::OpenAiExecutor;
use polkagent_executor_trait::ModelExecutor;
use polkagent_service::{AppService, HarnessRegistry, ProviderRegistry};
use polkagent_store_sqlite::{SqlitePool, SqliteRunStore};

use crate::cli::RunCmd;

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Execute the `run` subcommand.
///
/// Builds an [`AppService`] inline (no daemon required), starts the run, then
/// subscribes to the event bus and streams events to stdout until the run
/// reaches a terminal state or the timeout expires.
pub async fn run(cmd: &RunCmd, pool: &SqlitePool) -> Result<()> {
    let store = SqliteRunStore::new(pool.clone());

    // Resolve agent by name or ID (must be active/configured, not archived).
    let agent = store
        .get_agent_by_name_or_id(&cmd.agent_id)
        .map_err(|_| anyhow::anyhow!("Agent not found or not active: {}", cmd.agent_id))?;

    if matches!(agent.state.as_str(), "archived" | "deactivated") {
        anyhow::bail!("Agent not found or not active: {}", cmd.agent_id);
    }

    // Parse the agent's UUID string into a typed AgentId.
    let agent_id: AgentId = agent
        .id
        .parse()
        .with_context(|| format!("invalid agent ID in database: {}", agent.id))?;

    // Load config for provider/harness resolution.
    let config = load_config();

    // Build the provider registry from environment + config providers.
    let registry = build_provider_registry(&config);

    // Resolve provider and executor.
    let (executor, executor_note) =
        resolve_provider(cmd.provider.as_deref(), cmd.model.as_deref(), &config, &registry);

    if let Some(note) = &executor_note {
        eprintln!("{note}");
    }

    // Resolve harness via registry.
    let mut harness_registry = HarnessRegistry::with_known_harnesses();
    // Register any custom harness entries from the config file.
    for (id, entry) in &config.harness.harnesses {
        if let Some(ref path) = entry.binary_path {
            harness_registry.register_with_path(id, id, path.clone());
        } else {
            harness_registry.register(id, id);
        }
    }
    let resolution = harness_registry.resolve(
        cmd.harness.as_deref(),
        config.harness.default.as_deref(),
    );

    if let Some(note) = &resolution.note {
        eprintln!("{note}");
    }

    if let Some(ref name) = resolution.harness_name {
        info!(harness = %name, "harness selected");
    }

    // Instantiate the harness when one was resolved.
    let harness: Option<Arc<dyn polkagent_harness_trait::Harness>> =
        match resolution.harness_name.as_deref() {
            Some("codex") => {
                let hcfg = polkagent_harness_trait::HarnessConfig::new("codex");
                let codex = polkagent_harness_codex::CodexHarness::new(
                    hcfg,
                    polkagent_harness_codex::CodexHarnessConfig::default(),
                )
                .context("creating CodexHarness")?;
                Some(Arc::new(codex))
            }
            Some("claude-code") => {
                let hcfg = polkagent_harness_trait::HarnessConfig::new("claude-code");
                let claude = polkagent_harness_claude::ClaudeHarness::new(
                    hcfg,
                    polkagent_harness_claude::ClaudeHarnessConfig::default(),
                )
                .context("creating ClaudeHarness")?;
                Some(Arc::new(claude))
            }
            Some("cursor") => {
                let hcfg = polkagent_harness_trait::HarnessConfig::new("cursor");
                let cursor = polkagent_harness_acp::AcpHarness::new(
                    polkagent_harness_cursor::CursorConfigurator::default(),
                    hcfg,
                );
                Some(Arc::new(cursor))
            }
            _ => None,
        };

    // -----------------------------------------------------------------------
    // Build AppService inline
    // -----------------------------------------------------------------------

    // Create the event bus and subscribe BEFORE starting the run so we don't
    // miss RunCreated or any early events.
    let event_bus = EventBus::with_default_capacity();
    let mut event_rx = event_bus.subscribe();

    // SqlitePool implements EventStore, so Arc<SqlitePool> coerces to
    // Arc<dyn EventStore> for the EventRecorder.
    let event_recorder = EventRecorder::new(Arc::new(pool.clone()), event_bus.clone());

    // Wrap AppService in Arc so it can be shared with the Ctrl-C handler task.
    let mut builder = AppService::builder()
        .with_config(config)
        .with_run_store(Arc::new(pool.clone()))
        .with_event_bus(event_bus.clone())
        .with_event_recorder(event_recorder)
        .with_executor(executor)
        .with_provider_registry(registry);

    if let Some(h) = harness {
        builder = builder.with_harness(h);
    }

    let app_service = Arc::new(
        builder
            .build()
            .context("building AppService")?,
    );

    // Reconstruct the AgentSpec from the DB row and register it with AppService.
    let agent_spec = build_agent_spec(agent_id, &agent.name, &agent.spec_json, cmd.model.clone());
    app_service
        .create_agent(agent_spec)
        .context("registering agent with AppService")?;

    // -----------------------------------------------------------------------
    // Start the run
    // -----------------------------------------------------------------------
    let run_id = app_service
        .start_run(agent_id, &cmd.prompt)
        .await
        .context("starting run")?;

    info!(%run_id, agent_id = %agent.id, "run started");

    if cmd.json {
        let out = serde_json::json!({
            "run_id":   run_id.to_string(),
            "agent_id": agent.id,
            "state":    "running",
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        println!("Run started: {run_id}");
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
    let stream_tokens = cmd.stream;
    let mut final_text = String::new();

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
                    let _ = app_service.cancel_run(run_id).await;
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

            match &event.kind {
                EventKind::RunCreated => {
                    // Already printed "Run started: {id}" above.
                }
                EventKind::RunQueued | EventKind::RunStarted => {
                    // Lifecycle noise; skip for normal output.
                }
                EventKind::StreamingToken { text } => {
                    if stream_tokens {
                        print!("{text}");
                        // Flush immediately so tokens appear in the terminal.
                        let _ = std::io::stdout().flush();
                    } else {
                        final_text.push_str(text);
                    }
                }
                EventKind::ToolCallStarted { tool_name } => {
                    println!("\n[Tool: {tool_name}]");
                }
                EventKind::EffectIntentCreated { intent_id } => {
                    println!("\n[Effect pending approval: {intent_id}]");
                }
                EventKind::RunCompleted { .. } => {
                    if !stream_tokens && !final_text.is_empty() {
                        println!("{final_text}");
                    }
                    println!("\nRun completed successfully");
                    break;
                }
                EventKind::RunFailed { reason } => {
                    println!("\nRun failed: {reason}");
                    break;
                }
                EventKind::RunCancelled { reason } => {
                    println!("\nRun cancelled: {reason}");
                    break;
                }
                EventKind::RunTimedOut => {
                    println!("\nRun timed out");
                    break;
                }
                _ => {
                    // Ignore all other event kinds (TurnStarted, diagnostics, etc.)
                }
            }
        }

        Ok(())
    }
    .await;

    result
}

// ---------------------------------------------------------------------------
// Config loading
// ---------------------------------------------------------------------------

/// Attempt to load the Polkagent config from standard locations.
///
/// Returns [`Config::default()`] if no config file is found.
fn load_config() -> Config {
    // Project-local config.
    if let Ok(content) = std::fs::read_to_string(".polkagent/polkagent.toml") {
        if let Ok(cfg) = toml::from_str(&content) {
            return cfg;
        }
    }

    // User-global config.
    let home = std::env::var("HOME").unwrap_or_default();
    let user_cfg = format!("{home}/.config/polkagent/polkagent.toml");
    if let Ok(content) = std::fs::read_to_string(&user_cfg) {
        if let Ok(cfg) = toml::from_str(&content) {
            return cfg;
        }
    }

    Config::default()
}

// ---------------------------------------------------------------------------
// Provider registry construction
// ---------------------------------------------------------------------------

/// Build a [`ProviderRegistry`] populated from environment variables and
/// config-file `[[providers]]` sections.
///
/// Environment-synthesized providers are registered first, then config-file
/// providers are layered on top (potentially overriding env-synthesized
/// entries with the same id).
fn build_provider_registry(config: &Config) -> ProviderRegistry {
    let mut registry = ProviderRegistry::new();

    // 1. Synthesize providers from well-known environment variables.
    let env_providers = synthesize_providers_from_env(|k| std::env::var(k));
    for (_kind, pc) in env_providers {
        if let Some(executor) = executor_from_provider_config(&pc) {
            if let Err(e) = registry.register(pc, executor) {
                tracing::warn!(error = %e, "failed to register env-synthesized provider");
            }
        }
    }

    // 2. Register providers from config file (override env-synthesized ones).
    for pc in &config.providers {
        if let Some(executor) = executor_from_provider_config(pc) {
            if let Err(e) = registry.register(pc.clone(), executor) {
                tracing::warn!(error = %e, "failed to register config provider");
            }
        }
    }

    registry
}

/// Try to instantiate an executor from a [`ProviderConfig`].
///
/// Returns `None` if the required API key is not set in the environment.
fn executor_from_provider_config(
    pc: &polkagent_config::ProviderConfig,
) -> Option<Arc<dyn ModelExecutor>> {
    let api_key = if pc.api_key_env.is_empty() {
        String::new()
    } else {
        match std::env::var(&pc.api_key_env) {
            Ok(k) if !k.is_empty() => k,
            _ => return None,
        }
    };

    match pc.provider_type.as_str() {
        "anthropic" => Some(AnthropicExecutor::new(api_key, pc.default_model.clone())),
        "openai" | "openai_compatible" => {
            Some(OpenAiExecutor::new(api_key, pc.default_model.clone()))
        }
        "local" | "ollama" => {
            let url = if pc.base_url.is_empty() {
                "http://localhost:11434/v1".to_owned()
            } else {
                pc.base_url.clone()
            };
            Some(LocalExecutor::custom(url, pc.default_model.clone()))
        }
        "gemini" => {
            // Gemini executor not yet available; use OpenAI-compat shim.
            Some(OpenAiExecutor::new(api_key, pc.default_model.clone()))
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Provider resolution (§ 11.1)
// ---------------------------------------------------------------------------

/// Resolve which provider and executor to use.
///
/// Resolution order:
/// 1. CLI flag: `--provider anthropic --model claude-opus-4-6`
/// 2. Config `[execution] default_provider`
/// 3. First registered provider in the registry (env + config)
/// 4. Fallback: FakeExecutor with warning
fn resolve_provider(
    provider_flag: Option<&str>,
    model_override: Option<&str>,
    config: &Config,
    registry: &ProviderRegistry,
) -> (Arc<dyn ModelExecutor>, Option<String>) {
    // 1. CLI flag — look up in the registry.
    if let Some(provider_id) = provider_flag {
        if let Ok(executor) = registry.get_executor(provider_id) {
            let note = format!("Using provider '{provider_id}' (from --provider flag).");
            return (executor, Some(note));
        }
        // Provider flag given but not in registry — try env-based matching.
        if let Some(result) = try_provider_by_name(provider_id, model_override) {
            return result;
        }
        eprintln!(
            "Warning: provider '{provider_id}' not found in config and no matching \
             API key detected. Falling back."
        );
    }

    // 2. Config default_provider.
    if let Some(ref default_id) = config.execution.default_provider {
        if !default_id.is_empty() {
            if let Ok(executor) = registry.get_executor(default_id) {
                let note = format!(
                    "Using provider '{default_id}' (from config default_provider)."
                );
                return (executor, Some(note));
            }
            eprintln!(
                "Warning: configured default_provider '{default_id}' not found in registry."
            );
        }
    }

    // 3. First available provider from the registry.
    let providers = registry.list_providers();
    if let Some(first) = providers.first() {
        if let Ok(executor) = registry.get_executor(&first.id) {
            let note = format!(
                "Using provider '{}' (auto-detected from environment).",
                first.id
            );
            return (executor, Some(note));
        }
    }

    // 4. Fallback — fake executor.
    let note = Some(
        "No API key found (ANTHROPIC_API_KEY / OPENAI_API_KEY) and no local \
         model configured (OLLAMA_URL / OLLAMA_MODEL). Using the fake \
         executor \u{2014} responses will be simulated. Set an API key or \
         local model environment variable to use a real model."
            .to_string(),
    );
    (FakeExecutor::new(), note)
}

/// Try to build an executor by matching a provider name to well-known types.
fn try_provider_by_name(
    name: &str,
    model_override: Option<&str>,
) -> Option<(Arc<dyn ModelExecutor>, Option<String>)> {
    match name {
        "anthropic" => {
            let api_key = std::env::var("ANTHROPIC_API_KEY").ok().filter(|k| !k.is_empty())?;
            let model = model_override.unwrap_or("claude-sonnet-4-20250514").to_string();
            let executor = AnthropicExecutor::new(api_key, model.clone());
            Some((executor, Some(format!("Using Anthropic executor (model: {model})."))))
        }
        "openai" => {
            let api_key = std::env::var("OPENAI_API_KEY").ok().filter(|k| !k.is_empty())?;
            let model = model_override.unwrap_or("gpt-4o").to_string();
            let executor = OpenAiExecutor::new(api_key, model.clone());
            Some((executor, Some(format!("Using OpenAI executor (model: {model})."))))
        }
        "ollama" | "local" => {
            let url = std::env::var("OLLAMA_URL")
                .ok()
                .filter(|u| !u.is_empty())
                .unwrap_or_else(|| "http://localhost:11434/v1".to_owned());
            let model = model_override.unwrap_or("llama3.2").to_string();
            let executor = LocalExecutor::custom(url.clone(), model.clone());
            Some((executor, Some(format!("Using local executor at {url} (model: {model})."))))
        }
        _ => None,
    }
}

/// Detect which model executor to use based on environment variables.
///
/// Retained for backward-compatible test coverage. Production code now uses
/// [`build_provider_registry`] + [`resolve_provider`] instead.
///
/// Priority:
/// 1. `ANTHROPIC_API_KEY` present → [`AnthropicExecutor`].
/// 2. `OPENAI_API_KEY` present → [`OpenAiExecutor`].
/// 3. `OLLAMA_URL` present → [`LocalExecutor`] targeting the given URL.
/// 4. `OLLAMA_MODEL` present → [`LocalExecutor`] with default Ollama endpoint.
/// 5. Otherwise → [`FakeExecutor`] with a helpful note.
#[cfg(test)]
fn detect_executor(
    model_override: Option<&str>,
) -> (Arc<dyn ModelExecutor>, Option<String>) {
    // 1. Anthropic
    if let Ok(api_key) = std::env::var("ANTHROPIC_API_KEY") {
        if !api_key.is_empty() {
            let model = model_override
                .unwrap_or("claude-sonnet-4-20250514")
                .to_string();
            let executor = AnthropicExecutor::new(api_key, model.clone());
            let note = Some(format!(
                "Using Anthropic executor (model: {model})."
            ));
            return (executor, note);
        }
    }

    // 2. OpenAI
    if let Ok(api_key) = std::env::var("OPENAI_API_KEY") {
        if !api_key.is_empty() {
            let model = model_override.unwrap_or("gpt-4o").to_string();
            let executor = OpenAiExecutor::new(api_key, model.clone());
            let note = Some(format!(
                "Using OpenAI executor (model: {model})."
            ));
            return (executor, note);
        }
    }

    // 3. Local executor — explicit OLLAMA_URL
    if let Ok(url) = std::env::var("OLLAMA_URL") {
        if !url.is_empty() {
            let model = model_override.unwrap_or("llama3.2").to_string();
            let executor = LocalExecutor::custom(url.clone(), model.clone());
            let note = Some(format!(
                "Using local executor at {url} (model: {model})."
            ));
            return (executor, note);
        }
    }

    // 4. Local executor — OLLAMA_MODEL with default Ollama endpoint
    if let Ok(ollama_model) = std::env::var("OLLAMA_MODEL") {
        if !ollama_model.is_empty() {
            let model = model_override.unwrap_or(&ollama_model).to_string();
            let executor = LocalExecutor::ollama(model.clone());
            let note = Some(format!(
                "Using local Ollama executor (model: {model})."
            ));
            return (executor, note);
        }
    }

    // 5. Fallback — fake executor
    let note = Some(
        "No API key found (ANTHROPIC_API_KEY / OPENAI_API_KEY) and no local \
         model configured (OLLAMA_URL / OLLAMA_MODEL). Using the fake \
         executor \u{2014} responses will be simulated. Set an API key or \
         local model environment variable to use a real model."
            .to_string(),
    );
    (FakeExecutor::new(), note)
}

// ---------------------------------------------------------------------------
// AgentSpec reconstruction
// ---------------------------------------------------------------------------

/// Build an [`AgentSpec`] for use with [`AppService::create_agent`].
///
/// Tries to deserialise the stored `spec_json`. Falls back to a minimal spec
/// constructed from the agent's name. CLI model override always wins.
fn build_agent_spec(
    agent_id: AgentId,
    name: &str,
    spec_json: &str,
    model_override: Option<String>,
) -> AgentSpec {
    let mut spec: AgentSpec = serde_json::from_str(spec_json).unwrap_or_else(|_| {
        AgentSpec::new(agent_id, name, "fake/default-model")
    });

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

        let spec = build_agent_spec(
            id,
            "test-agent",
            &json,
            Some("openai/gpt-4o".to_string()),
        );
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
            let lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
            let vars: Vec<_> = Self::KEYS
                .iter()
                .map(|&k| (k, std::env::var(k).ok()))
                .collect();
            #[allow(deprecated)]
            for &k in Self::KEYS {
                std::env::remove_var(k);
            }
            Self { vars, _lock: lock }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            #[allow(deprecated)]
            for (k, v) in &self.vars {
                match v {
                    Some(val) => std::env::set_var(k, val),
                    None => std::env::remove_var(k),
                }
            }
        }
    }

    #[test]
    fn detect_executor_falls_back_to_fake_with_helpful_note() {
        let _guard = EnvGuard::new();

        let (_exec, note) = detect_executor(None);
        assert!(note.is_some());
        let note_text = note.unwrap();
        assert!(
            note_text.contains("fake executor"),
            "expected note to mention 'fake executor', got: {note_text}",
        );
    }

    #[test]
    fn detect_executor_uses_anthropic_when_key_set() {
        let _guard = EnvGuard::new();
        #[allow(deprecated)]
        std::env::set_var("ANTHROPIC_API_KEY", "sk-ant-test-key");

        let (_exec, note) = detect_executor(None);
        let note_text = note.expect("expected a note");
        assert!(
            note_text.contains("Anthropic"),
            "expected note to mention 'Anthropic', got: {note_text}",
        );
        assert!(
            !note_text.contains("fake"),
            "note should not mention 'fake' when real executor is used",
        );
    }

    #[test]
    fn detect_executor_uses_openai_when_key_set() {
        let _guard = EnvGuard::new();
        #[allow(deprecated)]
        std::env::set_var("OPENAI_API_KEY", "sk-test-key");

        let (_exec, note) = detect_executor(None);
        let note_text = note.expect("expected a note");
        assert!(
            note_text.contains("OpenAI"),
            "expected note to mention 'OpenAI', got: {note_text}",
        );
        assert!(
            !note_text.contains("fake"),
            "note should not mention 'fake' when real executor is used",
        );
    }

    #[test]
    fn detect_executor_uses_local_when_ollama_url_set() {
        let _guard = EnvGuard::new();
        #[allow(deprecated)]
        std::env::set_var("OLLAMA_URL", "http://localhost:11434/v1");

        let (_exec, note) = detect_executor(None);
        let note_text = note.expect("expected a note");
        assert!(
            note_text.contains("local executor"),
            "expected note to mention 'local executor', got: {note_text}",
        );
    }

    #[test]
    fn detect_executor_uses_local_when_ollama_model_set() {
        let _guard = EnvGuard::new();
        #[allow(deprecated)]
        std::env::set_var("OLLAMA_MODEL", "mistral");

        let (_exec, note) = detect_executor(None);
        let note_text = note.expect("expected a note");
        assert!(
            note_text.contains("Ollama"),
            "expected note to mention 'Ollama', got: {note_text}",
        );
        assert!(
            note_text.contains("mistral"),
            "expected note to mention model 'mistral', got: {note_text}",
        );
    }

    #[test]
    fn detect_executor_anthropic_takes_priority_over_openai() {
        let _guard = EnvGuard::new();
        #[allow(deprecated)]
        {
            std::env::set_var("ANTHROPIC_API_KEY", "sk-ant-test");
            std::env::set_var("OPENAI_API_KEY", "sk-test");
        }

        let (_exec, note) = detect_executor(None);
        let note_text = note.expect("expected a note");
        assert!(
            note_text.contains("Anthropic"),
            "Anthropic should take priority when both keys are set, got: {note_text}",
        );
    }

    #[test]
    fn detect_executor_model_override_is_applied() {
        let _guard = EnvGuard::new();
        #[allow(deprecated)]
        std::env::set_var("ANTHROPIC_API_KEY", "sk-ant-test");

        let (_exec, note) = detect_executor(Some("claude-opus-4-6"));
        let note_text = note.expect("expected a note");
        assert!(
            note_text.contains("claude-opus-4-6"),
            "model override should appear in note, got: {note_text}",
        );
    }

    #[test]
    fn detect_executor_empty_key_treated_as_absent() {
        let _guard = EnvGuard::new();
        #[allow(deprecated)]
        std::env::set_var("ANTHROPIC_API_KEY", "");

        let (_exec, note) = detect_executor(None);
        let note_text = note.expect("expected a note");
        assert!(
            note_text.contains("fake executor"),
            "empty key should fall through to fake executor, got: {note_text}",
        );
    }

    // -----------------------------------------------------------------------
    // Provider resolution tests
    // -----------------------------------------------------------------------

    #[test]
    fn resolve_provider_cli_flag_anthropic() {
        let _guard = EnvGuard::new();
        #[allow(deprecated)]
        std::env::set_var("ANTHROPIC_API_KEY", "sk-ant-test");

        let config = Config::default();
        let registry = build_provider_registry(&config);
        let (_exec, note) = resolve_provider(Some("anthropic"), None, &config, &registry);
        let note_text = note.expect("expected a note");
        assert!(
            note_text.contains("Anthropic") || note_text.contains("anthropic"),
            "expected note to mention 'Anthropic', got: {note_text}",
        );
    }

    #[test]
    fn resolve_provider_cli_flag_openai() {
        let _guard = EnvGuard::new();
        #[allow(deprecated)]
        std::env::set_var("OPENAI_API_KEY", "sk-test");

        let config = Config::default();
        let registry = build_provider_registry(&config);
        let (_exec, note) = resolve_provider(Some("openai"), None, &config, &registry);
        let note_text = note.expect("expected a note");
        assert!(
            note_text.contains("OpenAI") || note_text.contains("openai"),
            "expected note to mention 'OpenAI', got: {note_text}",
        );
    }

    #[test]
    fn resolve_provider_from_config_entry() {
        let _guard = EnvGuard::new();
        #[allow(deprecated)]
        std::env::set_var("MY_ANTHROPIC_KEY", "sk-custom");

        let mut config = Config::default();
        config.providers.push(polkagent_config::ProviderConfig {
            id: "my-anthropic".to_owned(),
            provider_type: "anthropic".to_owned(),
            api_key_env: "MY_ANTHROPIC_KEY".to_owned(),
            default_model: "claude-opus-4-6".to_owned(),
            ..Default::default()
        });

        let registry = build_provider_registry(&config);
        let (_exec, note) = resolve_provider(Some("my-anthropic"), None, &config, &registry);
        let note_text = note.expect("expected a note");
        assert!(
            note_text.contains("my-anthropic"),
            "expected note to mention provider id, got: {note_text}",
        );
    }

    #[test]
    fn resolve_provider_fallback_to_env_detection() {
        let _guard = EnvGuard::new();
        #[allow(deprecated)]
        std::env::set_var("OPENAI_API_KEY", "sk-test");

        let config = Config::default();
        let registry = build_provider_registry(&config);
        // No provider flag, no config providers — should fall to env detection.
        let (_exec, note) = resolve_provider(None, None, &config, &registry);
        let note_text = note.expect("expected a note");
        assert!(
            note_text.contains("OpenAI") || note_text.contains("openai"),
            "should detect OpenAI from env, got: {note_text}",
        );
    }

    #[test]
    fn resolve_provider_fallback_to_fake() {
        let _guard = EnvGuard::new();

        let config = Config::default();
        let registry = build_provider_registry(&config);
        let (_exec, note) = resolve_provider(None, None, &config, &registry);
        let note_text = note.expect("expected a note");
        assert!(
            note_text.contains("fake executor"),
            "should fall back to fake, got: {note_text}",
        );
    }

    #[test]
    fn resolve_provider_default_provider_from_config() {
        let _guard = EnvGuard::new();
        #[allow(deprecated)]
        std::env::set_var("OPENAI_API_KEY", "sk-test");

        let mut config = Config::default();
        config.execution.default_provider = Some("openai".to_owned());

        let registry = build_provider_registry(&config);
        // No CLI flag — should pick up default_provider from config.
        let (_exec, note) = resolve_provider(None, None, &config, &registry);
        let note_text = note.expect("expected a note");
        assert!(
            note_text.contains("openai") && note_text.contains("default_provider"),
            "should use config default_provider, got: {note_text}",
        );
    }

    #[test]
    fn build_provider_registry_populates_from_env() {
        let _guard = EnvGuard::new();
        #[allow(deprecated)]
        std::env::set_var("ANTHROPIC_API_KEY", "sk-ant-test");

        let config = Config::default();
        let registry = build_provider_registry(&config);
        assert!(
            !registry.is_empty(),
            "registry should have at least one provider from env"
        );
        assert!(
            registry.get_executor("anthropic").is_ok(),
            "should find anthropic provider in registry"
        );
    }

    #[test]
    fn build_provider_registry_config_overrides_env() {
        let _guard = EnvGuard::new();
        #[allow(deprecated)]
        std::env::set_var("MY_KEY", "sk-custom");

        let mut config = Config::default();
        config.providers.push(polkagent_config::ProviderConfig {
            id: "custom-provider".to_owned(),
            provider_type: "anthropic".to_owned(),
            api_key_env: "MY_KEY".to_owned(),
            default_model: "claude-opus-4-6".to_owned(),
            ..Default::default()
        });

        let registry = build_provider_registry(&config);
        assert!(
            registry.get_executor("custom-provider").is_ok(),
            "config-defined provider should be in registry"
        );
    }

    #[test]
    fn build_provider_registry_empty_when_no_keys() {
        let _guard = EnvGuard::new();

        let config = Config::default();
        let registry = build_provider_registry(&config);
        assert!(
            registry.is_empty(),
            "registry should be empty with no env keys"
        );
    }

    // -----------------------------------------------------------------------
    // Harness registry resolution tests
    // -----------------------------------------------------------------------

    #[test]
    fn harness_registry_resolve_no_flag_no_config_falls_back() {
        let mut registry = HarnessRegistry::with_known_harnesses();
        let result = registry.resolve(None, None);
        // We can't guarantee any harness is on PATH in CI, but the result
        // should either be auto-detected or fallback.
        if result.harness_name.is_none() {
            let note = result.note.expect("expected a note");
            assert!(
                note.contains("executor-only mode"),
                "expected fallback note, got: {note}",
            );
        } else {
            let note = result.note.expect("expected a note");
            assert!(
                note.contains("Auto-detected"),
                "expected auto-detect note, got: {note}",
            );
        }
    }

    #[test]
    fn harness_registry_resolve_nonexistent_binary_falls_back() {
        let mut registry = HarnessRegistry::with_known_harnesses();
        let result = registry.resolve(Some("nonexistent-harness-xyz"), None);
        assert!(result.harness_name.is_none());
        let note = result.note.expect("expected a note");
        assert!(
            note.contains("not found on PATH"),
            "expected 'not found' note, got: {note}",
        );
    }

    #[test]
    fn harness_registry_has_known_entries() {
        let registry = HarnessRegistry::with_known_harnesses();
        let ids: Vec<String> = registry
            .list_harnesses()
            .iter()
            .map(|h| h.id.clone())
            .collect();
        assert!(ids.contains(&"claude-code".to_owned()));
        assert!(ids.contains(&"codex".to_owned()));
        assert!(ids.contains(&"cursor".to_owned()));
        assert!(ids.contains(&"goose".to_owned()));
    }
}
