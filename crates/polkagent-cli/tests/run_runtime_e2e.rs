//! Process-level coverage for the one-shot shared-runtime path.

// End-to-end assertions unwrap controlled filesystem, database, and process fixtures.
#![allow(clippy::expect_used)]

use std::process::Command;

use polkagent_store_sqlite::{migrations, SqlitePool, SqliteRunStore};

#[test]
fn one_shot_run_executes_through_runtime_and_persists_completion() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let database_path = temp.path().join("runtime-e2e.db");
    let config_path = temp.path().join("polkagent.toml");
    std::fs::write(
        &config_path,
        format!(
            "[database.sqlite]\npath = {:?}\n",
            database_path.to_string_lossy()
        ),
    )
    .expect("write config");

    let pool = SqlitePool::open(&database_path).expect("open database");
    migrations::migrate(&pool.writer()).expect("migrate database");
    let store = SqliteRunStore::new(pool.clone());
    let timestamp = "2026-01-01T00:00:00Z";
    let spec = serde_json::json!({
        "name": "runtime-e2e-agent",
        "description": null,
        "model": "fake/default-model",
        "tools": [],
        "autonomy_level": "supervised",
        "created_at": timestamp,
        "updated_at": timestamp,
    });
    let agent = store
        .create_agent("runtime-e2e-agent", None, &spec.to_string())
        .expect("create agent");

    let mut command = Command::new(env!("CARGO_BIN_EXE_polkagent"));
    command
        .args([
            "--config",
            config_path.to_str().expect("UTF-8 config path"),
            "--log-file",
            "-",
            "run",
            "--agent-id",
            &agent.id,
            "--prompt",
            "verify the shared runtime",
            "--no-harness",
            "--json",
            "--timeout",
            "10",
        ])
        .env_remove("POLKAGENT_CONFIG")
        .env_remove("POLKAGENT_DATABASE_SQLITE_PATH")
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

    let output = command.output().expect("run polkagent binary");
    assert!(
        output.status.success(),
        "run failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("UTF-8 stdout");
    assert!(stdout.contains("\"state\": \"running\""));
    assert!(stdout.contains("\"ok\": true"));

    let completed: i64 = pool
        .writer()
        .query_row(
            "SELECT COUNT(*) FROM runs WHERE agent_id = ?1 AND state = 'completed'",
            [&agent.id],
            |row| row.get(0),
        )
        .expect("query completed runs");
    assert_eq!(completed, 1);
}
