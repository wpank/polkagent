//! Process-level coverage for durable, headless terminal chat.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use polkagent_store_sqlite::{migrations, SqlitePool, SqliteRunStore};

fn seed_agent(database_path: &Path, name: &str, model: &str) {
    let _ = seed_agent_id(database_path, name, model);
}

fn seed_agent_id(database_path: &Path, name: &str, model: &str) -> String {
    let pool = SqlitePool::open(database_path).expect("open chat database");
    migrations::migrate(&pool.writer()).expect("migrate chat database");
    let store = SqliteRunStore::new(pool);
    let timestamp = "2026-01-01T00:00:00Z";
    let spec = serde_json::json!({
        "name": name,
        "description": null,
        "model": model,
        "tools": [],
        "autonomy_level": "supervised",
        "created_at": timestamp,
        "updated_at": timestamp,
    });
    store
        .create_agent(name, None, &spec.to_string())
        .expect("create active chat agent")
        .id
}

fn chat_command(database_path: &Path, log_path: &Path, config_path: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_polkagent"));
    command
        .args([
            "--config",
            config_path.to_string_lossy().as_ref(),
            "--log-file",
            log_path.to_string_lossy().as_ref(),
        ])
        .env("NO_COLOR", "1")
        .env("POLKAGENT_DATABASE_SQLITE_PATH", database_path)
        .env_remove("POLKAGENT_CONFIG")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .env_remove("GEMINI_API_KEY")
        .env_remove("OPENROUTER_API_KEY")
        .env_remove("PERPLEXITY_API_KEY")
        .env_remove("CEREBRAS_API_KEY")
        .env_remove("OLLAMA_URL")
        .env_remove("OLLAMA_MODEL")
        .env_remove("POLKAGENT_CHAIN_RPC_URL")
        .env_remove("POLKAGENT_RPC_URL");
    command
}

fn run_with_stdin(mut command: Command, input: &str) -> Output {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn terminal chat");
    child
        .stdin
        .take()
        .expect("chat stdin")
        .write_all(input.as_bytes())
        .expect("write chat input");
    child.wait_with_output().expect("wait for terminal chat")
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "chat failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn session_id(stderr: &str) -> &str {
    stderr
        .lines()
        .find_map(|line| line.trim().strip_prefix("chat session: "))
        .expect("chat session ID on stderr")
}

#[test]
fn non_tty_chat_keeps_stdout_clean_and_resumes_durable_transcript_after_restart() {
    let temp = tempfile::tempdir().expect("chat tempdir");
    let database_path = temp.path().join("chat.db");
    let log_path = temp.path().join("chat.jsonl");
    let config_path = temp.path().join("polkagent.toml");
    std::fs::write(&config_path, "").expect("write isolated config");
    seed_agent(&database_path, "chat-fixture", "fake/test");

    let mut first = chat_command(&database_path, &log_path, &config_path);
    first.args(["chat", "--agent", "chat-fixture"]);
    let first = run_with_stdin(first, "first line\nsecond line\n");
    assert_success(&first);
    assert_eq!(
        String::from_utf8_lossy(&first.stdout),
        "I am a fake assistant\n"
    );
    let first_stderr = String::from_utf8(first.stderr).expect("UTF-8 chat stderr");
    assert!(!first_stderr.contains("I am a fake assistant"));
    assert!(first_stderr.contains("turn started:"));
    assert!(first_stderr.contains("[usage]"));
    let conversation_id = session_id(&first_stderr).to_owned();

    // Low-level conversation rows not correlated by an interaction turn must
    // never bleed into the surface-neutral transcript projection.
    {
        let connection = rusqlite::Connection::open(&database_path)
            .expect("open chat DB for unlinked message fixture");
        connection
            .execute(
                "INSERT INTO conversation_messages
                     (id, conversation_id, role, content_json, created_at)
                 VALUES (?1, ?2, 'user', ?3, '2026-01-01T00:00:01Z')",
                rusqlite::params![
                    uuid::Uuid::now_v7().to_string(),
                    &conversation_id,
                    serde_json::json!({
                        "type": "text",
                        "text": "unlinked message must not render",
                    })
                    .to_string(),
                ],
            )
            .expect("insert unlinked conversation message");
        connection
            .execute(
                "UPDATE conversations SET message_count = message_count + 1 WHERE id = ?1",
                [&conversation_id],
            )
            .expect("update unlinked message count");
    }

    let mut resumed = chat_command(&database_path, &log_path, &config_path);
    resumed.args([
        "chat",
        "--agent",
        "chat-fixture",
        "--resume",
        &conversation_id,
    ]);
    let resumed = run_with_stdin(resumed, "");
    assert_success(&resumed);
    let transcript = String::from_utf8(resumed.stdout).expect("UTF-8 resumed transcript");
    assert_eq!(
        transcript,
        "user> first line\nsecond line\nassistant> I am a fake assistant\n"
    );
    let resumed_stderr = String::from_utf8(resumed.stderr).expect("UTF-8 resume stderr");
    assert_eq!(session_id(&resumed_stderr), conversation_id);

    let connection = rusqlite::Connection::open(&database_path).expect("open durable chat DB");
    let interaction_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM interaction_sessions", [], |row| {
            row.get(0)
        })
        .expect("count durable interactions");
    let terminal_turns: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM interaction_turns WHERE state = 'completed' AND completed_at IS NOT NULL",
            [],
            |row| row.get(0),
        )
        .expect("count completed durable turns");
    assert_eq!((interaction_count, terminal_turns), (1, 1));
}

#[test]
fn non_tty_configuration_command_fails_without_polluting_stdout() {
    let temp = tempfile::tempdir().expect("chat tempdir");
    let database_path = temp.path().join("chat.db");
    let log_path = temp.path().join("chat.jsonl");
    let config_path = temp.path().join("polkagent.toml");
    std::fs::write(&config_path, "").expect("write isolated config");
    seed_agent(&database_path, "chat-fixture", "fake/test");

    let mut command = chat_command(&database_path, &log_path, &config_path);
    command.args(["chat", "--agent", "chat-fixture"]);
    let output = run_with_stdin(command, "/provider openai\n");
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains(
        "terminal chat does not support provider selection; configure the runtime and restart"
    ));
}

#[cfg(unix)]
#[tokio::test]
async fn ctrl_c_cancels_only_the_active_non_tty_turn_and_persists_it() {
    use std::time::Duration;

    use nix::sys::signal::{kill, Signal};
    use nix::unistd::Pid;
    use tokio::io::{AsyncBufReadExt as _, BufReader};

    let temp = tempfile::tempdir().expect("chat tempdir");
    let database_path = temp.path().join("chat.db");
    let log_path = temp.path().join("chat.jsonl");
    let config_path = temp.path().join("polkagent.toml");
    seed_agent(&database_path, "cancel-fixture", "fixture/model");

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind delayed chat provider");
    let address = listener.local_addr().expect("delayed provider address");
    std::fs::write(
        &config_path,
        format!(
            "[[providers]]\nid = \"cancel-provider\"\nprovider_type = \"local\"\nbase_url = \"http://{address}/v1\"\napi_key_env = \"POLKAGENT_CHAT_FIXTURE_KEY\"\ndefault_model = \"fixture-model\"\n"
        ),
    )
    .expect("write delayed provider config");
    let (request_tx, request_rx) = tokio::sync::oneshot::channel();
    let provider = tokio::spawn(async move {
        let (stream, _) = listener
            .accept()
            .await
            .expect("accept chat provider request");
        let mut reader = BufReader::new(stream);
        let mut request_line = String::new();
        reader
            .read_line(&mut request_line)
            .await
            .expect("read chat provider request");
        assert!(request_line.starts_with("POST /v1/chat/completions "));
        request_tx.send(()).expect("signal active provider request");
        std::future::pending::<()>().await;
    });

    let mut command = chat_command(&database_path, &log_path, &config_path);
    command
        .args(["chat", "--agent", "cancel-fixture"])
        .env("POLKAGENT_CHAT_FIXTURE_KEY", "fixture-key")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().expect("spawn cancellable terminal chat");
    let pid = child.id();
    child
        .stdin
        .take()
        .expect("chat stdin")
        .write_all(b"wait for cancellation\n")
        .expect("write cancellable prompt");
    tokio::time::timeout(Duration::from_secs(10), request_rx)
        .await
        .expect("provider request did not become active")
        .expect("provider request signal dropped");
    let raw_pid = i32::try_from(pid).expect("child PID fits i32");
    kill(Pid::from_raw(raw_pid), Signal::SIGINT).expect("send Ctrl-C to terminal chat");
    let output = tokio::task::spawn_blocking(move || child.wait_with_output())
        .await
        .expect("join chat waiter")
        .expect("wait for cancelled terminal chat");
    provider.abort();

    assert_success(&output);
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("cancellation requested for turn"));
    assert!(stderr.contains("cancellation reason:") || stderr.contains("turn cancelled"));
    assert!(stderr.contains("chat session:"));

    let connection = rusqlite::Connection::open(&database_path).expect("open durable chat DB");
    let (state, completed_at): (String, Option<String>) = connection
        .query_row(
            "SELECT state, completed_at FROM interaction_turns ORDER BY ordinal DESC LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("load cancelled chat turn");
    assert_eq!(state, "cancelled");
    assert!(completed_at.is_some());
}

#[test]
fn chat_help_does_not_claim_unsupported_commands() {
    let temp = tempfile::tempdir().expect("chat tempdir");
    let database_path = temp.path().join("chat.db");
    let log_path = temp.path().join("chat.jsonl");
    let config_path = temp.path().join("polkagent.toml");
    std::fs::write(&config_path, "").expect("write isolated config");
    seed_agent(&database_path, "chat-fixture", "fake/test");

    let mut command = chat_command(&database_path, &log_path, &config_path);
    command.args(["chat", "--agent", "chat-fixture"]);
    let output = run_with_stdin(command, "/help\n");
    assert_success(&output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    for command in [
        "/help", "/status", "/agents", "/agent", "/cancel", "/new", "/resume", "/model",
    ] {
        assert!(stdout.contains(command), "missing {command}: {stdout}");
    }
    for command in ["  /approve", "  /runs"] {
        assert!(!stdout.contains(command), "advertised {command}: {stdout}");
    }
}

#[test]
fn agent_commands_list_persist_resume_and_drive_the_next_process_without_creating_work() {
    let temp = tempfile::tempdir().expect("chat tempdir");
    let database_path = temp.path().join("chat.db");
    let log_path = temp.path().join("chat.jsonl");
    let config_path = temp.path().join("polkagent.toml");
    std::fs::write(&config_path, "").expect("write isolated config");
    let first_id = seed_agent_id(&database_path, "first-agent", "fake/first");
    let second_id = seed_agent_id(&database_path, "second-agent", "fake/second");

    let mut list = chat_command(&database_path, &log_path, &config_path);
    list.args(["chat", "--agent", "first-agent"]);
    let list = run_with_stdin(list, "/agents\n");
    assert_success(&list);
    let list_stdout = String::from_utf8_lossy(&list.stdout);
    assert!(list_stdout.contains("* first-agent"), "{list_stdout}");
    assert!(list_stdout.contains("second-agent"), "{list_stdout}");
    assert!(list_stdout.contains("[active]"), "{list_stdout}");
    assert!(
        list_stdout.contains("readiness: not projected"),
        "{list_stdout}"
    );
    let conversation_id = session_id(&String::from_utf8_lossy(&list.stderr)).to_owned();

    let connection = rusqlite::Connection::open(&database_path).expect("open agent command DB");
    let specs_before: String = connection
        .query_row(
            "SELECT group_concat(spec_json, '\n') FROM (SELECT spec_json FROM agents ORDER BY id)",
            [],
            |row| row.get(0),
        )
        .expect("snapshot agent specs");

    let mut select = chat_command(&database_path, &log_path, &config_path);
    select.args([
        "chat",
        "--agent",
        "first-agent",
        "--resume",
        &conversation_id,
    ]);
    let select = run_with_stdin(select, "/agent second-agent\n");
    assert_success(&select);
    let select_stdout = String::from_utf8_lossy(&select.stdout);
    assert!(
        select_stdout.contains(&format!("target: second-agent ({second_id})")),
        "{select_stdout}"
    );

    let (config_json, turns, runs, events): (String, i64, i64, i64) = connection
        .query_row(
            "SELECT config_json,
                    (SELECT COUNT(*) FROM interaction_turns),
                    (SELECT COUNT(*) FROM runs),
                    (SELECT COUNT(*) FROM interaction_events)
             FROM interaction_sessions WHERE conversation_id = ?1",
            [&conversation_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .expect("load selected durable target and zero-work counts");
    let config: serde_json::Value = serde_json::from_str(&config_json).expect("decode config");
    assert_eq!(config["target"]["id"], second_id);
    assert_eq!((turns, runs, events), (0, 0, 0));
    let specs_after: String = connection
        .query_row(
            "SELECT group_concat(spec_json, '\n') FROM (SELECT spec_json FROM agents ORDER BY id)",
            [],
            |row| row.get(0),
        )
        .expect("reload agent specs");
    assert_eq!(
        specs_after, specs_before,
        "/agent must not mutate AgentSpec rows"
    );

    let mut status = chat_command(&database_path, &log_path, &config_path);
    status.args([
        "chat",
        "--agent",
        "first-agent",
        "--resume",
        &conversation_id,
    ]);
    let status = run_with_stdin(status, "/status\n");
    assert_success(&status);
    let status_stdout = String::from_utf8_lossy(&status.stdout);
    assert!(
        status_stdout.contains(&format!("target: second-agent ({second_id})")),
        "{status_stdout}"
    );
    let status_stderr = String::from_utf8_lossy(&status.stderr);
    assert!(
        status_stderr.contains(&format!("chat agent: second-agent ({second_id})")),
        "{status_stderr}"
    );

    let mut prompt = chat_command(&database_path, &log_path, &config_path);
    prompt.args([
        "chat",
        "--agent",
        "first-agent",
        "--resume",
        &conversation_id,
    ]);
    let prompt = run_with_stdin(prompt, "use the persisted target\n");
    assert_success(&prompt);
    let run_agent_id: String = connection
        .query_row(
            "SELECT agent_id FROM runs WHERE conversation_id = ?1",
            [&conversation_id],
            |row| row.get(0),
        )
        .expect("load run target after restart");
    assert_eq!(run_agent_id, second_id);
    assert_ne!(run_agent_id, first_id);
}

#[test]
fn agent_command_rejects_unknown_inactive_and_ambiguous_targets_without_work() {
    let temp = tempfile::tempdir().expect("chat tempdir");
    let database_path = temp.path().join("chat.db");
    let log_path = temp.path().join("chat.jsonl");
    let config_path = temp.path().join("polkagent.toml");
    std::fs::write(&config_path, "").expect("write isolated config");
    let first_id = seed_agent_id(&database_path, "first-agent", "fake/first");
    let inactive_id = seed_agent_id(&database_path, "inactive-agent", "fake/inactive");
    seed_agent(&database_path, &first_id, "fake/ambiguous");
    let pool = SqlitePool::open(&database_path).expect("open refusal database");
    SqliteRunStore::new(pool)
        .update_agent_state(&inactive_id, "paused")
        .expect("pause inactive fixture");

    for (selector, code) in [
        ("missing-agent", "not_found"),
        ("inactive-agent", "not_found"),
        (first_id.as_str(), "conflict"),
    ] {
        let mut command = chat_command(&database_path, &log_path, &config_path);
        command.args(["chat", "--agent", "first-agent"]);
        let output = run_with_stdin(command, &format!("/agent {selector}\n"));
        assert!(
            !output.status.success(),
            "{selector} unexpectedly succeeded"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains(code),
            "missing {code} for {selector}: {stderr}"
        );
    }

    let connection = rusqlite::Connection::open(&database_path).expect("open refusal DB");
    let counts: (i64, i64, i64) = connection
        .query_row(
            "SELECT (SELECT COUNT(*) FROM interaction_turns),
                    (SELECT COUNT(*) FROM runs),
                    (SELECT COUNT(*) FROM interaction_events)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("count refusal work rows");
    assert_eq!(counts, (0, 0, 0));
}

#[test]
fn model_command_is_conversation_scoped_persists_across_process_resume_and_creates_no_turn() {
    let temp = tempfile::tempdir().expect("chat tempdir");
    let database_path = temp.path().join("chat.db");
    let log_path = temp.path().join("chat.jsonl");
    let config_path = temp.path().join("polkagent.toml");
    std::fs::write(
        &config_path,
        r#"
[[providers]]
id = "fake"
provider_type = "fake"
api_key_env = "POLKAGENT_TEST_FAKE_KEY_UNSET"
default_model = "default-model"

[[providers]]
id = "other"
provider_type = "fake"
api_key_env = "POLKAGENT_TEST_OTHER_KEY_UNSET"
default_model = "other-default"

[[models]]
slug = "model-a"
provider = "fake"

[[models]]
slug = "other-model"
provider = "other"
"#,
    )
    .expect("write isolated model config");
    seed_agent(&database_path, "chat-fixture", "fake/default-model");

    let mut select = chat_command(&database_path, &log_path, &config_path);
    select.args(["chat", "--agent", "chat-fixture"]);
    let select = run_with_stdin(select, "/model model-a\n");
    assert_success(&select);
    let select_stdout = String::from_utf8_lossy(&select.stdout);
    assert!(
        select_stdout.contains("model: fake/model-a"),
        "{select_stdout}"
    );
    assert!(
        select_stdout.contains("updated for this durable conversation"),
        "{select_stdout}"
    );
    let select_stderr = String::from_utf8_lossy(&select.stderr);
    let conversation_id = session_id(&select_stderr).to_owned();

    let mut query = chat_command(&database_path, &log_path, &config_path);
    query.args([
        "chat",
        "--agent",
        "chat-fixture",
        "--resume",
        &conversation_id,
    ]);
    let query = run_with_stdin(query, "/model\n");
    assert_success(&query);
    assert!(
        String::from_utf8_lossy(&query.stdout).contains("model: fake/model-a"),
        "{}",
        String::from_utf8_lossy(&query.stdout)
    );

    for (model, code) in [
        ("missing-model", "invalid_request"),
        ("other-model", "unsupported"),
    ] {
        let mut refused = chat_command(&database_path, &log_path, &config_path);
        refused.args([
            "chat",
            "--agent",
            "chat-fixture",
            "--resume",
            &conversation_id,
        ]);
        let refused = run_with_stdin(refused, &format!("/model {model}\n"));
        assert!(!refused.status.success(), "{model} unexpectedly succeeded");
        let stderr = String::from_utf8_lossy(&refused.stderr);
        assert!(stderr.contains(code), "missing {code} refusal: {stderr}");
        assert!(stderr.contains(model), "missing refused model: {stderr}");
    }

    let connection = rusqlite::Connection::open(&database_path).expect("open durable chat DB");
    let (config_json, turn_count): (String, i64) = connection
        .query_row(
            "SELECT s.config_json,
                    (SELECT COUNT(*) FROM interaction_turns t
                     WHERE t.conversation_id = s.conversation_id)
             FROM interaction_sessions s WHERE s.conversation_id = ?1",
            [&conversation_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("load persisted interaction config");
    let config: serde_json::Value = serde_json::from_str(&config_json).expect("decode config");
    assert_eq!(config["model"], "fake/model-a");
    assert_eq!(
        turn_count, 0,
        "model commands must never become transcript turns"
    );

    let agent_spec_json: String = connection
        .query_row(
            "SELECT spec_json FROM agents WHERE name = 'chat-fixture'",
            [],
            |row| row.get(0),
        )
        .expect("load unchanged agent spec");
    let agent_spec: serde_json::Value =
        serde_json::from_str(&agent_spec_json).expect("decode agent spec");
    assert_eq!(agent_spec["model"], "fake/default-model");

    let mut prompt = chat_command(&database_path, &log_path, &config_path);
    prompt.args([
        "chat",
        "--agent",
        "chat-fixture",
        "--resume",
        &conversation_id,
    ]);
    let prompt = run_with_stdin(prompt, "use persisted model\n");
    assert_success(&prompt);
    let turn_config_json: String = connection
        .query_row(
            "SELECT config_json FROM interaction_turns WHERE conversation_id = ?1",
            [&conversation_id],
            |row| row.get(0),
        )
        .expect("load effective turn config");
    let turn_config: serde_json::Value =
        serde_json::from_str(&turn_config_json).expect("decode turn config");
    assert_eq!(turn_config["model"], "fake/model-a");
}
