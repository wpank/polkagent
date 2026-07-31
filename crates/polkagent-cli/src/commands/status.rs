//! `polkagent status` — system-wide overview.
//!
//! Reports agent count, active runs, pending-effect queue depth, and memory
//! usage.  All counts are read directly from SQLite so the daemon does not
//! need to be running.

use anyhow::Result;

use polkagent_store_sqlite::SqlitePool;

use crate::cli::StatusCmd;

/// Execute the `status` subcommand.
pub fn run(cmd: &StatusCmd, pool: &SqlitePool) -> Result<()> {
    let reader = pool
        .reader()
        .map_err(|e| anyhow::anyhow!("opening database reader: {e}"))?;

    // Agent counts.
    let total_agents: i64 = reader
        .query_row("SELECT count(*) FROM agents", [], |r| r.get(0))
        .unwrap_or(0);
    let active_agents: i64 = reader
        .query_row(
            "SELECT count(*) FROM agents WHERE state = 'active'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let paused_agents: i64 = reader
        .query_row(
            "SELECT count(*) FROM agents WHERE state = 'paused'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);

    // Run counts.
    let active_runs: i64 = reader
        .query_row(
            "SELECT count(*) FROM runs WHERE state = 'running'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let pending_runs: i64 = reader
        .query_row(
            "SELECT count(*) FROM runs WHERE state = 'created'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);

    // Effect queue depth (unclaimed intents).
    let effect_queue_depth: i64 = reader
        .query_row(
            "SELECT count(*) FROM effect_intents WHERE claimed_by IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);

    // Memory stats (from the main DB; separate memory.db not counted here).
    let memory_entries: i64 = {
        // Try the in-process memory DB via env var.
        let home = std::env::var("HOME").unwrap_or_default();
        let default = format!("{home}/.local/share/polkagent/memory.db");
        let mem_path = std::env::var("POLKAGENT_MEMORY_DB_PATH").unwrap_or(default);

        rusqlite::Connection::open_with_flags(
            &mem_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .and_then(|conn| conn.query_row("SELECT count(*) FROM memories", [], |r| r.get(0)))
        .unwrap_or(0)
    };

    if cmd.json {
        let out = serde_json::json!({
            "agents": {
                "total":  total_agents,
                "active": active_agents,
                "paused": paused_agents,
            },
            "runs": {
                "active":  active_runs,
                "pending": pending_runs,
            },
            "effect_queue_depth": effect_queue_depth,
            "memory_entries": memory_entries,
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        println!("Polkagent Status");
        println!("{}", "-".repeat(40));
        println!("  Agents");
        println!("    Total:         {total_agents}");
        println!("    Active:        {active_agents}");
        println!("    Paused:        {paused_agents}");
        println!();
        println!("  Runs");
        println!("    Active:        {active_runs}");
        println!("    Pending:       {pending_runs}");
        println!();
        println!("  Effects");
        println!("    Queue depth:   {effect_queue_depth}");
        println!();
        println!("  Memory");
        println!("    Entries:       {memory_entries}");
    }

    Ok(())
}
