//! Process-level coverage for durable, headless terminal chat.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use polkagent_store_sqlite::{migrations, SqlitePool, SqliteRunStore};

fn seed_agent(database_path: &Path, name: &str, model: &str) {
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
        .expect("create active chat agent");
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
    for command in ["/help", "/status", "/cancel", "/new", "/resume"] {
        assert!(stdout.contains(command), "missing {command}: {stdout}");
    }
    for command in ["  /agent ", "  /model", "  /approve", "  /runs"] {
        assert!(!stdout.contains(command), "advertised {command}: {stdout}");
    }
}
