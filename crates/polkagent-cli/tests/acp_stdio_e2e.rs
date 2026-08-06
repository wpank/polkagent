//! Executable ACP proof through the real `polkagent acp` subprocess.

// End-to-end assertions unwrap controlled process and protocol fixtures.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::fs::OpenOptions;
use std::io::Write as _;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_client_protocol::schema::v1::{
    ContentBlock, InitializeRequest, LoadSessionRequest, NewSessionRequest, PromptRequest,
    PromptResponse, ResumeSessionRequest, SessionConfigKind, SessionConfigOption,
    SessionConfigSelectOptions, SessionNotification, SessionUpdate, SetSessionConfigOptionRequest,
    StopReason, TextContent,
};
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::{AcpAgent, AcpAgentConfig, Agent, LineDirection};
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader};

#[derive(Default)]
struct ObservedUpdates {
    command_names: Vec<String>,
    messages: Vec<String>,
    usage_updates: Vec<(u64, u64)>,
    timeline: Vec<String>,
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

struct RecordingProvider {
    base_url: String,
    requests: tokio::sync::mpsc::Receiver<serde_json::Value>,
    task: tokio::task::JoinHandle<()>,
}

struct DelayedSuccessProvider {
    base_url: String,
    model: tokio::sync::oneshot::Receiver<String>,
    task: tokio::task::JoinHandle<()>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DurablePromptIdentity {
    conversation_id: String,
    turn_id: String,
    run_ids: Vec<String>,
    checkpoint: u64,
}

fn durable_work_counts(db_path: &std::path::Path, conversation_id: &str) -> (i64, i64, i64) {
    let connection = rusqlite::Connection::open(db_path).expect("open durable work database");
    let turns = connection
        .query_row(
            "SELECT COUNT(*) FROM interaction_turns WHERE conversation_id = ?1",
            [conversation_id],
            |row| row.get(0),
        )
        .expect("count durable interaction turns");
    let runs = connection
        .query_row(
            "SELECT COUNT(*) FROM runs WHERE conversation_id = ?1",
            [conversation_id],
            |row| row.get(0),
        )
        .expect("count durable interaction runs");
    let events = connection
        .query_row(
            "SELECT COUNT(*) FROM interaction_events WHERE conversation_id = ?1",
            [conversation_id],
            |row| row.get(0),
        )
        .expect("count durable interaction events");
    (turns, runs, events)
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

fn write_model_selection_config(path: &std::path::Path, base_url: &str) {
    std::fs::write(
        path,
        format!(
            "[[providers]]\n\
             id = \"dynamic-provider\"\n\
             provider_type = \"local\"\n\
             base_url = \"{base_url}\"\n\
             api_key_env = \"POLKAGENT_ACP_FIXTURE_KEY\"\n\
             default_model = \"fixture-default\"\n\
             \n\
             [[providers]]\n\
             id = \"other-provider\"\n\
             provider_type = \"local\"\n\
             base_url = \"{base_url}\"\n\
             api_key_env = \"POLKAGENT_ACP_FIXTURE_KEY\"\n\
             default_model = \"foreign-model\"\n\
             \n\
             [[models]]\n\
             slug = \"fixture-alternate\"\n\
             provider = \"dynamic-provider\"\n\
             \n\
             [[models]]\n\
             slug = \"foreign-model\"\n\
             provider = \"other-provider\"\n"
        ),
    )
    .expect("write dynamic model provider config");
}

fn write_progressive_provider_config(path: &std::path::Path, base_url: &str) {
    std::fs::write(
        path,
        format!(
            "[[providers]]\n\
             id = \"progressive-provider\"\n\
             provider_type = \"local\"\n\
             base_url = \"{base_url}\"\n\
             api_key_env = \"POLKAGENT_ACP_FIXTURE_KEY\"\n\
             default_model = \"claude-sonnet-4-6\"\n"
        ),
    )
    .expect("write progressive provider config");
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
    let inspected_run_id = Arc::new(Mutex::new(None::<String>));
    let inspected_run_id_by_client = Arc::clone(&inspected_run_id);
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

                for command in ["/commands stop", "/st", "/agents", "/use editor-fixture"] {
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
                        session.session_id.clone(),
                        vec![ContentBlock::Text(TextContent::new(
                            "Return a short editor integration greeting.",
                        ))],
                    ))
                    .block_task()
                    .await?;
                assert_eq!(run.stop_reason, StopReason::EndTurn);
                let identity = durable_prompt_identity(&run);
                assert_eq!(identity.run_ids.len(), 1);
                let run_id = identity.run_ids[0].clone();
                for command in ["/runs".to_owned(), format!("/inspect {run_id}")] {
                    let response = connection
                        .send_request(PromptRequest::new(
                            session.session_id.clone(),
                            vec![ContentBlock::Text(TextContent::new(command))],
                        ))
                        .block_task()
                        .await?;
                    assert_eq!(response.stop_reason, StopReason::EndTurn);
                }
                *inspected_run_id_by_client
                    .lock()
                    .expect("inspected run ID lock") = Some(run_id);
                Ok(())
            },
        )
        .await
        .expect("official ACP client completed the subprocess session");

    let observed = observed.lock().expect("observed updates lock");
    assert_editor_command_updates(&observed);
    let inspected_run_id = inspected_run_id
        .lock()
        .expect("inspected run ID lock")
        .clone()
        .expect("inspected run identity");
    assert!(
        observed
            .messages
            .iter()
            .filter(|message| message.contains(&inspected_run_id))
            .count()
            >= 2,
        "run list and inspection did not retain the durable ID: {:?}",
        observed.messages
    );
    assert_safe_success_diagnostics(&log_path);
}

#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "one official-client scenario must retain identities across three real ACP subprocess lifetimes to prove retry, load replay, resume, and exact database correlation end to end"
)]
async fn official_client_retries_and_loads_the_same_durable_interaction_after_restart() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let db_path = temp.path().join("polkagent.db");
    let project_path = temp.path().to_path_buf();
    let binary = env!("CARGO_BIN_EXE_polkagent");
    create_active_agent(binary, &db_path, "durable-acp", "fake/test");

    let first_observed = Arc::new(Mutex::new(ObservedUpdates::default()));
    let first_identity = Arc::new(Mutex::new(None));
    let first_identity_by_client = Arc::clone(&first_identity);
    let first_agent = observed_agent(
        AcpAgentConfig::new(binary)
            .args(["acp", "--agent", "durable-acp"])
            .env(
                "POLKAGENT_DATABASE_SQLITE_PATH",
                db_path.to_string_lossy().into_owned(),
            ),
        Arc::clone(&first_observed),
    );
    let first_turn_id = uuid::Uuid::now_v7().to_string();
    let first_turn_id_for_client = first_turn_id.clone();

    agent_client_protocol::Client
        .connect_with(
            first_agent,
            |connection: agent_client_protocol::ConnectionTo<Agent>| async move {
                let initialized = connection
                    .send_request(InitializeRequest::new(ProtocolVersion::V1))
                    .block_task()
                    .await?;
                assert!(initialized.agent_capabilities.load_session);
                let session = connection
                    .send_request(NewSessionRequest::new(project_path))
                    .block_task()
                    .await?;
                assert!(session.session_id.0.as_ref().parse::<uuid::Uuid>().is_ok());
                connection
                    .send_request(SetSessionConfigOptionRequest::new(
                        session.session_id.clone(),
                        "model",
                        "fake/test",
                    ))
                    .block_task()
                    .await?;
                let request = prompt_with_turn_id(
                    &session.session_id,
                    "First durable ACP turn.",
                    &first_turn_id_for_client,
                );
                let first = connection
                    .send_request(request.clone())
                    .block_task()
                    .await?;
                let retry = connection.send_request(request).block_task().await?;
                let first = durable_prompt_identity(&first);
                assert_eq!(durable_prompt_identity(&retry), first);
                let conflict = connection
                    .send_request(prompt_with_turn_id(
                        &session.session_id,
                        "Conflicting reuse of a durable ACP turn.",
                        &first_turn_id_for_client,
                    ))
                    .block_task()
                    .await
                    .expect_err("conflicting turn retry must fail");
                assert_eq!(
                    conflict.code,
                    agent_client_protocol::Error::invalid_request().code
                );
                assert!(
                    conflict
                        .data
                        .as_ref()
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|detail| detail.contains("conflict")),
                    "unexpected retry conflict: {conflict:?}"
                );
                assert_eq!(first.conversation_id, session.session_id.0.as_ref());
                assert_eq!(first.turn_id, first_turn_id_for_client);
                *first_identity_by_client
                    .lock()
                    .expect("first prompt identity lock") = Some(first);
                Ok(())
            },
        )
        .await
        .expect("official ACP client completed durable first process");

    let first = first_identity
        .lock()
        .expect("first prompt identity lock")
        .clone()
        .expect("first durable identity");
    assert_eq!(first.run_ids.len(), 1);

    let loaded_user_messages = Arc::new(Mutex::new(Vec::new()));
    let loaded_agent_messages = Arc::new(Mutex::new(Vec::new()));
    let users_by_client = Arc::clone(&loaded_user_messages);
    let agents_by_client = Arc::clone(&loaded_agent_messages);
    let second_identity = Arc::new(Mutex::new(None));
    let second_identity_by_client = Arc::clone(&second_identity);
    let second_observed = Arc::new(Mutex::new(ObservedUpdates::default()));
    let second_agent = observed_agent(
        AcpAgentConfig::new(binary)
            .args(["acp", "--agent", "durable-acp"])
            .env(
                "POLKAGENT_DATABASE_SQLITE_PATH",
                db_path.to_string_lossy().into_owned(),
            ),
        Arc::clone(&second_observed),
    );
    let session_id =
        agent_client_protocol::schema::v1::SessionId::new(first.conversation_id.clone());
    let second_turn_id = uuid::Uuid::now_v7().to_string();
    let second_turn_id_for_client = second_turn_id.clone();
    let first_conversation_id = first.conversation_id.clone();
    let first_run_id_for_client = first.run_ids[0].clone();
    let second_project_path = temp.path().to_path_buf();

    agent_client_protocol::Client
        .builder()
        .on_receive_notification(
            async move |notification: SessionNotification, _connection| {
                match notification.update {
                    SessionUpdate::UserMessageChunk(chunk) => {
                        if let ContentBlock::Text(text) = chunk.content {
                            users_by_client
                                .lock()
                                .expect("loaded users lock")
                                .push(text.text);
                        }
                    }
                    SessionUpdate::AgentMessageChunk(chunk) => {
                        if let ContentBlock::Text(text) = chunk.content {
                            agents_by_client
                                .lock()
                                .expect("loaded agents lock")
                                .push(text.text);
                        }
                    }
                    _ => {}
                }
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .connect_with(
            second_agent,
            |connection: agent_client_protocol::ConnectionTo<Agent>| async move {
                connection
                    .send_request(InitializeRequest::new(ProtocolVersion::V1))
                    .block_task()
                    .await?;
                let loaded = connection
                    .send_request(LoadSessionRequest::new(
                        session_id.clone(),
                        second_project_path,
                    ))
                    .block_task()
                    .await?;
                assert_eq!(
                    config_current(
                        loaded
                            .config_options
                            .as_deref()
                            .expect("loaded durable config options"),
                        "model",
                    ),
                    "fake/test"
                );
                for command in [
                    "/runs".to_owned(),
                    format!("/inspect {first_run_id_for_client}"),
                ] {
                    let response = connection
                        .send_request(PromptRequest::new(
                            session_id.clone(),
                            vec![ContentBlock::Text(TextContent::new(command))],
                        ))
                        .block_task()
                        .await?;
                    assert_eq!(response.stop_reason, StopReason::EndTurn);
                }
                let follow_up = connection
                    .send_request(prompt_with_turn_id(
                        &session_id,
                        "Follow up after ACP restart.",
                        &second_turn_id_for_client,
                    ))
                    .block_task()
                    .await?;
                let identity = durable_prompt_identity(&follow_up);
                assert_eq!(identity.conversation_id, first_conversation_id);
                assert_eq!(identity.turn_id, second_turn_id_for_client);
                *second_identity_by_client
                    .lock()
                    .expect("second prompt identity lock") = Some(identity);
                Ok(())
            },
        )
        .await
        .expect("official ACP client loaded durable session after restart");

    let second = second_identity
        .lock()
        .expect("second prompt identity lock")
        .clone()
        .expect("second durable identity");
    assert_eq!(second.run_ids.len(), 1);
    assert_ne!(first.turn_id, second.turn_id);
    assert_ne!(first.run_ids, second.run_ids);
    assert!(loaded_user_messages
        .lock()
        .expect("loaded users lock")
        .iter()
        .any(|text| text == "First durable ACP turn."));
    assert!(!loaded_agent_messages
        .lock()
        .expect("loaded agents lock")
        .is_empty());
    assert!(
        loaded_agent_messages
            .lock()
            .expect("loaded agents lock")
            .iter()
            .filter(|text| text.contains(&first.run_ids[0]))
            .count()
            >= 2,
        "restarted ACP process did not list and inspect the first durable run"
    );

    let resumed_user_chunks = Arc::new(Mutex::new(0_usize));
    let resumed_user_chunks_by_client = Arc::clone(&resumed_user_chunks);
    let third_observed = Arc::new(Mutex::new(ObservedUpdates::default()));
    let third_agent = observed_agent(
        AcpAgentConfig::new(binary)
            .args(["acp", "--agent", "durable-acp"])
            .env(
                "POLKAGENT_DATABASE_SQLITE_PATH",
                db_path.to_string_lossy().into_owned(),
            ),
        Arc::clone(&third_observed),
    );
    let resumed_session_id =
        agent_client_protocol::schema::v1::SessionId::new(first.conversation_id.clone());
    let resume_project_path = temp.path().to_path_buf();
    agent_client_protocol::Client
        .builder()
        .on_receive_notification(
            async move |notification: SessionNotification, _connection| {
                if matches!(notification.update, SessionUpdate::UserMessageChunk(_)) {
                    *resumed_user_chunks_by_client
                        .lock()
                        .expect("resume user chunk lock") += 1;
                }
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .connect_with(
            third_agent,
            |connection: agent_client_protocol::ConnectionTo<Agent>| async move {
                let initialized = connection
                    .send_request(InitializeRequest::new(ProtocolVersion::V1))
                    .block_task()
                    .await?;
                assert!(initialized
                    .agent_capabilities
                    .session_capabilities
                    .resume
                    .is_some());
                let resumed = connection
                    .send_request(ResumeSessionRequest::new(
                        resumed_session_id,
                        resume_project_path,
                    ))
                    .block_task()
                    .await?;
                assert!(resumed.config_options.is_some());
                Ok(())
            },
        )
        .await
        .expect("official ACP client resumed durable session without replay");
    assert_eq!(
        *resumed_user_chunks.lock().expect("resume user chunk lock"),
        0,
        "session/resume must not replay transcript chunks"
    );

    let connection = rusqlite::Connection::open(&db_path).expect("open durable ACP database");
    let durable_rows: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM interaction_turns t \
             JOIN interaction_turn_runs r ON r.turn_id = t.id \
             WHERE t.conversation_id = ?1 \
               AND ((t.id = ?2 AND r.run_id = ?3) OR (t.id = ?4 AND r.run_id = ?5))",
            rusqlite::params![
                first.conversation_id,
                first.turn_id,
                first.run_ids[0],
                second.turn_id,
                second.run_ids[0],
            ],
            |row| row.get(0),
        )
        .expect("count exact durable ACP correlations");
    assert_eq!(durable_rows, 2);
    let interaction_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM interaction_sessions WHERE conversation_id = ?1",
            [&first.conversation_id],
            |row| row.get(0),
        )
        .expect("count durable ACP interaction");
    assert_eq!(interaction_count, 1);
    assert_protocol_stdout(
        &first_observed
            .lock()
            .expect("first observed lock")
            .stdout_lines,
    );
    assert_protocol_stdout(
        &second_observed
            .lock()
            .expect("second observed lock")
            .stdout_lines,
    );
    assert_protocol_stdout(
        &third_observed
            .lock()
            .expect("third observed lock")
            .stdout_lines,
    );
}

#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "the official-client proof spans fresh, cross-workspace, exact-restart, and legacy ACP processes"
)]
async fn official_client_enforces_durable_workspace_origin_before_load_or_resume() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let db_path = temp.path().join("polkagent.db");
    let workspace_alpha = temp.path().join("workspace-alpha");
    let workspace_beta = temp.path().join("workspace-beta");
    std::fs::create_dir(&workspace_alpha).expect("create alpha workspace");
    std::fs::create_dir(&workspace_beta).expect("create beta workspace");
    let binary = env!("CARGO_BIN_EXE_polkagent");
    create_active_agent(binary, &db_path, "origin-acp", "fake/test");

    let identity = Arc::new(Mutex::new(None::<String>));
    let identity_by_client = Arc::clone(&identity);
    let first_observed = Arc::new(Mutex::new(ObservedUpdates::default()));
    let first_agent = observed_agent(
        AcpAgentConfig::new(binary)
            .args(["acp", "--agent", "origin-acp"])
            .env(
                "POLKAGENT_DATABASE_SQLITE_PATH",
                db_path.to_string_lossy().into_owned(),
            ),
        Arc::clone(&first_observed),
    );
    let first_workspace = workspace_alpha.clone();
    agent_client_protocol::Client
        .connect_with(
            first_agent,
            |connection: agent_client_protocol::ConnectionTo<Agent>| async move {
                connection
                    .send_request(InitializeRequest::new(ProtocolVersion::V1))
                    .block_task()
                    .await?;
                let session = connection
                    .send_request(NewSessionRequest::new(first_workspace))
                    .block_task()
                    .await?;
                connection
                    .send_request(PromptRequest::new(
                        session.session_id.clone(),
                        vec![ContentBlock::Text(TextContent::new(
                            "Bind this session to workspace alpha.",
                        ))],
                    ))
                    .block_task()
                    .await?;
                *identity_by_client.lock().expect("session identity lock") =
                    Some(session.session_id.0.as_ref().to_owned());
                Ok(())
            },
        )
        .await
        .expect("create origin-bound ACP session");
    let session_id = identity
        .lock()
        .expect("session identity lock")
        .clone()
        .expect("durable ACP session identity");
    let stored_origin: String = rusqlite::Connection::open(&db_path)
        .expect("open origin database")
        .query_row(
            "SELECT origin_working_directory FROM interaction_sessions
             WHERE conversation_id = ?1",
            [&session_id],
            |row| row.get(0),
        )
        .expect("load exact stored origin");
    assert_eq!(stored_origin, workspace_alpha.to_string_lossy());
    let before_attach_attempts = durable_work_counts(&db_path, &session_id);

    let second_observed = Arc::new(Mutex::new(ObservedUpdates::default()));
    let second_agent = observed_agent(
        AcpAgentConfig::new(binary)
            .args(["acp", "--agent", "origin-acp"])
            .env(
                "POLKAGENT_DATABASE_SQLITE_PATH",
                db_path.to_string_lossy().into_owned(),
            ),
        Arc::clone(&second_observed),
    );
    let exact_session = agent_client_protocol::schema::v1::SessionId::new(session_id.clone());
    let exact_workspace = workspace_alpha.clone();
    let other_workspace = workspace_beta.clone();
    let traversal_workspace = workspace_alpha.join("nested/../other");
    agent_client_protocol::Client
        .connect_with(
            second_agent,
            |connection: agent_client_protocol::ConnectionTo<Agent>| async move {
                connection
                    .send_request(InitializeRequest::new(ProtocolVersion::V1))
                    .block_task()
                    .await?;
                for error in [
                    connection
                        .send_request(LoadSessionRequest::new(
                            exact_session.clone(),
                            other_workspace.clone(),
                        ))
                        .block_task()
                        .await
                        .expect_err("workspace beta must not cross-load alpha"),
                    connection
                        .send_request(ResumeSessionRequest::new(
                            exact_session.clone(),
                            other_workspace,
                        ))
                        .block_task()
                        .await
                        .expect_err("workspace beta must not cross-resume alpha"),
                ] {
                    assert_eq!(
                        error.code,
                        agent_client_protocol::Error::invalid_request().code
                    );
                }
                let traversal = connection
                    .send_request(LoadSessionRequest::new(
                        exact_session.clone(),
                        traversal_workspace,
                    ))
                    .block_task()
                    .await
                    .expect_err("traversal cwd must fail before backend replay");
                assert_eq!(
                    traversal.code,
                    agent_client_protocol::Error::invalid_params().code
                );
                let relative = connection
                    .send_request(ResumeSessionRequest::new(
                        exact_session.clone(),
                        std::path::PathBuf::from("relative/workspace"),
                    ))
                    .block_task()
                    .await
                    .expect_err("relative cwd must fail before backend replay");
                assert_eq!(
                    relative.code,
                    agent_client_protocol::Error::invalid_params().code
                );
                connection
                    .send_request(LoadSessionRequest::new(exact_session, exact_workspace))
                    .block_task()
                    .await?;
                Ok(())
            },
        )
        .await
        .expect("exact workspace loads after rejected cross-workspace requests");
    assert_eq!(
        durable_work_counts(&db_path, &session_id),
        before_attach_attempts,
        "failed load/resume requests must not append events, turns, or runs"
    );

    {
        let connection = rusqlite::Connection::open(&db_path).expect("open legacy fixture db");
        connection
            .execute_batch("DROP TRIGGER trg_interaction_origin_immutable;")
            .expect("allow exact legacy fixture mutation");
        connection
            .execute(
                "UPDATE interaction_sessions SET origin_working_directory = NULL
                 WHERE conversation_id = ?1",
                [&session_id],
            )
            .expect("model interaction created before cwd provenance");
    }
    let legacy_agent = observed_agent(
        AcpAgentConfig::new(binary)
            .args(["acp", "--agent", "origin-acp"])
            .env(
                "POLKAGENT_DATABASE_SQLITE_PATH",
                db_path.to_string_lossy().into_owned(),
            ),
        Arc::new(Mutex::new(ObservedUpdates::default())),
    );
    let legacy_session = agent_client_protocol::schema::v1::SessionId::new(session_id.clone());
    agent_client_protocol::Client
        .connect_with(
            legacy_agent,
            |connection: agent_client_protocol::ConnectionTo<Agent>| async move {
                connection
                    .send_request(InitializeRequest::new(ProtocolVersion::V1))
                    .block_task()
                    .await?;
                let load_error = connection
                    .send_request(LoadSessionRequest::new(
                        legacy_session.clone(),
                        workspace_alpha.clone(),
                    ))
                    .block_task()
                    .await
                    .expect_err("legacy load must not claim an unproven cwd");
                let resume_error = connection
                    .send_request(ResumeSessionRequest::new(legacy_session, workspace_alpha))
                    .block_task()
                    .await
                    .expect_err("legacy resume must not claim an unproven cwd");
                for error in [load_error, resume_error] {
                    assert_eq!(
                        error.code,
                        agent_client_protocol::Error::invalid_request().code
                    );
                    assert!(error
                        .data
                        .as_ref()
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(
                            |detail| detail.contains("no durable working-directory provenance")
                        ));
                }
                Ok(())
            },
        )
        .await
        .expect("legacy ACP load failed closed without terminating protocol");
    assert_eq!(
        durable_work_counts(&db_path, &session_id),
        before_attach_attempts
    );
}

#[tokio::test]
async fn official_client_configures_agent_and_model_for_the_next_real_run() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let db_path = temp.path().join("polkagent.db");
    let config_path = temp.path().join("polkagent.toml");
    let binary = env!("CARGO_BIN_EXE_polkagent");
    let first_id = create_active_agent(
        binary,
        &db_path,
        "config-first",
        "dynamic-provider/fixture-default",
    );
    let second_id = create_active_agent(
        binary,
        &db_path,
        "config-second",
        "dynamic-provider/fixture-default",
    );
    let mut provider = recording_provider(2).await;
    write_model_selection_config(&config_path, &provider.base_url);

    let observed = Arc::new(Mutex::new(ObservedUpdates::default()));
    let observed_by_client = Arc::clone(&observed);
    let first_id_for_client = first_id.clone();
    let second_id_for_client = second_id.clone();
    let project_path = temp.path().to_path_buf();
    let config_arg = config_path.to_string_lossy().into_owned();
    let agent = observed_agent(
        AcpAgentConfig::new(binary)
            .args([
                "--config",
                config_arg.as_str(),
                "acp",
                "--agent",
                "config-first",
                "--provider",
                "dynamic-provider",
            ])
            .env(
                "POLKAGENT_DATABASE_SQLITE_PATH",
                db_path.to_string_lossy().into_owned(),
            )
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
                exercise_session_configuration(
                    &connection,
                    project_path,
                    &first_id_for_client,
                    second_id_for_client,
                )
                .await
            },
        )
        .await
        .expect("official ACP client configured a real subprocess session");

    let first_request = provider
        .requests
        .recv()
        .await
        .expect("first recorded request");
    let second_request = provider
        .requests
        .recv()
        .await
        .expect("second recorded request");
    provider.task.await.expect("recording provider completed");
    assert_request_model(
        [&first_request, &second_request],
        "Use the native model option.",
        "dynamic-provider/fixture-alternate",
    );
    assert_request_model(
        [&first_request, &second_request],
        "Use the slash model option.",
        "dynamic-provider/fixture-default",
    );
    assert_runs_used_agent(&db_path, &first_id, 2, 1);
    assert_runs_used_agent(&db_path, &second_id, 2, 1);
    let observed = observed.lock().expect("observed updates lock");
    assert_protocol_stdout(&observed.stdout_lines);
    assert!(observed
        .messages
        .iter()
        .any(|message| message.contains("Configured model 'missing-model' is unavailable")));
    assert!(observed
        .messages
        .iter()
        .any(|message| { message.contains("Selected model 'dynamic-provider/fixture-default'") }));
}

#[tokio::test]
async fn official_client_receives_runtime_text_and_usage_before_prompt_completion() {
    const EXPECTED_TEXT: &str = "A real progressive runtime response.";

    let temp = tempfile::tempdir().expect("temporary directory");
    let db_path = temp.path().join("polkagent.db");
    let config_path = temp.path().join("polkagent.toml");
    let binary = env!("CARGO_BIN_EXE_polkagent");
    create_active_agent(binary, &db_path, "progressive-fixture", "claude-sonnet-4-6");
    let provider = delayed_success_provider(EXPECTED_TEXT).await;
    write_progressive_provider_config(&config_path, &provider.base_url);

    let observed = Arc::new(Mutex::new(ObservedUpdates::default()));
    let observed_by_client = Arc::clone(&observed);
    let observed_at_terminal = Arc::clone(&observed);
    let project_path = temp.path().to_path_buf();
    let config_arg = config_path.to_string_lossy().into_owned();
    let agent = observed_agent(
        AcpAgentConfig::new(binary)
            .args([
                "--config",
                config_arg.as_str(),
                "acp",
                "--agent",
                "progressive-fixture",
                "--provider",
                "progressive-provider",
            ])
            .env(
                "POLKAGENT_DATABASE_SQLITE_PATH",
                db_path.to_string_lossy().into_owned(),
            )
            .env("POLKAGENT_ACP_FIXTURE_KEY", "fixture-key"),
        Arc::clone(&observed),
    );

    agent_client_protocol::Client
        .builder()
        .on_receive_notification(
            async move |notification: SessionNotification, _connection| {
                let mut observed = observed_by_client.lock().expect("observed updates lock");
                match notification.update {
                    SessionUpdate::AgentMessageChunk(chunk) => {
                        if let ContentBlock::Text(text) = chunk.content {
                            observed.messages.push(text.text);
                            observed.timeline.push("text_chunk".to_owned());
                        }
                    }
                    SessionUpdate::UsageUpdate(usage) => {
                        observed.usage_updates.push((usage.used, usage.size));
                        observed.timeline.push("usage".to_owned());
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
                connection
                    .send_request(InitializeRequest::new(ProtocolVersion::V1))
                    .block_task()
                    .await?;
                let session = connection
                    .send_request(NewSessionRequest::new(project_path))
                    .block_task()
                    .await?;
                let response = connection
                    .send_request(PromptRequest::new(
                        session.session_id,
                        vec![ContentBlock::Text(TextContent::new(
                            "Prove progressive ACP delivery.",
                        ))],
                    ))
                    .block_task()
                    .await?;
                assert_eq!(response.stop_reason, StopReason::EndTurn);
                observed_at_terminal
                    .lock()
                    .expect("observed updates lock")
                    .timeline
                    .push("terminal".to_owned());
                Ok(())
            },
        )
        .await
        .expect("official ACP client received progressive runtime updates");
    let requested_model = provider.model.await.expect("record delayed provider model");
    provider.task.await.expect("delayed provider completed");
    assert_eq!(requested_model, "claude-sonnet-4-6");

    let durable_usage: (i64, i64) = rusqlite::Connection::open(&db_path)
        .expect("open progressive ACP database")
        .query_row(
            "SELECT input_tokens, output_tokens FROM turns ORDER BY completed_at DESC LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("load progressive turn usage");
    assert_eq!(durable_usage, (7, 3));
    let observed = observed.lock().expect("observed updates lock");
    assert_eq!(observed.messages.concat(), EXPECTED_TEXT);
    assert_eq!(
        observed.usage_updates,
        vec![(10, 200_000)],
        "timeline={:?}; stdout={:?}; stderr={:?}",
        observed.timeline,
        observed.stdout_lines,
        observed.stderr_lines
    );
    let text_index = observed
        .timeline
        .iter()
        .position(|event| event == "text_chunk")
        .expect("text update timeline entry");
    let usage_index = observed
        .timeline
        .iter()
        .position(|event| event == "usage")
        .expect("usage timeline entry");
    let terminal_index = observed
        .timeline
        .iter()
        .position(|event| event == "terminal")
        .expect("terminal timeline entry");
    assert!(text_index < usage_index);
    assert!(usage_index < terminal_index);
    assert_protocol_stdout(&observed.stdout_lines);
}

async fn exercise_session_configuration(
    connection: &agent_client_protocol::ConnectionTo<Agent>,
    project_path: std::path::PathBuf,
    first_id: &str,
    second_id: String,
) -> Result<(), agent_client_protocol::Error> {
    connection
        .send_request(InitializeRequest::new(ProtocolVersion::V1))
        .block_task()
        .await?;
    let native_session = connection
        .send_request(NewSessionRequest::new(project_path.clone()))
        .block_task()
        .await?;
    let options = native_session
        .config_options
        .as_deref()
        .expect("ACP config options are advertised");
    assert_config_discovery(options, first_id, &second_id);

    assert_config_rejected(
        connection,
        &native_session.session_id,
        "provider",
        "dynamic-provider",
        "unsupported session config option",
    )
    .await;
    assert_config_rejected(
        connection,
        &native_session.session_id,
        "polkagent.agent",
        "missing-agent",
        "active agent config value",
    )
    .await;
    let changed_agent = connection
        .send_request(SetSessionConfigOptionRequest::new(
            native_session.session_id.clone(),
            "polkagent.agent",
            agent_client_protocol::schema::v1::SessionConfigValueId::new(second_id.clone()),
        ))
        .block_task()
        .await?;
    assert_eq!(
        config_current(&changed_agent.config_options, "polkagent.agent"),
        second_id
    );
    assert_config_rejected(
        connection,
        &native_session.session_id,
        "model",
        "missing-model",
        "model config value",
    )
    .await;
    assert_config_rejected(
        connection,
        &native_session.session_id,
        "model",
        "foreign-model",
        "provider switching is not supported",
    )
    .await;
    let changed_model = connection
        .send_request(SetSessionConfigOptionRequest::new(
            native_session.session_id.clone(),
            "model",
            "fixture-alternate",
        ))
        .block_task()
        .await?;
    assert_eq!(
        config_current(&changed_model.config_options, "model"),
        "dynamic-provider/fixture-alternate"
    );
    assert!(config_values(&changed_model.config_options, "model")
        .iter()
        .any(|value| value == "dynamic-provider/fixture-alternate"));

    let slash_session = connection
        .send_request(NewSessionRequest::new(project_path))
        .block_task()
        .await?;
    send_text_prompt(
        connection,
        &slash_session.session_id,
        "/model missing-model",
    )
    .await?;
    send_text_prompt(
        connection,
        &slash_session.session_id,
        "/model fixture-default",
    )
    .await?;
    tokio::try_join!(
        send_text_prompt(
            connection,
            &native_session.session_id,
            "Use the native model option.",
        ),
        send_text_prompt(
            connection,
            &slash_session.session_id,
            "Use the slash model option.",
        ),
    )?;
    Ok(())
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
        vec!["help", "status", "agents", "agent", "runs", "inspect", "model", "cancel"]
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
        observed
            .messages
            .iter()
            .any(|message| message.contains("Recent runs for the selected conversation")),
        "run discovery was not streamed: {:?}",
        observed.messages
    );
    assert!(
        observed
            .messages
            .iter()
            .any(|message| message.contains("Run:") && message.contains("State:")),
        "run inspection was not streamed: {:?}",
        observed.messages
    );
    assert!(
        observed.messages.len() >= 7,
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
    assert_eq!(state, "failed:recovered after restart");
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
#[allow(
    clippy::too_many_lines,
    reason = "the official-client cancellation proof keeps provider activation, protocol cancellation, and linked durable run/interaction assertions in one subprocess scenario"
)]
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
    let interaction_state: String = connection
        .query_row(
            "SELECT t.state FROM interaction_turns t \
             JOIN interaction_turn_runs r ON r.turn_id = t.id \
             JOIN runs ON runs.id = r.run_id \
             ORDER BY runs.created_at DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .expect("load cancelled durable interaction turn");
    assert_eq!(interaction_state, "cancelled");

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

fn create_active_agent(binary: &str, db_path: &std::path::Path, name: &str, model: &str) -> String {
    assert_cli_success(
        binary,
        db_path,
        &["agent", "create", name, "--model", model],
    );
    assert_cli_success(binary, db_path, &["agent", "start", name]);
    rusqlite::Connection::open(db_path)
        .expect("open agent fixture database")
        .query_row("SELECT id FROM agents WHERE name = ?1", [name], |row| {
            row.get(0)
        })
        .expect("load active agent fixture ID")
}

fn assert_config_discovery(options: &[SessionConfigOption], first_id: &str, second_id: &str) {
    let ids = options
        .iter()
        .map(|option| option.id.0.as_ref())
        .collect::<Vec<_>>();
    assert_eq!(ids, vec!["polkagent.agent", "model"]);
    assert_eq!(config_current(options, "polkagent.agent"), first_id);
    assert_eq!(config_current(options, "model"), "_polkagent_agent_model");
    let agent_values = config_values(options, "polkagent.agent");
    assert!(agent_values.iter().any(|value| value == first_id));
    assert!(agent_values.iter().any(|value| value == second_id));
    let model_values = config_values(options, "model");
    assert!(model_values.iter().any(|value| value == "fixture-default"));
    assert!(model_values
        .iter()
        .any(|value| value == "fixture-alternate"));
}

fn config_current(options: &[SessionConfigOption], id: &str) -> String {
    let option = options
        .iter()
        .find(|option| option.id.0.as_ref() == id)
        .expect("advertised config option");
    let SessionConfigKind::Select(select) = &option.kind else {
        panic!("expected select config option")
    };
    select.current_value.0.to_string()
}

fn config_values(options: &[SessionConfigOption], id: &str) -> Vec<String> {
    let option = options
        .iter()
        .find(|option| option.id.0.as_ref() == id)
        .expect("advertised config option");
    let SessionConfigKind::Select(select) = &option.kind else {
        panic!("expected select config option")
    };
    let SessionConfigSelectOptions::Ungrouped(values) = &select.options else {
        panic!("expected ungrouped config choices")
    };
    values
        .iter()
        .map(|option| option.value.0.to_string())
        .collect()
}

async fn assert_config_rejected(
    connection: &agent_client_protocol::ConnectionTo<Agent>,
    session_id: &agent_client_protocol::schema::v1::SessionId,
    config_id: &str,
    value: &str,
    expected: &str,
) {
    let error = connection
        .send_request(SetSessionConfigOptionRequest::new(
            session_id.clone(),
            config_id.to_owned(),
            agent_client_protocol::schema::v1::SessionConfigValueId::new(value.to_owned()),
        ))
        .block_task()
        .await
        .expect_err("invalid session config value must be rejected");
    let detail = error
        .data
        .as_ref()
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    assert!(
        detail.contains(expected),
        "unexpected config error: {detail}"
    );
}

async fn send_text_prompt(
    connection: &agent_client_protocol::ConnectionTo<Agent>,
    session_id: &agent_client_protocol::schema::v1::SessionId,
    prompt: &str,
) -> Result<(), agent_client_protocol::Error> {
    let response = connection
        .send_request(PromptRequest::new(
            session_id.clone(),
            vec![ContentBlock::Text(TextContent::new(prompt))],
        ))
        .block_task()
        .await?;
    assert_eq!(response.stop_reason, StopReason::EndTurn);
    Ok(())
}

fn prompt_with_turn_id(
    session_id: &agent_client_protocol::schema::v1::SessionId,
    prompt: &str,
    turn_id: &str,
) -> PromptRequest {
    let mut meta = serde_json::Map::new();
    meta.insert(
        "polkagent.turnId".to_owned(),
        serde_json::Value::String(turn_id.to_owned()),
    );
    PromptRequest::new(
        session_id.clone(),
        vec![ContentBlock::Text(TextContent::new(prompt))],
    )
    .meta(meta)
}

fn durable_prompt_identity(response: &PromptResponse) -> DurablePromptIdentity {
    let value = response
        .meta
        .as_ref()
        .and_then(|meta| meta.get("polkagent"))
        .expect("Polkagent durable prompt metadata");
    DurablePromptIdentity {
        conversation_id: value["conversationId"]
            .as_str()
            .expect("conversationId metadata")
            .to_owned(),
        turn_id: value["turnId"]
            .as_str()
            .expect("turnId metadata")
            .to_owned(),
        run_ids: value["runIds"]
            .as_array()
            .expect("runIds metadata")
            .iter()
            .map(|run_id| run_id.as_str().expect("run ID string").to_owned())
            .collect(),
        checkpoint: value["checkpoint"].as_u64().expect("checkpoint metadata"),
    }
}

fn assert_runs_used_agent(
    db_path: &std::path::Path,
    agent_id: &str,
    expected_total: i64,
    expected_matching: i64,
) {
    let connection = rusqlite::Connection::open(db_path).expect("open durable ACP database");
    let total: i64 = connection
        .query_row("SELECT COUNT(*) FROM runs", [], |row| row.get(0))
        .expect("count ACP runs");
    let matching: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM runs WHERE agent_id = ?1",
            [agent_id],
            |row| row.get(0),
        )
        .expect("count configured-agent runs");
    assert_eq!((total, matching), (expected_total, expected_matching));
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

async fn delayed_success_provider(text: &str) -> DelayedSuccessProvider {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind delayed success provider");
    let address = listener
        .local_addr()
        .expect("delayed success provider address");
    let text = text.to_owned();
    let (model_tx, model_rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept provider request");
        let mut reader = BufReader::new(stream);
        let request = read_http_json_request(&mut reader).await;
        let model = request
            .get("model")
            .and_then(serde_json::Value::as_str)
            .expect("provider request model")
            .to_owned();
        model_tx.send(model.clone()).expect("record provider model");
        tokio::time::sleep(Duration::from_millis(150)).await;
        let body = serde_json::json!({
            "id": "chatcmpl-acp-progressive",
            "object": "chat.completion",
            "model": model,
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": text},
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 7, "completion_tokens": 3, "total_tokens": 10}
        })
        .to_string();
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        reader
            .get_mut()
            .write_all(response.as_bytes())
            .await
            .expect("write delayed provider response");
        reader
            .get_mut()
            .shutdown()
            .await
            .expect("close delayed provider response");
    });
    DelayedSuccessProvider {
        base_url: format!("http://{address}/v1"),
        model: model_rx,
        task,
    }
}

async fn recording_provider(request_count: usize) -> RecordingProvider {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind recording provider");
    let address = listener.local_addr().expect("recording provider address");
    let (requests_tx, requests_rx) = tokio::sync::mpsc::channel(request_count);
    let task = tokio::spawn(async move {
        for index in 0..request_count {
            let (stream, _) = listener.accept().await.expect("accept provider request");
            let mut reader = BufReader::new(stream);
            let request = read_http_json_request(&mut reader).await;
            let model = request
                .get("model")
                .and_then(serde_json::Value::as_str)
                .expect("provider request model")
                .to_owned();
            requests_tx.send(request).await.expect("record request");
            let body = serde_json::json!({
                "id": format!("chatcmpl-acp-{index}"),
                "object": "chat.completion",
                "model": model,
                "choices": [{
                    "index": 0,
                    "message": {"role": "assistant", "content": "configured response"},
                    "finish_reason": "stop"
                }],
                "usage": {"prompt_tokens": 2, "completion_tokens": 2, "total_tokens": 4}
            })
            .to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            reader
                .get_mut()
                .write_all(response.as_bytes())
                .await
                .expect("write provider response");
            reader
                .get_mut()
                .shutdown()
                .await
                .expect("close provider response");
        }
    });
    RecordingProvider {
        base_url: format!("http://{address}/v1"),
        requests: requests_rx,
        task,
    }
}

async fn read_http_json_request(
    reader: &mut BufReader<tokio::net::TcpStream>,
) -> serde_json::Value {
    let mut content_length = None;
    loop {
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .await
            .expect("read provider request header");
        assert!(!line.is_empty(), "provider request closed before headers");
        if line == "\r\n" {
            break;
        }
        let lowercase = line.to_ascii_lowercase();
        if let Some(value) = lowercase.strip_prefix("content-length:").map(str::trim) {
            content_length = Some(value.parse::<usize>().expect("content length"));
        }
    }
    let mut body = vec![0; content_length.expect("provider request content length")];
    reader
        .read_exact(&mut body)
        .await
        .expect("read provider request body");
    serde_json::from_slice(&body).expect("parse provider request JSON")
}

fn assert_request_model(requests: [&serde_json::Value; 2], prompt: &str, expected_model: &str) {
    let request = requests
        .into_iter()
        .find(|request| request.to_string().contains(prompt))
        .unwrap_or_else(|| panic!("no provider request contained prompt {prompt:?}"));
    assert_eq!(
        request.get("model").and_then(serde_json::Value::as_str),
        Some(expected_model)
    );
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
