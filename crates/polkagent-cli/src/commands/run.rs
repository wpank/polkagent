//! `polkagent run` — submit a run and stream its output.
//!
//! For now this creates a run record in the database and prints its status.
//! Full streaming integration requires the polkagent-serve daemon.

use anyhow::Result;
use tracing::info;

use crate::cli::RunCmd;

/// Execute the `run` subcommand.
pub fn run(cmd: &RunCmd, db_path: &str) -> Result<()> {
    let conn = open_db(db_path)?;

    // Resolve agent by name or ID.
    let agent_row: Option<(String, String)> = conn
        .query_row(
            "SELECT id, name FROM agents WHERE (id = ?1 OR name = ?1) AND state NOT IN ('archived','deactivated') LIMIT 1",
            [&cmd.agent_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .ok();

    let (agent_id, agent_name) = match agent_row {
        Some(r) => r,
        None => anyhow::bail!("Agent not found or not active: {}", cmd.agent_id),
    };

    // Create a run record.
    use polkagent_core::ids::RunId;
    let run_id = RunId::new();
    let now = chrono::Utc::now().to_rfc3339();
    let params_json = serde_json::json!({ "prompt": cmd.prompt }).to_string();

    conn.execute(
        "INSERT INTO runs (id, agent_id, state, params_json, created_at, updated_at)
         VALUES (?1, ?2, 'created', ?3, ?4, ?4)",
        rusqlite::params![run_id.to_string(), agent_id, params_json, now],
    )?;

    if cmd.json {
        let out = serde_json::json!({
            "run_id":   run_id.to_string(),
            "agent_id": agent_id,
            "state":    "created",
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        println!("Run submitted:");
        println!("  Run ID: {run_id}");
        println!("  Agent:  {agent_name} ({agent_id})");
        println!("  State:  created");
        println!();
        println!("Note: A polkagent-serve daemon is required to execute the run.");
        println!("      Use `polkagent tui` to monitor run status.");
    }

    info!(run_id = %run_id, agent_id = %agent_id, "run created");
    Ok(())
}

fn open_db(path: &str) -> anyhow::Result<rusqlite::Connection> {
    let expanded = if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            format!("{home}/{rest}")
        } else {
            path.to_owned()
        }
    } else {
        path.to_owned()
    };
    rusqlite::Connection::open(&expanded)
        .map_err(|e| anyhow::anyhow!("opening database at {expanded}: {e}"))
}
