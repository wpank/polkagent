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

use polkagent_config::Config;
use polkagent_core::event::EventKind;
use polkagent_core::{AgentId, AgentSpec};
use polkagent_event::{EventBus, EventRecorder};
use polkagent_executor_anthropic::AnthropicExecutor;
use polkagent_executor_fake::FakeExecutor;
use polkagent_executor_local::LocalExecutor;
use polkagent_executor_openai::OpenAiExecutor;
use polkagent_executor_trait::ModelExecutor;
use polkagent_service::AppService;
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

    // Detect executor based on environment.
    let (executor, executor_note) = detect_executor(cmd.model.as_deref());

    if let Some(note) = &executor_note {
        eprintln!("{note}");
    }

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

    let config = Config::default();

    // Wrap AppService in Arc so it can be shared with the Ctrl-C handler task.
    let app_service = Arc::new(
        AppService::builder()
            .with_config(config)
            .with_run_store(Arc::new(pool.clone()))
            .with_event_bus(event_bus.clone())
            .with_event_recorder(event_recorder)
            .with_executor(executor)
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
// Executor detection
// ---------------------------------------------------------------------------

/// Detect which model executor to use based on environment variables.
///
/// Priority:
/// 1. `ANTHROPIC_API_KEY` present → [`AnthropicExecutor`] with the key.
/// 2. `OPENAI_API_KEY` present → [`OpenAiExecutor`] with the key.
/// 3. `OLLAMA_URL` present → [`LocalExecutor`] targeting the given URL.
/// 4. `OLLAMA_MODEL` present (without URL) → [`LocalExecutor`] targeting
///    the default Ollama endpoint (`http://localhost:11434/v1`).
/// 5. Otherwise → [`FakeExecutor`] with a helpful note.
///
/// An optional `model_override` replaces the executor's default model.
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

    /// Helper: save, clear, and restore executor-related env vars so that
    /// individual tests are isolated from the host environment.
    struct EnvGuard {
        vars: Vec<(&'static str, Option<String>)>,
    }

    impl EnvGuard {
        const KEYS: &'static [&'static str] = &[
            "ANTHROPIC_API_KEY",
            "OPENAI_API_KEY",
            "OLLAMA_URL",
            "OLLAMA_MODEL",
        ];

        fn new() -> Self {
            let vars: Vec<_> = Self::KEYS
                .iter()
                .map(|&k| (k, std::env::var(k).ok()))
                .collect();
            #[allow(deprecated)]
            for &k in Self::KEYS {
                std::env::remove_var(k);
            }
            Self { vars }
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
}
