//! `polkagent agent` subcommand handlers.

use anyhow::Result;
use tracing::info;

use crate::cli::{AgentCmd, AgentCreateCmd, AgentDeleteCmd, AgentListCmd, AgentShowCmd};

/// Dispatch the agent subcommand.
pub fn run(cmd: &AgentCmd, db_path: &str) -> Result<()> {
    match cmd {
        AgentCmd::Create(c) => create(c, db_path),
        AgentCmd::List(c)   => list(c, db_path),
        AgentCmd::Show(c)   => show(c, db_path),
        AgentCmd::Delete(c) => delete(c, db_path),
    }
}

// ---------------------------------------------------------------------------
// create
// ---------------------------------------------------------------------------

fn create(cmd: &AgentCreateCmd, db_path: &str) -> Result<()> {
    use polkagent_core::ids::AgentId;
    use chrono::Utc;

    let id = AgentId::new();
    let now = Utc::now().to_rfc3339();

    // Build minimal spec JSON.
    let spec_json = serde_json::json!({
        "id":           id.to_string(),
        "name":         cmd.name,
        "description":  cmd.description,
        "model":        cmd.model,
        "tools":        [],
        "autonomy_level": "supervised",
        "created_at":   now,
        "updated_at":   now,
    });

    // Open database and insert the agent.
    let conn = open_db(db_path)?;
    conn.execute(
        "INSERT INTO agents (id, name, description, state, spec_json, created_at, updated_at)
         VALUES (?1, ?2, ?3, 'configured', ?4, ?5, ?5)",
        rusqlite::params![
            id.to_string(),
            cmd.name,
            cmd.description,
            spec_json.to_string(),
            now,
        ],
    )?;

    if cmd.json {
        let out = serde_json::json!({
            "id":    id.to_string(),
            "name":  cmd.name,
            "model": cmd.model,
            "state": "configured",
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        println!("Created agent:");
        println!("  ID:    {id}");
        println!("  Name:  {}", cmd.name);
        println!("  Model: {}", cmd.model);
        println!("  State: configured");
    }

    info!(agent_id = %id, name = %cmd.name, "agent created");
    Ok(())
}

// ---------------------------------------------------------------------------
// list
// ---------------------------------------------------------------------------

fn list(cmd: &AgentListCmd, db_path: &str) -> Result<()> {
    let conn = open_db(db_path)?;

    let where_clause = match cmd.filter.as_str() {
        "active"      => "AND state = 'active'",
        "idle"        => "AND state IN ('configured','created')",
        "error"       => "AND state NOT IN ('active','configured','created','archived')",
        _             => "",
    };

    let sql = format!(
        "SELECT id, name, state, updated_at FROM agents WHERE state != 'archived' {where_clause} ORDER BY name"
    );

    let mut stmt = conn.prepare(&sql)?;
    let rows: Vec<(String, String, String, String)> = stmt
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?
        .filter_map(|r| r.ok())
        .collect();

    if cmd.json {
        let agents: Vec<serde_json::Value> = rows
            .iter()
            .map(|(id, name, state, updated)| {
                serde_json::json!({
                    "id": id, "name": name, "state": state, "updated_at": updated
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&agents)?);
        return Ok(());
    }

    if rows.is_empty() {
        println!("No agents found.");
        return Ok(());
    }

    println!("{:<36}  {:<24}  {:<14}  Updated", "ID", "Name", "State");
    println!("{}", "-".repeat(90));
    for (id, name, state, updated) in &rows {
        let glyph = match state.as_str() {
            "active"     => "◉",
            "configured" => "○",
            "paused"     => "⏸",
            _            => "■",
        };
        println!("{id}  {glyph} {name:<22}  {state:<14}  {updated}");
    }
    println!();
    println!("{} agent(s)", rows.len());

    Ok(())
}

// ---------------------------------------------------------------------------
// show
// ---------------------------------------------------------------------------

fn show(cmd: &AgentShowCmd, db_path: &str) -> Result<()> {
    let conn = open_db(db_path)?;

    let row: Option<(String, String, String, String, String)> = conn
        .query_row(
            "SELECT id, name, state, spec_json, updated_at FROM agents
             WHERE id = ?1 OR name = ?1 LIMIT 1",
            [&cmd.agent],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
        )
        .ok();

    let (id, name, state, spec_json, updated) = match row {
        Some(r) => r,
        None => anyhow::bail!("Agent not found: {}", cmd.agent),
    };

    if cmd.json {
        let spec: serde_json::Value = serde_json::from_str(&spec_json)
            .unwrap_or(serde_json::Value::Null);
        let out = serde_json::json!({
            "id": id, "name": name, "state": state,
            "updated_at": updated, "spec": spec,
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }

    println!("Agent: {name}");
    println!("  ID:       {id}");
    println!("  State:    {state}");
    println!("  Updated:  {updated}");

    if cmd.full {
        let spec: serde_json::Value = serde_json::from_str(&spec_json)
            .unwrap_or(serde_json::Value::Null);
        println!("  Spec:");
        println!("{}", serde_json::to_string_pretty(&spec)?);
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// delete
// ---------------------------------------------------------------------------

fn delete(cmd: &AgentDeleteCmd, db_path: &str) -> Result<()> {
    if !cmd.yes {
        use std::io::{Write, self};
        print!("Archive agent '{}'? This cannot be undone. [y/N] ", cmd.agent);
        io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            println!("Aborted.");
            return Ok(());
        }
    }

    let conn = open_db(db_path)?;
    let now = chrono::Utc::now().to_rfc3339();
    let affected = conn.execute(
        "UPDATE agents SET state = 'archived', updated_at = ?1
         WHERE (id = ?2 OR name = ?2) AND state != 'archived'",
        rusqlite::params![now, cmd.agent],
    )?;

    if affected == 0 {
        anyhow::bail!("Agent not found or already archived: {}", cmd.agent);
    }

    println!("Agent '{}' archived.", cmd.agent);
    info!(agent = %cmd.agent, "agent archived");
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn open_db(path: &str) -> anyhow::Result<rusqlite::Connection> {
    let expanded = expand_tilde(path);
    rusqlite::Connection::open(&expanded)
        .with_context_msg(&format!("opening database at {expanded}"))
}

fn expand_tilde(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return format!("{home}/{rest}");
        }
    }
    path.to_owned()
}

trait WithContextMsg<T> {
    fn with_context_msg(self, msg: &str) -> anyhow::Result<T>;
}

impl<T, E: std::error::Error + Send + Sync + 'static> WithContextMsg<T> for Result<T, E> {
    fn with_context_msg(self, msg: &str) -> anyhow::Result<T> {
        self.map_err(|e| anyhow::anyhow!("{}: {}", msg, e))
    }
}
