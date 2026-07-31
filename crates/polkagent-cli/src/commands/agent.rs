//! `polkagent agent` subcommand handlers.

use anyhow::Result;
use tracing::info;

use polkagent_store_sqlite::{SqlitePool, SqliteRunStore};

use crate::cli::{
    AgentCmd, AgentCreateCmd, AgentDeleteCmd, AgentListCmd, AgentPauseCmd, AgentResumeCmd,
    AgentShowCmd, AgentStartCmd, AgentStopCmd,
};

/// Dispatch the agent subcommand.
pub fn run(cmd: &AgentCmd, pool: &SqlitePool) -> Result<()> {
    let store = SqliteRunStore::new(pool.clone());
    match cmd {
        AgentCmd::Create(c) => create(c, &store),
        AgentCmd::List(c)   => list(c, &store),
        AgentCmd::Show(c)   => show(c, &store),
        AgentCmd::Delete(c) => delete(c, &store),
        AgentCmd::Start(c)  => start(c, &store),
        AgentCmd::Stop(c)   => stop(c, &store),
        AgentCmd::Pause(c)  => pause(c, &store),
        AgentCmd::Resume(c) => resume(c, &store),
    }
}

// ---------------------------------------------------------------------------
// create
// ---------------------------------------------------------------------------

fn create(cmd: &AgentCreateCmd, store: &SqliteRunStore) -> Result<()> {
    use chrono::Utc;

    let now = Utc::now().to_rfc3339();

    // Build minimal spec JSON.
    let spec_json = serde_json::json!({
        "name":         cmd.name,
        "description":  cmd.description,
        "model":        cmd.model,
        "tools":        [],
        "autonomy_level": "supervised",
        "created_at":   now,
        "updated_at":   now,
    });

    let agent = store.create_agent(
        &cmd.name,
        cmd.description.as_deref(),
        &spec_json.to_string(),
    )?;

    if cmd.json {
        let out = serde_json::json!({
            "id":    agent.id,
            "name":  agent.name,
            "model": cmd.model,
            "state": agent.state,
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        println!("Created agent:");
        println!("  ID:    {}", agent.id);
        println!("  Name:  {}", agent.name);
        println!("  Model: {}", cmd.model);
        println!("  State: {}", agent.state);
    }

    info!(agent_id = %agent.id, name = %cmd.name, "agent created");
    Ok(())
}

// ---------------------------------------------------------------------------
// list
// ---------------------------------------------------------------------------

fn list(cmd: &AgentListCmd, store: &SqliteRunStore) -> Result<()> {
    // Map CLI filter strings to the optional state_filter argument.
    let state_filter = match cmd.filter.as_str() {
        "active" => Some("active"),
        "idle"   => Some("configured"),
        "error"  => None, // handled via post-filter below
        _        => None, // "all"
    };

    let rows = store.list_agents(state_filter, false)?;

    // For the "error" filter, post-filter to states that are not normal.
    let rows: Vec<_> = if cmd.filter == "error" {
        rows.into_iter()
            .filter(|r| !matches!(r.state.as_str(), "active" | "configured" | "created" | "archived"))
            .collect()
    } else {
        rows
    };

    if cmd.json {
        let agents: Vec<serde_json::Value> = rows
            .iter()
            .map(|r| {
                serde_json::json!({
                    "id": r.id, "name": r.name, "state": r.state, "updated_at": r.updated_at
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
    for r in &rows {
        let glyph = match r.state.as_str() {
            "active"     => "◉",
            "configured" => "○",
            "paused"     => "⏸",
            _            => "■",
        };
        println!("{}  {} {:<22}  {:<14}  {}", r.id, glyph, r.name, r.state, r.updated_at);
    }
    println!();
    println!("{} agent(s)", rows.len());

    Ok(())
}

// ---------------------------------------------------------------------------
// show
// ---------------------------------------------------------------------------

fn show(cmd: &AgentShowCmd, store: &SqliteRunStore) -> Result<()> {
    let agent = store
        .get_agent_by_name_or_id(&cmd.agent)
        .map_err(|e| anyhow::anyhow!("Agent not found: {} ({})", cmd.agent, e))?;

    if cmd.json {
        let spec: serde_json::Value = serde_json::from_str(&agent.spec_json)
            .unwrap_or(serde_json::Value::Null);
        let out = serde_json::json!({
            "id": agent.id, "name": agent.name, "state": agent.state,
            "updated_at": agent.updated_at, "spec": spec,
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }

    println!("Agent: {}", agent.name);
    println!("  ID:       {}", agent.id);
    println!("  State:    {}", agent.state);
    println!("  Updated:  {}", agent.updated_at);

    if cmd.full {
        let spec: serde_json::Value = serde_json::from_str(&agent.spec_json)
            .unwrap_or(serde_json::Value::Null);
        println!("  Spec:");
        println!("{}", serde_json::to_string_pretty(&spec)?);
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// delete
// ---------------------------------------------------------------------------

fn delete(cmd: &AgentDeleteCmd, store: &SqliteRunStore) -> Result<()> {
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

    // Resolve the agent first so we get a clear error if it doesn't exist.
    let agent = store
        .get_agent_by_name_or_id(&cmd.agent)
        .map_err(|_| anyhow::anyhow!("Agent not found or already archived: {}", cmd.agent))?;

    if agent.state == "archived" {
        anyhow::bail!("Agent not found or already archived: {}", cmd.agent);
    }

    store
        .update_agent_state(&agent.id, "archived")
        .map_err(|e| anyhow::anyhow!("archiving agent: {e}"))?;

    println!("Agent '{}' archived.", cmd.agent);
    info!(agent = %cmd.agent, "agent archived");
    Ok(())
}

// ---------------------------------------------------------------------------
// start
// ---------------------------------------------------------------------------

fn start(cmd: &AgentStartCmd, store: &SqliteRunStore) -> Result<()> {
    let agent = store
        .get_agent_by_name_or_id(&cmd.agent)
        .map_err(|_| anyhow::anyhow!("Agent not found: {}", cmd.agent))?;

    if agent.state == "archived" {
        anyhow::bail!("Cannot start archived agent: {}", cmd.agent);
    }

    if agent.state == "active" {
        println!("Agent '{}' is already active.", cmd.agent);
        return Ok(());
    }

    store
        .update_agent_state(&agent.id, "active")
        .map_err(|e| anyhow::anyhow!("starting agent: {e}"))?;

    println!("Agent '{}' started.", cmd.agent);
    println!("  ID:    {}", agent.id);
    println!("  State: active");
    info!(agent_id = %agent.id, "agent started");
    Ok(())
}

// ---------------------------------------------------------------------------
// stop
// ---------------------------------------------------------------------------

fn stop(cmd: &AgentStopCmd, store: &SqliteRunStore) -> Result<()> {
    let agent = store
        .get_agent_by_name_or_id(&cmd.agent)
        .map_err(|_| anyhow::anyhow!("Agent not found: {}", cmd.agent))?;

    if agent.state == "archived" {
        anyhow::bail!("Cannot stop archived agent: {}", cmd.agent);
    }

    if agent.state == "configured" {
        println!("Agent '{}' is already stopped.", cmd.agent);
        return Ok(());
    }

    store
        .update_agent_state(&agent.id, "configured")
        .map_err(|e| anyhow::anyhow!("stopping agent: {e}"))?;

    println!("Agent '{}' stopped.", cmd.agent);
    println!("  ID:    {}", agent.id);
    println!("  State: configured");
    info!(agent_id = %agent.id, "agent stopped");
    Ok(())
}

// ---------------------------------------------------------------------------
// pause
// ---------------------------------------------------------------------------

fn pause(cmd: &AgentPauseCmd, store: &SqliteRunStore) -> Result<()> {
    let agent = store
        .get_agent_by_name_or_id(&cmd.agent)
        .map_err(|_| anyhow::anyhow!("Agent not found: {}", cmd.agent))?;

    if agent.state == "archived" {
        anyhow::bail!("Cannot pause archived agent: {}", cmd.agent);
    }

    if agent.state == "paused" {
        println!("Agent '{}' is already paused.", cmd.agent);
        return Ok(());
    }

    store
        .update_agent_state(&agent.id, "paused")
        .map_err(|e| anyhow::anyhow!("pausing agent: {e}"))?;

    println!("Agent '{}' paused.", cmd.agent);
    println!("  ID:    {}", agent.id);
    println!("  State: paused");
    info!(agent_id = %agent.id, "agent paused");
    Ok(())
}

// ---------------------------------------------------------------------------
// resume
// ---------------------------------------------------------------------------

fn resume(cmd: &AgentResumeCmd, store: &SqliteRunStore) -> Result<()> {
    let agent = store
        .get_agent_by_name_or_id(&cmd.agent)
        .map_err(|_| anyhow::anyhow!("Agent not found: {}", cmd.agent))?;

    if agent.state == "archived" {
        anyhow::bail!("Cannot resume archived agent: {}", cmd.agent);
    }

    if agent.state != "paused" {
        println!(
            "Agent '{}' is not paused (current state: {}). Use `agent start` instead.",
            cmd.agent, agent.state
        );
        return Ok(());
    }

    store
        .update_agent_state(&agent.id, "active")
        .map_err(|e| anyhow::anyhow!("resuming agent: {e}"))?;

    println!("Agent '{}' resumed.", cmd.agent);
    println!("  ID:    {}", agent.id);
    println!("  State: active");
    info!(agent_id = %agent.id, "agent resumed");
    Ok(())
}
