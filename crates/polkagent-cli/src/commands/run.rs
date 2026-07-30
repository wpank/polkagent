//! `polkagent run` — submit a run and stream its output.
//!
//! For now this creates a run record in the database and prints its status.
//! Full streaming integration requires the polkagent-serve daemon.

use anyhow::Result;
use tracing::info;

use polkagent_store_sqlite::{SqlitePool, SqliteRunStore};

use crate::cli::RunCmd;

/// Execute the `run` subcommand.
pub fn run(cmd: &RunCmd, pool: &SqlitePool) -> Result<()> {
    let store = SqliteRunStore::new(pool.clone());

    // Resolve agent by name or ID (must be active/configured, not archived).
    let agent = store
        .get_agent_by_name_or_id(&cmd.agent_id)
        .map_err(|_| anyhow::anyhow!("Agent not found or not active: {}", cmd.agent_id))?;

    if matches!(agent.state.as_str(), "archived" | "deactivated") {
        anyhow::bail!("Agent not found or not active: {}", cmd.agent_id);
    }

    // Create a run record via the store.
    let params_json = serde_json::json!({ "prompt": cmd.prompt }).to_string();
    let run_row = store
        .create_run(&agent.id, None, &params_json)
        .map_err(|e| anyhow::anyhow!("creating run: {e}"))?;

    if cmd.json {
        let out = serde_json::json!({
            "run_id":   run_row.id,
            "agent_id": agent.id,
            "state":    run_row.state,
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        println!("Run submitted:");
        println!("  Run ID: {}", run_row.id);
        println!("  Agent:  {} ({})", agent.name, agent.id);
        println!("  State:  {}", run_row.state);
        println!();
        println!("Note: A polkagent-serve daemon is required to execute the run.");
        println!("      Use `polkagent tui` to monitor run status.");
    }

    info!(run_id = %run_row.id, agent_id = %agent.id, "run created");
    Ok(())
}
