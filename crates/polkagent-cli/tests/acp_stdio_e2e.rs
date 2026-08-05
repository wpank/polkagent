//! Executable ACP proof through the real `polkagent acp` subprocess.

// End-to-end assertions unwrap controlled process and protocol fixtures.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::fs::OpenOptions;
use std::io::Write as _;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_client_protocol::schema::v1::{
    ContentBlock, InitializeRequest, NewSessionRequest, PromptRequest, SessionNotification,
    SessionUpdate, StopReason, TextContent,
};
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::{AcpAgent, AcpAgentConfig, Agent, LineDirection};
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};

#[derive(Default)]
struct ObservedUpdates {
    command_names: Vec<String>,
    messages: Vec<String>,
    stdout_lines: Vec<String>,
    stderr_lines: Vec<String>,
}

struct DelayedProvider {
    base_url: String,
    request_seen: tokio::sync::oneshot::Receiver<()>,
    task: tokio::task::JoinHandle<()>,
}

struct FailingProvider {
    base_url: String,
    task: tokio::task::JoinHandle<()>,
}

fn write_local_provider_config(path: &std::path::Path, id: &str, base_url: &str) {
    std::fs::write(
        path,
        format!(
            "[[providers]]\n\
             id = \"{id}\"\n\
             provider_type = \"local\"\n\
             base_url = \"{base_url}\"\n\
             api_key_env = \"POLKAGENT_ACP_FIXTURE_KEY\"\n\
             default_model = \"fixture-model\"\n"
        ),
    )
    .expect("write local provider config");
}

#[cfg(debug_assertions)]
#[test]
fn acp_panic_hook_suppresses_payload_and_keeps_stdout_empty() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let db_path = temp.path().join("polkagent.db");
    let binary = env!("CARGO_BIN_EXE_polkagent");
    let raw_secret = "sk-acpPanicHookFixture123";

    let output = run_acp_panic_probe(binary, &db_path, temp.path(), None, raw_secret);

    assert!(!output.status.success());
    assert!(
        output.stdout.is_empty(),
        "ACP panic contaminated protocol stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains(
        "Polkagent ACP encountered an internal panic; sensitive details were suppressed"
    ));
    assert!(
        !stderr.contains(raw_secret),
        "panic payload leaked: {stderr}"
    );
    assert!(
        !stderr.contains("panicked at") && !stderr.contains("main.rs"),
        "default panic location leaked: {stderr}"
    );
    assert!(
        !temp.path().join(".polkagent/logs").exists(),
        "ACP diagnostics must remain disabled without an explicit --log-file"
    );
}

#[cfg(debug_assertions)]
#[test]
fn acp_file_diagnostics_are_redacted_restrictive_rotated_and_stdout_safe() {
    const MAX_BYTES: usize = 1024 * 1024;

    let temp = tempfile::tempdir().expect("temporary directory");
    let db_path = temp.path().join("polkagent.db");
    let log_path = temp.path().join("private/acp.jsonl");
    let binary = env!("CARGO_BIN_EXE_polkagent");
    let first_secret = "sk-acpFileDiagnosticFixture123";

    let first = run_acp_panic_probe(binary, &db_path, temp.path(), Some(&log_path), first_secret);
    assert!(!first.status.success());
    assert!(first.stdout.is_empty());
    let first_log = std::fs::read_to_string(&log_path).expect("read ACP diagnostic file");
    assert!(first_log.contains("\"event\":\"acp.panic_probe\""));
    assert!(first_log.contains("sk-***REDACTED***"));
    assert!(!first_log.contains(first_secret));
    for line in first_log.lines() {
        serde_json::from_str::<serde_json::Value>(line).expect("valid ACP diagnostic JSONL");
    }

    OpenOptions::new()
        .append(true)
        .open(&log_path)
        .expect("open diagnostic file for rotation fixture")
        .write_all(&vec![b'x'; MAX_BYTES])
        .expect("grow diagnostic file beyond rotation threshold");

    let second_secret = "sk-acpRotatedDiagnosticFixture456";
    let second = run_acp_panic_probe(
        binary,
        &db_path,
        temp.path(),
        Some(&log_path),
        second_secret,
    );
    assert!(!second.status.success());
    assert!(second.stdout.is_empty());
    assert!(!String::from_utf8_lossy(&second.stderr).contains(second_secret));
    let rotated_path = std::path::PathBuf::from(format!("{}.1", log_path.display()));
    assert!(
        rotated_path.exists(),
        "oversized active log was not rotated"
    );
    assert!(
        std::fs::metadata(&log_path)
            .expect("active diagnostic metadata")
            .len()
            < u64::try_from(MAX_BYTES).expect("rotation threshold fits u64")
    );
    let second_log = std::fs::read_to_string(&log_path).expect("read rotated active log");
    assert!(second_log.contains("sk-***REDACTED***"));
    assert!(!second_log.contains(second_secret));

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;

        let file_mode = std::fs::metadata(&log_path)
            .expect("diagnostic file metadata")
            .permissions()
            .mode()
            & 0o777;
        let dir_mode = std::fs::metadata(log_path.parent().expect("diagnostic parent"))
            .expect("diagnostic directory metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(file_mode, 0o600);
        assert_eq!(dir_mode, 0o700);
    }
}

#[cfg(debug_assertions)]
fn run_acp_panic_probe(
    binary: &str,
    db_path: &std::path::Path,
    home: &std::path::Path,
    log_path: Option<&std::path::Path>,
    payload: &str,
) -> std::process::Output {
    let mut command = Command::new(binary);
    if let Some(log_path) = log_path {
        command.args(["--log-file", log_path.to_string_lossy().as_ref()]);
    }
    command
        .arg("acp")
        .env("HOME", home)
        .env("POLKAGENT_DATABASE_SQLITE_PATH", db_path)
        .env("POLKAGENT_INTERNAL_ACP_PANIC_PROBE", payload)
        .output()
        .expect("run the debug-only ACP panic probe")
}

#[cfg(debug_assertions)]
#[test]
fn acp_panic_probe_is_ignored_by_non_acp_commands() {
    let binary = env!("CARGO_BIN_EXE_polkagent");
    let output = Command::new(binary)
        .arg("version")
        .env(
            "POLKAGENT_INTERNAL_ACP_PANIC_PROBE",
            "sk-nonAcpHookIsolationFixture123",
        )
        .output()
        .expect("run a non-ACP command with the debug probe variable");

    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).starts_with("polkagent "));
    assert!(
        output.stderr.is_empty(),
        "non-ACP command unexpectedly used the ACP panic hook: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
async fn official_client_drives_editor_commands_and_a_real_run() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let db_path = temp.path().join("polkagent.db");
    let log_path = temp.path().join("acp-success.jsonl");
    let log_arg = log_path.to_string_lossy().into_owned();
    let project_path = temp.path().to_path_buf();
    let binary = env!("CARGO_BIN_EXE_polkagent");

    assert_cli_success(
        binary,
        &db_path,
        &["agent", "create", "editor-fixture", "--model", "fake/test"],
    );
    assert_cli_success(binary, &db_path, &["agent", "start", "editor-fixture"]);

    let observed = Arc::new(Mutex::new(ObservedUpdates::default()));
    let observed_by_client = Arc::clone(&observed);
    let agent = observed_agent(
        AcpAgentConfig::new(binary)
            .args([
                "--log-file",
                log_arg.as_str(),
                "acp",
                "--agent",
                "editor-fixture",
                "--timeout",
                "20",
            ])
            .env(
                "POLKAGENT_DATABASE_SQLITE_PATH",
                db_path.to_string_lossy().into_owned(),
            ),
        Arc::clone(&observed),
    );

    agent_client_protocol::Client
        .builder()
        .on_receive_notification(
            async move |notification: SessionNotification, _connection| {
                let mut observed = observed_by_client.lock().expect("observed updates lock");
                match notification.update {
                    SessionUpdate::AvailableCommandsUpdate(update) => {
                        observed.command_names.extend(
                            update
                                .available_commands
                                .into_iter()
                                .map(|command| command.name),
                        );
                    }
                    SessionUpdate::AgentMessageChunk(chunk) => {
                        if let ContentBlock::Text(text) = chunk.content {
                            observed.messages.push(text.text);
                        }
                    }
                    _ => {}
                }
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .connect_with(
            agent,
            |connection: agent_client_protocol::ConnectionTo<Agent>| async move {
                let initialized = connection
                    .send_request(InitializeRequest::new(ProtocolVersion::V1))
                    .block_task()
                    .await?;
                assert_eq!(initialized.protocol_version, ProtocolVersion::V1);
                assert_eq!(
                    initialized
                        .agent_info
                        .as_ref()
                        .map(|info| info.name.as_str()),
                    Some("polkagent")
                );

                let session = connection
                    .send_request(NewSessionRequest::new(project_path))
                    .block_task()
                    .await?;

                for command in [
                    "/commands stop",
                    "/st",
                    "/agents",
                    "/use editor-fixture",
                    "/runs",
                ] {
                    let response = connection
                        .send_request(PromptRequest::new(
                            session.session_id.clone(),
                            vec![ContentBlock::Text(TextContent::new(command))],
                        ))
                        .block_task()
                        .await?;
                    assert_eq!(response.stop_reason, StopReason::EndTurn);
                }

                let run = connection
                    .send_request(PromptRequest::new(
                        session.session_id,
                        vec![ContentBlock::Text(TextContent::new(
                            "Return a short editor integration greeting.",
                        ))],
                    ))
                    .block_task()
                    .await?;
                assert_eq!(run.stop_reason, StopReason::EndTurn);
                Ok(())
            },
        )
        .await
        .expect("official ACP client completed the subprocess session");

    let observed = observed.lock().expect("observed updates lock");
    assert_editor_command_updates(&observed);
    assert_safe_success_diagnostics(&log_path);
}

fn assert_safe_success_diagnostics(log_path: &std::path::Path) {
    let content = std::fs::read_to_string(log_path).expect("read successful ACP diagnostics");
    assert!(content.contains("\"event\":\"acp.server_ready\""));
    assert!(content.contains("\"event\":\"acp.prompt_completed\""));
    assert!(!content.contains("Return a short editor integration greeting."));
    assert!(!content.contains("I am a fake assistant"));
    for line in content.lines() {
        serde_json::from_str::<serde_json::Value>(line).expect("valid ACP diagnostic JSONL");
    }
}

fn assert_editor_command_updates(observed: &ObservedUpdates) {
    assert_eq!(
        observed.command_names,
        vec!["help", "status", "agents", "agent", "cancel"]
    );
    assert!(
        observed
            .messages
            .iter()
            .any(|message| message.contains("/cancel") && message.contains("/stop")),
        "registry-backed detailed help was not streamed: {:?}",
        observed.messages
    );
    assert!(
        observed
            .messages
            .iter()
            .any(|message| message.contains("Active prompt: no")),
        "status alias did not report truthful editor state: {:?}",
        observed.messages
    );
    assert!(
        observed
            .messages
            .iter()
            .any(|message| message.contains("Active agents:")),
        "agent discovery was not streamed: {:?}",
        observed.messages
    );
    assert!(
        observed
            .messages
            .iter()
            .any(|message| message.contains("Selected agent 'editor-fixture'")),
        "agent-selection alias was not actionable: {:?}",
        observed.messages
    );
    assert!(
        observed.messages.iter().any(|message| {
            message.contains("shared Polkagent command registry")
                && message.contains("not supported by ACP yet")
        }),
        "unsupported durable command was not declined truthfully: {:?}",
        observed.messages
    );
    assert!(
        observed.messages.len() >= 6,
        "the real Polkagent run did not stream an agent message after commands: {:?}",
        observed.messages
    );
    assert_protocol_stdout(&observed.stdout_lines);
    assert!(
        observed
            .stderr_lines
            .iter()
            .any(|line| line.contains("Using simulated responses")),
        "provider diagnostics did not stay on stderr: {:?}",
        observed.stderr_lines
    );
}

#[tokio::test]
async fn acp_restart_recovers_abandoned_run_through_shared_runtime() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let db_path = temp.path().join("polkagent.db");
    let log_path = temp.path().join("acp-restart.jsonl");
    let project_path = temp.path().to_path_buf();
    let binary = env!("CARGO_BIN_EXE_polkagent");

    assert_cli_success(
        binary,
        &db_path,
        &["agent", "create", "restart-fixture", "--model", "fake/test"],
    );
    assert_cli_success(binary, &db_path, &["agent", "start", "restart-fixture"]);

    let abandoned_run_id = uuid::Uuid::now_v7().to_string();
    {
        let connection = rusqlite::Connection::open(&db_path).expect("open fixture database");
        let agent_id: String = connection
            .query_row(
                "SELECT id FROM agents WHERE name = 'restart-fixture'",
                [],
                |row| row.get(0),
            )
            .expect("load fixture agent ID");
        connection
            .execute(
                "INSERT INTO runs (id, agent_id, state, params_json, created_at, updated_at) \
                 VALUES (?1, ?2, 'running', '{}', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
                rusqlite::params![abandoned_run_id, agent_id],
            )
            .expect("seed run abandoned by a previous process");
    }

    let observed = Arc::new(Mutex::new(ObservedUpdates::default()));
    let log_arg = log_path.to_string_lossy().into_owned();
    let agent = observed_agent(
        AcpAgentConfig::new(binary)
            .args([
                "--log-file",
                log_arg.as_str(),
                "acp",
                "--agent",
                "restart-fixture",
            ])
            .env(
                "POLKAGENT_DATABASE_SQLITE_PATH",
                db_path.to_string_lossy().into_owned(),
            ),
        Arc::clone(&observed),
    );

    agent_client_protocol::Client
        .connect_with(
            agent,
            |connection: agent_client_protocol::ConnectionTo<Agent>| async move {
                connection
                    .send_request(InitializeRequest::new(ProtocolVersion::V1))
                    .block_task()
                    .await?;
                connection
                    .send_request(NewSessionRequest::new(project_path))
                    .block_task()
                    .await?;
                Ok(())
            },
        )
        .await
        .expect("official ACP client restarted the subprocess");

    let connection = rusqlite::Connection::open(&db_path).expect("reopen durable ACP database");
    let (state, completed_at): (String, Option<String>) = connection
        .query_row(
            "SELECT state, completed_at FROM runs WHERE id = ?1",
            [&abandoned_run_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("load recovered run");
    assert_eq!(state, "failed");
    assert!(
        completed_at.is_some(),
        "runtime startup recovery must persist a terminal timestamp"
    );

    let diagnostics = std::fs::read_to_string(log_path).expect("read restart diagnostics");
    assert!(diagnostics.contains("\"event\":\"acp.runtime_ready\""));
    assert!(diagnostics.contains("\"event\":\"acp.server_ready\""));
    let observed = observed.lock().expect("observed updates lock");
    assert_protocol_stdout(&observed.stdout_lines);
}

#[tokio::test]
async fn official_client_cancels_active_run_and_persists_terminal_state() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let db_path = temp.path().join("polkagent.db");
    let config_path = temp.path().join("polkagent.toml");
    let project_path = temp.path().to_path_buf();
    let binary = env!("CARGO_BIN_EXE_polkagent");

    assert_cli_success(
        binary,
        &db_path,
        &[
            "agent",
            "create",
            "cancel-fixture",
            "--model",
            "fixture/model",
        ],
    );
    assert_cli_success(binary, &db_path, &["agent", "start", "cancel-fixture"]);

    let delayed_provider = delayed_provider().await;
    write_local_provider_config(&config_path, "cancel-provider", &delayed_provider.base_url);

    let observed = Arc::new(Mutex::new(ObservedUpdates::default()));
    let observed_by_client = Arc::clone(&observed);
    let config_arg = config_path.to_string_lossy().into_owned();
    let agent = observed_agent(
        AcpAgentConfig::new(binary)
            .args([
                "--config",
                config_arg.as_str(),
                "acp",
                "--agent",
                "cancel-fixture",
                "--provider",
                "cancel-provider",
                "--timeout",
                "20",
            ])
            .env(
                "POLKAGENT_DATABASE_SQLITE_PATH",
                db_path.to_string_lossy().into_owned(),
            )
            .env("ANTHROPIC_API_KEY", "")
            .env("OPENAI_API_KEY", "")
            .env("GEMINI_API_KEY", "")
            .env("OPENROUTER_API_KEY", "")
            .env("PERPLEXITY_API_KEY", "")
            .env("CEREBRAS_API_KEY", "")
            .env("OLLAMA_URL", "")
            .env("OLLAMA_MODEL", "")
            .env("POLKAGENT_ACP_FIXTURE_KEY", "fixture-key"),
        Arc::clone(&observed),
    );

    agent_client_protocol::Client
        .builder()
        .on_receive_notification(
            async move |notification: SessionNotification, _connection| {
                if let SessionUpdate::AgentMessageChunk(chunk) = notification.update {
                    if let ContentBlock::Text(text) = chunk.content {
                        observed_by_client
                            .lock()
                            .expect("observed updates lock")
                            .messages
                            .push(text.text);
                    }
                }
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .connect_with(
            agent,
            |connection: agent_client_protocol::ConnectionTo<Agent>| async move {
                connection
                    .send_request(InitializeRequest::new(ProtocolVersion::V1))
                    .block_task()
                    .await?;
                let session = connection
                    .send_request(NewSessionRequest::new(project_path))
                    .block_task()
                    .await?;

                let prompt = connection
                    .send_request(PromptRequest::new(
                        session.session_id.clone(),
                        vec![ContentBlock::Text(TextContent::new(
                            "Wait for cancellation from the editor.",
                        ))],
                    ))
                    .block_task();
                tokio::pin!(prompt);

                tokio::time::timeout(Duration::from_secs(10), delayed_provider.request_seen)
                    .await
                    .expect("provider request did not become active")
                    .expect("provider request signal dropped");
                let cancel = connection
                    .send_request(PromptRequest::new(
                        session.session_id.clone(),
                        vec![ContentBlock::Text(TextContent::new("/stop"))],
                    ))
                    .block_task()
                    .await?;
                assert_eq!(cancel.stop_reason, StopReason::EndTurn);

                let response = tokio::time::timeout(Duration::from_secs(10), &mut prompt)
                    .await
                    .expect("cancelled ACP prompt did not terminate")?;
                assert_eq!(response.stop_reason, StopReason::Cancelled);
                Ok(())
            },
        )
        .await
        .expect("official ACP client cancelled the active subprocess run");
    delayed_provider.task.abort();

    let connection = rusqlite::Connection::open(&db_path).expect("open durable ACP database");
    let (state, completed_at): (String, Option<String>) = connection
        .query_row(
            "SELECT state, completed_at FROM runs ORDER BY created_at DESC LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("load cancelled ACP run");
    assert_eq!(state, "cancelled:cancelled by user");
    assert!(
        completed_at.is_some(),
        "cancelled ACP run must have a durable terminal timestamp"
    );

    let observed = observed.lock().expect("observed updates lock");
    assert_protocol_stdout(&observed.stdout_lines);
    assert!(
        observed
            .messages
            .iter()
            .any(|message| message.contains("Run cancelled: cancelled by user")),
        "cancelled run update was not emitted: {:?}",
        observed.messages
    );
    assert!(
        observed
            .messages
            .iter()
            .any(|message| message.contains("Cancellation requested for the active editor prompt")),
        "slash-command cancellation acknowledgement was not emitted: {:?}",
        observed.messages
    );
}

#[tokio::test]
async fn provider_failure_is_redacted_and_keeps_stdout_protocol_only() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let db_path = temp.path().join("polkagent.db");
    let config_path = temp.path().join("polkagent.toml");
    let project_path = temp.path().to_path_buf();
    let binary = env!("CARGO_BIN_EXE_polkagent");
    let raw_secret = "sk-acpProviderFailureFixture123";

    assert_cli_success(
        binary,
        &db_path,
        &[
            "agent",
            "create",
            "failure-fixture",
            "--model",
            "fixture/model",
        ],
    );
    assert_cli_success(binary, &db_path, &["agent", "start", "failure-fixture"]);

    let failing_provider = failing_provider(raw_secret).await;
    write_local_provider_config(&config_path, "failing-provider", &failing_provider.base_url);

    let observed = Arc::new(Mutex::new(ObservedUpdates::default()));
    let config_arg = config_path.to_string_lossy().into_owned();
    let agent = observed_agent(
        AcpAgentConfig::new(binary)
            .args([
                "--config",
                config_arg.as_str(),
                "acp",
                "--agent",
                "failure-fixture",
                "--provider",
                "failing-provider",
                "--timeout",
                "20",
            ])
            .env(
                "POLKAGENT_DATABASE_SQLITE_PATH",
                db_path.to_string_lossy().into_owned(),
            )
            .env("ANTHROPIC_API_KEY", "")
            .env("OPENAI_API_KEY", "")
            .env("GEMINI_API_KEY", "")
            .env("GOOGLE_API_KEY", "")
            .env("OPENROUTER_API_KEY", "")
            .env("PERPLEXITY_API_KEY", "")
            .env("CEREBRAS_API_KEY", "")
            .env("OLLAMA_URL", "")
            .env("OLLAMA_MODEL", "")
            .env("POLKAGENT_ACP_FIXTURE_KEY", "fixture-key"),
        Arc::clone(&observed),
    );

    agent_client_protocol::Client
        .connect_with(
            agent,
            |connection: agent_client_protocol::ConnectionTo<Agent>| async move {
                connection
                    .send_request(InitializeRequest::new(ProtocolVersion::V1))
                    .block_task()
                    .await?;
                let session = connection
                    .send_request(NewSessionRequest::new(project_path))
                    .block_task()
                    .await?;
                let error = connection
                    .send_request(PromptRequest::new(
                        session.session_id,
                        vec![ContentBlock::Text(TextContent::new(
                            "Exercise a provider failure without leaking credentials.",
                        ))],
                    ))
                    .block_task()
                    .await
                    .expect_err("failing provider must return a JSON-RPC error");
                let detail = error
                    .data
                    .as_ref()
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default();
                assert!(detail.contains("sk-***REDACTED***"), "{detail}");
                assert!(!detail.contains(raw_secret), "{detail}");
                Ok(())
            },
        )
        .await
        .expect("official ACP client observed the provider failure safely");
    failing_provider
        .task
        .await
        .expect("failing provider task completed");

    let observed = observed.lock().expect("observed updates lock");
    assert_protocol_stdout(&observed.stdout_lines);
    let protocol_output = observed.stdout_lines.join("\n");
    assert!(protocol_output.contains("sk-***REDACTED***"));
    assert!(!protocol_output.contains(raw_secret));
    assert!(
        observed
            .stderr_lines
            .iter()
            .all(|line| !line.contains(raw_secret)),
        "provider secret leaked to stderr: {:?}",
        observed.stderr_lines
    );
}

#[test]
fn explicit_config_startup_failure_keeps_stdout_empty() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let db_path = temp.path().join("polkagent.db");
    let missing_config = temp.path().join("missing-polkagent.toml");
    let log_path = temp.path().join("startup-failure.jsonl");
    let binary = env!("CARGO_BIN_EXE_polkagent");

    let output = Command::new(binary)
        .args([
            "--log-file",
            log_path.to_string_lossy().as_ref(),
            "--config",
            missing_config.to_string_lossy().as_ref(),
            "acp",
        ])
        .env("POLKAGENT_DATABASE_SQLITE_PATH", &db_path)
        .output()
        .expect("run ACP with a missing explicit config");

    assert_eq!(output.status.code(), Some(4));
    assert!(
        output.stdout.is_empty(),
        "ACP startup failure contaminated stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Error: building shared ACP runtime"));
    assert!(stderr.contains("failed to load configuration from"));
    let diagnostics = std::fs::read_to_string(log_path).expect("read startup diagnostics");
    assert!(diagnostics.contains("\"event\":\"acp.backend_initialization_failed\""));
    assert!(diagnostics.contains("\"event\":\"acp.server_failed\""));
    assert!(!diagnostics.contains("missing-polkagent.toml"));
}

#[test]
fn unavailable_explicit_provider_fails_before_protocol_stdout() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let db_path = temp.path().join("polkagent.db");
    let config_path = temp.path().join("polkagent.toml");
    let binary = env!("CARGO_BIN_EXE_polkagent");
    std::fs::write(&config_path, "").expect("write empty provider config");

    let output = Command::new(binary)
        .args([
            "--config",
            config_path.to_string_lossy().as_ref(),
            "acp",
            "--provider",
            "definitely-unavailable",
        ])
        .env("POLKAGENT_DATABASE_SQLITE_PATH", &db_path)
        .env("ANTHROPIC_API_KEY", "")
        .env("OPENAI_API_KEY", "")
        .env("GEMINI_API_KEY", "")
        .env("GOOGLE_API_KEY", "")
        .env("OPENROUTER_API_KEY", "")
        .env("PERPLEXITY_API_KEY", "")
        .env("CEREBRAS_API_KEY", "")
        .env("OLLAMA_URL", "")
        .env("OLLAMA_MODEL", "")
        .output()
        .expect("run ACP with an unavailable explicit provider");

    assert!(!output.status.success());
    assert!(
        output.stdout.is_empty(),
        "ACP provider startup failure contaminated stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Provider 'definitely-unavailable' was explicitly requested"));
    assert!(stderr.contains("could not be found"));
}

fn observed_agent(config: AcpAgentConfig, observed: Arc<Mutex<ObservedUpdates>>) -> AcpAgent {
    AcpAgent::new(config).with_debug(move |line, direction| {
        let mut observed = observed.lock().expect("observed updates lock");
        match direction {
            LineDirection::Stdout => observed.stdout_lines.push(line.to_owned()),
            LineDirection::Stderr => observed.stderr_lines.push(line.to_owned()),
            LineDirection::Stdin => {}
        }
    })
}

fn assert_protocol_stdout(lines: &[String]) {
    assert!(
        !lines.is_empty(),
        "ACP subprocess emitted no protocol stdout"
    );
    for line in lines {
        let frame = serde_json::from_str::<serde_json::Value>(line)
            .unwrap_or_else(|error| panic!("non-JSON ACP stdout line {line:?}: {error}"));
        assert_eq!(
            frame.get("jsonrpc").and_then(serde_json::Value::as_str),
            Some("2.0"),
            "ACP stdout was JSON but not a JSON-RPC 2.0 frame: {line}"
        );
    }
}

async fn delayed_provider() -> DelayedProvider {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind delayed provider");
    let address = listener.local_addr().expect("delayed provider address");
    let (request_tx, request_rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept provider request");
        let mut reader = BufReader::new(stream);
        let mut request_line = String::new();
        reader
            .read_line(&mut request_line)
            .await
            .expect("read provider request line");
        assert!(
            request_line.starts_with("POST /v1/chat/completions "),
            "unexpected provider request: {request_line}"
        );
        request_tx.send(()).expect("signal active provider request");
        std::future::pending::<()>().await;
        drop(reader);
    });
    DelayedProvider {
        base_url: format!("http://{address}/v1"),
        request_seen: request_rx,
        task,
    }
}

async fn failing_provider(raw_secret: &str) -> FailingProvider {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind failing provider");
    let address = listener.local_addr().expect("failing provider address");
    let raw_secret = raw_secret.to_owned();
    let task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept provider request");
        let mut reader = BufReader::new(stream);
        let mut request_line = String::new();
        reader
            .read_line(&mut request_line)
            .await
            .expect("read provider request line");
        assert!(
            request_line.starts_with("POST /v1/chat/completions "),
            "unexpected provider request: {request_line}"
        );

        let body = format!(r#"{{"error":{{"message":"credential rejected: {raw_secret}"}}}}"#);
        let response = format!(
            "HTTP/1.1 401 Unauthorized\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        reader
            .get_mut()
            .write_all(response.as_bytes())
            .await
            .expect("write failing provider response");
        reader
            .get_mut()
            .shutdown()
            .await
            .expect("close failing provider response");
    });
    FailingProvider {
        base_url: format!("http://{address}/v1"),
        task,
    }
}

fn assert_cli_success(binary: &str, db_path: &std::path::Path, args: &[&str]) {
    let output = Command::new(binary)
        .args(args)
        .env("POLKAGENT_DATABASE_SQLITE_PATH", db_path)
        .output()
        .expect("run Polkagent fixture command");
    assert!(
        output.status.success(),
        "polkagent {} failed\nstdout: {}\nstderr: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
