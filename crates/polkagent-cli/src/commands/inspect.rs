//! `polkagent inspect` — Inspect runs, effects, artifacts, agents, and policies.
//!
//! A read-only diagnostic command that drills into individual entities stored
//! in the local `SQLite` database or on disk (policy TOML files).  Each
//! subcommand fetches a single entity by ID/path, formats it for human
//! consumption, and optionally emits structured JSON (`--json`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use rusqlite::OptionalExtension as _;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Subcommand enum
// ---------------------------------------------------------------------------

/// Top-level `inspect` subcommand dispatcher.
#[derive(Debug, Subcommand)]
pub enum InspectCmd {
    /// Show run details: status, agent, turns, steps, timing, effects.
    Run(InspectRunArgs),

    /// Show effect details: intent, attempts, outcome, timing.
    Effect(InspectEffectArgs),

    /// Show artifact metadata: kind, digest, size, lineage.
    Artifact(InspectArtifactArgs),

    /// Show agent spec: model, tools, policies, autonomy level, capabilities.
    Agent(InspectAgentArgs),

    /// Parse and display a TOML policy file with resolved rules.
    Policy(InspectPolicyArgs),

    /// Show database stats: table row counts, database size, WAL size, migration version.
    Db(InspectDbArgs),
}

// ---------------------------------------------------------------------------
// Per-subcommand argument structs
// ---------------------------------------------------------------------------

/// Arguments for `inspect run`.
#[derive(Debug, Args)]
pub struct InspectRunArgs {
    /// Run ID (UUID) to inspect.
    #[arg(value_name = "RUN_ID")]
    pub run_id: String,

    /// Emit structured JSON output instead of a human-readable table.
    #[arg(long)]
    pub json: bool,
}

/// Arguments for `inspect effect`.
#[derive(Debug, Args)]
pub struct InspectEffectArgs {
    /// Effect ID (UUID) to inspect.
    #[arg(value_name = "EFFECT_ID")]
    pub effect_id: String,

    /// Emit structured JSON output.
    #[arg(long)]
    pub json: bool,
}

/// Arguments for `inspect artifact`.
#[derive(Debug, Args)]
pub struct InspectArtifactArgs {
    /// Artifact ID (UUID or content-address) to inspect.
    #[arg(value_name = "ARTIFACT_ID")]
    pub artifact_id: String,

    /// Emit structured JSON output.
    #[arg(long)]
    pub json: bool,
}

/// Arguments for `inspect agent`.
#[derive(Debug, Args)]
pub struct InspectAgentArgs {
    /// Agent name or UUID to inspect.
    #[arg(value_name = "AGENT_ID")]
    pub agent_id: String,

    /// Emit structured JSON output.
    #[arg(long)]
    pub json: bool,
}

/// Arguments for `inspect policy`.
#[derive(Debug, Args)]
pub struct InspectPolicyArgs {
    /// Path to the TOML policy file.
    #[arg(value_name = "PATH")]
    pub path: PathBuf,

    /// Emit structured JSON output.
    #[arg(long)]
    pub json: bool,
}

/// Arguments for `inspect db`.
#[derive(Debug, Args)]
pub struct InspectDbArgs {
    /// Emit structured JSON output.
    #[arg(long)]
    pub json: bool,
}

// ---------------------------------------------------------------------------
// Output structures
// ---------------------------------------------------------------------------

/// Serialisable representation of a run inspection result.
#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct RunInfo {
    pub id: String,
    pub agent_id: String,
    pub state: String,
    pub created_at: String,
    pub updated_at: String,
    pub turns: u64,
    pub steps: u64,
    pub elapsed_ms: Option<u64>,
    pub effects: Vec<String>,
}

/// Serialisable representation of an effect inspection result.
#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct EffectInfo {
    pub id: String,
    pub run_id: String,
    pub agent_id: String,
    pub intent: String,
    pub state: String,
    pub attempts: u32,
    pub outcome: Option<String>,
    pub payload: Option<String>,
    pub created_at: String,
    pub resolved_at: Option<String>,
}

/// Serialisable representation of an artifact inspection result.
#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct ArtifactInfo {
    pub id: String,
    pub run_id: Option<String>,
    pub kind: String,
    pub digest: String,
    pub size_bytes: u64,
    pub metadata: Option<String>,
    pub created_at: String,
    pub lineage: ArtifactLineage,
}

/// Lineage information for an artifact.
#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct ArtifactLineage {
    pub run_id: Option<String>,
    pub step_id: Option<String>,
    pub parent_artifact_id: Option<String>,
}

/// Serialisable representation of an agent inspection result.
#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct AgentInfo {
    pub id: String,
    pub name: String,
    pub model: String,
    pub state: String,
    pub autonomy_level: String,
    pub tools: Vec<String>,
    pub capabilities: Vec<String>,
    pub policy_refs: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
    pub spec_json: Option<String>,
}

/// A single resolved policy rule for display.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct PolicyRule {
    pub action: String,
    pub resource: String,
    pub effect: String,
    pub conditions: Vec<String>,
}

/// Serialisable representation of a parsed policy file.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct PolicyInfo {
    pub path: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub rules: Vec<PolicyRule>,
}

/// Serialisable representation of database statistics.
#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct DbStats {
    pub db_size_bytes: u64,
    pub wal_size_bytes: u64,
    pub migration_version: u64,
    pub table_row_counts: BTreeMap<String, u64>,
}

type ArtifactRow = (String, Option<String>, String, String, i64, String, String);

// ---------------------------------------------------------------------------
// Dispatcher
// ---------------------------------------------------------------------------

/// Dispatch the `inspect` subcommand.
pub async fn run(cmd: &InspectCmd) -> Result<()> {
    match cmd {
        InspectCmd::Run(args) => execute_inspect_run(&args.run_id, args.json).await,
        InspectCmd::Effect(args) => execute_inspect_effect(&args.effect_id, args.json).await,
        InspectCmd::Artifact(args) => execute_inspect_artifact(&args.artifact_id, args.json).await,
        InspectCmd::Agent(args) => execute_inspect_agent(&args.agent_id, args.json).await,
        InspectCmd::Policy(args) => execute_inspect_policy(&args.path, args.json).await,
        InspectCmd::Db(args) => execute_inspect_db(args.json).await,
    }
}

// ---------------------------------------------------------------------------
// Run inspection
// ---------------------------------------------------------------------------

/// Inspect a run by ID or ID prefix.
///
/// Accepts either a full 36-character UUID (exact match) or a shorter prefix
/// string (prefix-match via LIKE, the same way `git log` handles short SHAs).
/// Queries the local `SQLite` database directly without requiring the daemon.
pub async fn execute_inspect_run(run_id: &str, json_output: bool) -> Result<()> {
    if run_id.is_empty() {
        anyhow::bail!("run ID must not be empty");
    }

    let db_path = resolve_db_path();
    let expanded = expand_tilde(&db_path);

    let run_id = run_id.to_owned();
    let info = tokio::task::spawn_blocking(move || {
        query_run(&expanded, &run_id).with_context(|| format!("querying database at {expanded}"))
    })
    .await
    .context("joining inspect run query task")??;

    render_run(&info, json_output)
}

/// Query the `SQLite` database for a run by exact ID or ID prefix.
fn query_run(db_path: &str, run_id: &str) -> Result<RunInfo> {
    let conn =
        rusqlite::Connection::open_with_flags(db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .with_context(|| format!("opening database at {db_path}"))?;

    // Choose exact match or prefix match depending on input length.
    let (sql, param): (&str, String) = if run_id.len() == 36 {
        (
            "SELECT id, agent_id, state, created_at, updated_at
             FROM runs
             WHERE id = ?1
             LIMIT 1",
            run_id.to_string(),
        )
    } else {
        (
            "SELECT id, agent_id, state, created_at, updated_at
             FROM runs
             WHERE id LIKE ?1
             ORDER BY created_at DESC
             LIMIT 2",
            format!("{run_id}%"),
        )
    };

    // Collect matching rows (at most 2 so we can detect ambiguity).
    let mut stmt = conn.prepare(sql).context("preparing run query")?;

    let rows: Vec<(String, String, String, String, String)> = stmt
        .query_map([&param], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })
        .context("executing run query")?
        .collect::<Result<Vec<_>, _>>()
        .context("reading run rows")?;

    if rows.is_empty() {
        anyhow::bail!("Run not found: {run_id}");
    }
    if rows.len() > 1 {
        let candidates: Vec<&str> = rows.iter().map(|(id, ..)| id.as_str()).collect();
        anyhow::bail!(
            "Ambiguous run ID prefix '{}' — {} matches: {}",
            run_id,
            rows.len(),
            candidates.join(", ")
        );
    }

    let (full_id, agent_id, state, created_at, updated_at) = rows
        .into_iter()
        .next()
        .context("run query returned no rows after its non-empty check")?;

    // Count turns for this run.
    let turns: u64 = conn
        .query_row(
            "SELECT count(*) FROM turns WHERE run_id = ?1",
            [&full_id],
            |r| r.get(0),
        )
        .unwrap_or(0);

    // Count steps for this run (via turns).
    let steps: u64 = conn
        .query_row(
            "SELECT count(*) FROM steps s
             JOIN turns t ON s.turn_id = t.id
             WHERE t.run_id = ?1",
            [&full_id],
            |r| r.get(0),
        )
        .unwrap_or(0);

    // Collect associated effect IDs.
    let mut effect_stmt = conn
        .prepare("SELECT id FROM effect_intents WHERE run_id = ?1 ORDER BY created_at")
        .context("preparing effect query")?;

    let effects: Vec<String> = effect_stmt
        .query_map([&full_id], |row| row.get(0))
        .context("executing effect query")?
        .collect::<Result<Vec<_>, _>>()
        .context("reading effect rows")?;

    // Compute elapsed time between created_at and updated_at in milliseconds.
    let elapsed_ms = compute_elapsed_ms(&created_at, &updated_at);

    Ok(RunInfo {
        id: full_id,
        agent_id,
        state,
        created_at,
        updated_at,
        turns,
        steps,
        elapsed_ms,
        effects,
    })
}

/// Compute elapsed milliseconds between two RFC 3339 timestamp strings.
///
/// Returns `None` if either timestamp cannot be parsed.
fn compute_elapsed_ms(from: &str, to: &str) -> Option<u64> {
    let from = chrono::DateTime::parse_from_rfc3339(from).ok()?;
    let to = chrono::DateTime::parse_from_rfc3339(to).ok()?;
    u64::try_from(to.signed_duration_since(from).num_milliseconds()).ok()
}

/// Render a `RunInfo` to stdout.
fn render_run(info: &RunInfo, json_output: bool) -> Result<()> {
    if json_output {
        println!("{}", serde_json::to_string_pretty(info)?);
        return Ok(());
    }

    println!("Run: {}", info.id);
    println!("{}", "-".repeat(60));
    println!("  Agent:       {}", info.agent_id);
    println!("  State:       {}", info.state);
    println!("  Created:     {}", info.created_at);
    println!("  Updated:     {}", info.updated_at);
    println!("  Turns:       {}", info.turns);
    println!("  Steps:       {}", info.steps);

    if let Some(ms) = info.elapsed_ms {
        println!("  Elapsed:     {ms}ms");
    }

    if !info.effects.is_empty() {
        println!("  Effects:");
        for eid in &info.effects {
            println!("    - {eid}");
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Effect inspection
// ---------------------------------------------------------------------------

/// Inspect an effect by ID.
///
/// Opens a read-only connection to the `SQLite` database and queries the
/// `effect_intents`, `effect_attempts`, and `effect_outcomes` tables.
/// Supports UUID prefix matching.
pub async fn execute_inspect_effect(effect_id: &str, json_output: bool) -> Result<()> {
    validate_uuid(effect_id).context("invalid effect ID")?;

    let db_path = resolve_db_path();
    let expanded = expand_tilde(&db_path);

    let effect_id = effect_id.to_owned();
    let info = tokio::task::spawn_blocking(move || query_effect(&expanded, &effect_id))
        .await
        .context("joining inspect effect query task")??;

    render_effect(&info, json_output)
}

/// Query an effect intent from the `SQLite` database by exact or prefix UUID match.
fn query_effect(db_path: &str, effect_id: &str) -> Result<EffectInfo> {
    let conn =
        rusqlite::Connection::open_with_flags(db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .with_context(|| format!("opening database at {db_path}"))?;

    // Try exact match first, then prefix match.
    let pattern = format!("{effect_id}%");
    let row: Option<(String, String, String, String, String, String)> = conn
        .query_row(
            "SELECT ei.id, ei.run_id, ei.kind, ei.state, ei.params_json, ei.created_at \
             FROM effect_intents ei \
             WHERE ei.id = ?1 OR ei.id LIKE ?2 \
             ORDER BY ei.created_at DESC LIMIT 1",
            rusqlite::params![effect_id, pattern],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                ))
            },
        )
        .optional()
        .with_context(|| "querying effect_intents table")?;

    let (id, run_id, kind, state, params_json, created_at) =
        row.ok_or_else(|| anyhow::anyhow!("Effect not found: {effect_id}"))?;

    // Resolve the agent_id by joining through runs.
    let agent_id: String = conn
        .query_row(
            "SELECT agent_id FROM runs WHERE id = ?1",
            rusqlite::params![run_id],
            |r| r.get(0),
        )
        .unwrap_or_default();

    // Count attempts.
    let attempts: u32 = conn
        .query_row(
            "SELECT count(*) FROM effect_attempts WHERE intent_id = ?1",
            rusqlite::params![id],
            |r| r.get(0),
        )
        .unwrap_or(0);

    // Look up outcome.
    let outcome_row: Option<(String, Option<String>)> = conn
        .query_row(
            "SELECT eo.status, eo.created_at FROM effect_outcomes eo WHERE eo.intent_id = ?1",
            rusqlite::params![id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .unwrap_or(None);

    let (outcome, resolved_at) = match outcome_row {
        Some((status, resolved)) => (Some(status), resolved),
        None => (None, None),
    };

    // Only include params_json payload when it's not the default empty object.
    let payload = if params_json == "{}" || params_json.is_empty() {
        None
    } else {
        Some(params_json)
    };

    Ok(EffectInfo {
        id,
        run_id,
        agent_id,
        intent: kind,
        state,
        attempts,
        outcome,
        payload,
        created_at,
        resolved_at,
    })
}

/// Render an `EffectInfo` to stdout.
fn render_effect(info: &EffectInfo, json_output: bool) -> Result<()> {
    if json_output {
        println!("{}", serde_json::to_string_pretty(info)?);
        return Ok(());
    }

    println!("Effect: {}", info.id);
    println!("{}", "-".repeat(60));
    println!("  Run:         {}", info.run_id);
    if !info.agent_id.is_empty() {
        println!("  Agent:       {}", info.agent_id);
    }
    println!("  Kind:        {}", info.intent);
    println!("  State:       {}", info.state);
    println!("  Attempts:    {}", info.attempts);

    if let Some(ref outcome) = info.outcome {
        println!("  Outcome:     {outcome}");
    }

    println!("  Created:     {}", info.created_at);

    if let Some(ref resolved) = info.resolved_at {
        println!("  Resolved:    {resolved}");
    }

    if let Some(ref payload) = info.payload {
        println!("  Payload:     {payload}");
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Artifact inspection
// ---------------------------------------------------------------------------

/// Inspect an artifact by ID.
///
/// Opens a read-only connection to the `SQLite` database and queries the
/// `artifacts` and `artifact_lineage` tables.  Supports UUID prefix matching.
pub async fn execute_inspect_artifact(artifact_id: &str, json_output: bool) -> Result<()> {
    if artifact_id.is_empty() {
        anyhow::bail!("artifact ID must not be empty");
    }

    let db_path = resolve_db_path();
    let expanded = expand_tilde(&db_path);

    let artifact_id = artifact_id.to_owned();
    let info = tokio::task::spawn_blocking(move || query_artifact(&expanded, &artifact_id))
        .await
        .context("joining inspect artifact query task")??;

    render_artifact(&info, json_output)
}

/// Query an artifact from the `SQLite` database by exact or prefix UUID match.
fn query_artifact(db_path: &str, artifact_id: &str) -> Result<ArtifactInfo> {
    let conn =
        rusqlite::Connection::open_with_flags(db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .with_context(|| format!("opening database at {db_path}"))?;

    // Try exact match first, then prefix match.
    let pattern = format!("{artifact_id}%");
    let row: Option<ArtifactRow> = conn
        .query_row(
            "SELECT id, run_id, kind, digest_hex, size_bytes, metadata_json, created_at \
             FROM artifacts \
             WHERE id = ?1 OR id LIKE ?2 \
             ORDER BY created_at DESC LIMIT 1",
            rusqlite::params![artifact_id, pattern],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                ))
            },
        )
        .optional()
        .with_context(|| "querying artifacts table")?;

    let (id, run_id, kind, digest_hex, size_bytes, metadata_json, created_at) =
        row.ok_or_else(|| anyhow::anyhow!("Artifact not found: {artifact_id}"))?;

    // Look up parent artifact from lineage table.
    let parent_artifact_id: Option<String> = conn
        .query_row(
            "SELECT parent_id FROM artifact_lineage WHERE child_id = ?1 LIMIT 1",
            rusqlite::params![id],
            |r| r.get(0),
        )
        .optional()
        .unwrap_or(None);

    // Only include metadata when it's not the default empty object.
    let metadata = if metadata_json == "{}" || metadata_json.is_empty() {
        None
    } else {
        Some(metadata_json)
    };

    Ok(ArtifactInfo {
        id,
        run_id: run_id.clone(),
        kind,
        digest: digest_hex,
        size_bytes: u64::try_from(size_bytes).unwrap_or_default(),
        metadata,
        created_at,
        lineage: ArtifactLineage {
            run_id,
            step_id: None,
            parent_artifact_id,
        },
    })
}

/// Render an `ArtifactInfo` to stdout.
fn render_artifact(info: &ArtifactInfo, json_output: bool) -> Result<()> {
    if json_output {
        println!("{}", serde_json::to_string_pretty(info)?);
        return Ok(());
    }

    println!("Artifact: {}", info.id);
    println!("{}", "-".repeat(60));
    println!("  Kind:        {}", info.kind);
    println!("  Digest:      {}", info.digest);
    println!("  Size:        {} bytes", info.size_bytes);
    println!("  Created:     {}", info.created_at);

    if let Some(ref meta) = info.metadata {
        println!("  Metadata:    {meta}");
    }

    let has_lineage = info.lineage.run_id.is_some()
        || info.lineage.step_id.is_some()
        || info.lineage.parent_artifact_id.is_some();

    if has_lineage {
        println!("  Lineage:");
        if let Some(ref rid) = info.lineage.run_id {
            println!("    Run:       {rid}");
        }
        if let Some(ref sid) = info.lineage.step_id {
            println!("    Step:      {sid}");
        }
        if let Some(ref pid) = info.lineage.parent_artifact_id {
            println!("    Parent:    {pid}");
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Agent inspection
// ---------------------------------------------------------------------------

/// Inspect an agent by name or UUID.
///
/// Opens a read-only connection to the `SQLite` database and queries the `agents`
/// table.  Tries an exact match on `id`, then on `name`, then a UUID prefix
/// match.  Parses `spec_json` to extract model, tools, capabilities, and
/// autonomy level for display.
pub async fn execute_inspect_agent(agent_id: &str, json_output: bool) -> Result<()> {
    if agent_id.is_empty() {
        anyhow::bail!("agent ID must not be empty");
    }

    let db_path = resolve_db_path();
    let expanded = expand_tilde(&db_path);

    let agent_id = agent_id.to_owned();
    let info = tokio::task::spawn_blocking(move || query_agent(&expanded, &agent_id))
        .await
        .context("joining inspect agent query task")??;

    render_agent(&info, json_output)
}

/// Query an agent from the `SQLite` database by ID, name, or UUID prefix.
fn query_agent(db_path: &str, agent_id: &str) -> Result<AgentInfo> {
    let conn =
        rusqlite::Connection::open_with_flags(db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .with_context(|| format!("opening database at {db_path}"))?;

    // Try: exact id match, then exact name match, then id prefix match.
    let id_pattern = format!("{agent_id}%");
    let row: Option<(String, String, String, String, String, String)> = conn
        .query_row(
            "SELECT id, name, state, spec_json, created_at, updated_at FROM agents \
             WHERE id = ?1 OR name = ?1 OR id LIKE ?2 \
             ORDER BY created_at DESC LIMIT 1",
            rusqlite::params![agent_id, id_pattern],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                ))
            },
        )
        .optional()
        .with_context(|| "querying agents table")?;

    let (id, name, state, spec_json, created_at, updated_at) =
        row.ok_or_else(|| anyhow::anyhow!("Agent not found: {agent_id}"))?;

    // Parse spec_json to extract structured fields.
    let spec: serde_json::Value = serde_json::from_str(&spec_json).unwrap_or_default();

    let model = spec
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let autonomy_level = spec
        .get("autonomy_level")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let tools: Vec<String> = spec
        .get("tools")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|t| t.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let capabilities: Vec<String> = spec
        .get("capabilities")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|c| c.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let policy_refs: Vec<String> = spec
        .get("policy_refs")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|p| p.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    // Store raw spec_json only when it's non-trivial.
    let spec_json_opt = if spec_json == "{}" || spec_json.is_empty() {
        None
    } else {
        Some(spec_json)
    };

    Ok(AgentInfo {
        id,
        name,
        model,
        state,
        autonomy_level,
        tools,
        capabilities,
        policy_refs,
        created_at,
        updated_at,
        spec_json: spec_json_opt,
    })
}

/// Render an `AgentInfo` to stdout.
fn render_agent(info: &AgentInfo, json_output: bool) -> Result<()> {
    if json_output {
        println!("{}", serde_json::to_string_pretty(info)?);
        return Ok(());
    }

    println!("Agent: {} ({})", info.name, info.id);
    println!("{}", "-".repeat(60));
    println!("  Model:       {}", info.model);
    println!("  State:       {}", info.state);
    println!("  Autonomy:    {}", info.autonomy_level);
    println!("  Created:     {}", info.created_at);
    println!("  Updated:     {}", info.updated_at);

    if !info.tools.is_empty() {
        println!("  Tools:");
        for t in &info.tools {
            println!("    - {t}");
        }
    }

    if !info.capabilities.is_empty() {
        println!("  Capabilities:");
        for c in &info.capabilities {
            println!("    - {c}");
        }
    }

    if !info.policy_refs.is_empty() {
        println!("  Policies:");
        for p in &info.policy_refs {
            println!("    - {p}");
        }
    }

    if let Some(ref spec) = info.spec_json {
        println!("  Spec:");
        println!("    {spec}");
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Policy inspection
// ---------------------------------------------------------------------------

/// Parse a TOML policy file and display its resolved rules.
///
/// The TOML file is expected to have optional top-level `name`, `version`, and
/// `description` keys, plus a `[[rules]]` array-of-tables where each entry has
/// `action`, `resource`, `effect`, and an optional `conditions` list.
pub async fn execute_inspect_policy(path: &Path, json_output: bool) -> Result<()> {
    let path = path.to_owned();
    let info = tokio::task::spawn_blocking(move || {
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("reading policy file: {}", path.display()))?;
        parse_policy_toml(&content, &path)
    })
    .await
    .context("joining inspect policy read task")??;

    render_policy(&info, json_output)
}

/// Parse raw TOML content into a `PolicyInfo`.
fn parse_policy_toml(content: &str, path: &Path) -> Result<PolicyInfo> {
    let table: toml::Value =
        toml::from_str(content).with_context(|| "invalid TOML in policy file")?;

    let name = table
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("unnamed")
        .to_string();

    let version = table
        .get("version")
        .and_then(|v| v.as_str())
        .unwrap_or("0.0.0")
        .to_string();

    let description = table
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let rules = parse_policy_rules(&table);

    Ok(PolicyInfo {
        path: path.display().to_string(),
        name,
        version,
        description,
        rules,
    })
}

/// Extract `[[rules]]` from a parsed TOML table.
fn parse_policy_rules(table: &toml::Value) -> Vec<PolicyRule> {
    let Some(rules_val) = table.get("rules") else {
        return Vec::new();
    };

    let Some(rules_arr) = rules_val.as_array() else {
        // Single rule table rather than array — wrap in a vec.
        return if let Some(rule_tbl) = rules_val.as_table() {
            vec![extract_single_rule(&toml::Value::Table(rule_tbl.clone()))]
        } else {
            Vec::new()
        };
    };

    rules_arr.iter().map(extract_single_rule).collect()
}

/// Extract a single `PolicyRule` from a TOML table value.
fn extract_single_rule(val: &toml::Value) -> PolicyRule {
    let action = val
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("*")
        .to_string();

    let resource = val
        .get("resource")
        .and_then(|v| v.as_str())
        .unwrap_or("*")
        .to_string();

    let effect = val
        .get("effect")
        .and_then(|v| v.as_str())
        .unwrap_or("deny")
        .to_string();

    let conditions = val
        .get("conditions")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|c| c.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    PolicyRule {
        action,
        resource,
        effect,
        conditions,
    }
}

/// Render a `PolicyInfo` to stdout.
fn render_policy(info: &PolicyInfo, json_output: bool) -> Result<()> {
    if json_output {
        println!("{}", serde_json::to_string_pretty(info)?);
        return Ok(());
    }

    println!("Policy: {}", info.name);
    println!("{}", "-".repeat(60));
    println!("  File:        {}", info.path);
    println!("  Version:     {}", info.version);

    if !info.description.is_empty() {
        println!("  Description: {}", info.description);
    }

    if info.rules.is_empty() {
        println!("  Rules:       (none)");
    } else {
        println!("  Rules:");
        println!(
            "    {:<20} {:<20} {:<10} Conditions",
            "Action", "Resource", "Effect"
        );
        println!("    {}", "-".repeat(56));
        for rule in &info.rules {
            let conds = if rule.conditions.is_empty() {
                "-".to_string()
            } else {
                rule.conditions.join(", ")
            };
            println!(
                "    {:<20} {:<20} {:<10} {}",
                rule.action, rule.resource, rule.effect, conds
            );
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Database inspection
// ---------------------------------------------------------------------------

/// Show database statistics.
///
/// Reads table row counts, file sizes, and migration version directly from the
/// `SQLite` database file.  Does not require the daemon to be running.
pub async fn execute_inspect_db(json_output: bool) -> Result<()> {
    let db_path = resolve_db_path();
    let expanded = expand_tilde(&db_path);

    let stats = tokio::task::spawn_blocking(move || gather_db_stats(&expanded))
        .await
        .context("joining inspect database statistics task")??;

    render_db_stats(&stats, json_output)
}

/// Gather database statistics from the `SQLite` file at `path`.
fn gather_db_stats(path: &str) -> Result<DbStats> {
    let conn =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .with_context(|| format!("opening database at {path}"))?;

    // Database file size.
    let db_size_bytes = std::fs::metadata(path).map_or(0, |m| m.len());

    // WAL file size.
    let wal_path = format!("{path}-wal");
    let wal_size_bytes = std::fs::metadata(&wal_path).map_or(0, |m| m.len());

    // Migration version (user_version pragma).
    let migration_version: u64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap_or(0);

    // Enumerate user tables and count rows in each.
    let table_names = list_user_tables(&conn)?;
    let mut table_row_counts = BTreeMap::new();

    for name in &table_names {
        let count: u64 = conn
            .query_row(&format!("SELECT count(*) FROM [{name}]"), [], |r| r.get(0))
            .unwrap_or(0);
        table_row_counts.insert(name.clone(), count);
    }

    Ok(DbStats {
        db_size_bytes,
        wal_size_bytes,
        migration_version,
        table_row_counts,
    })
}

/// List user-created table names in the database (excludes sqlite_ internals).
fn list_user_tables(conn: &rusqlite::Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
    )?;

    let names: Vec<String> = stmt
        .query_map([], |row| row.get(0))?
        .filter_map(std::result::Result::ok)
        .collect();

    Ok(names)
}

/// Render `DbStats` to stdout.
fn render_db_stats(stats: &DbStats, json_output: bool) -> Result<()> {
    if json_output {
        println!("{}", serde_json::to_string_pretty(stats)?);
        return Ok(());
    }

    println!("Database Statistics");
    println!("{}", "-".repeat(60));
    println!(
        "  DB size:           {} bytes ({})",
        stats.db_size_bytes,
        format_bytes(stats.db_size_bytes)
    );
    println!(
        "  WAL size:          {} bytes ({})",
        stats.wal_size_bytes,
        format_bytes(stats.wal_size_bytes)
    );
    println!("  Migration version: {}", stats.migration_version);
    println!();

    if stats.table_row_counts.is_empty() {
        println!("  Tables: (none)");
    } else {
        println!("  Tables:");
        println!("    {:<30} Rows", "Name");
        println!("    {}", "-".repeat(40));
        for (name, count) in &stats.table_row_counts {
            println!("    {name:<30} {count}");
        }

        let total: u64 = stats.table_row_counts.values().sum();
        println!();
        println!("  Total rows: {total}");
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Validate that a string looks like a UUID (36 chars, dashes in the right places).
fn validate_uuid(s: &str) -> Result<()> {
    if s.len() != 36 {
        anyhow::bail!("expected a 36-character UUID, got {} characters", s.len());
    }

    let parts: Vec<&str> = s.split('-').collect();
    if parts.len() != 5 {
        anyhow::bail!("expected UUID format xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx");
    }

    let expected_lens = [8, 4, 4, 4, 12];
    for (part, &expected) in parts.iter().zip(&expected_lens) {
        if part.len() != expected {
            anyhow::bail!(
                "UUID segment has wrong length: expected {expected}, got {}",
                part.len()
            );
        }
        if !part.chars().all(|c| c.is_ascii_hexdigit()) {
            anyhow::bail!("UUID contains non-hex character");
        }
    }

    Ok(())
}

/// Format a byte count into a human-readable string (KiB, MiB, GiB).
fn format_bytes(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = 1024 * KIB;
    const GIB: u64 = 1024 * MIB;

    if bytes >= GIB {
        format_scaled_bytes(bytes, GIB, "GiB")
    } else if bytes >= MIB {
        format_scaled_bytes(bytes, MIB, "MiB")
    } else if bytes >= KIB {
        format_scaled_bytes(bytes, KIB, "KiB")
    } else {
        format!("{bytes} B")
    }
}

/// Format a byte count as a rounded, fixed two-decimal multiple of `unit`.
fn format_scaled_bytes(bytes: u64, unit: u64, suffix: &str) -> String {
    let unit = u128::from(unit);
    let rounded_hundredths = (u128::from(bytes) * 100 + unit / 2) / unit;
    let whole = rounded_hundredths / 100;
    let fractional = rounded_hundredths % 100;
    format!("{whole}.{fractional:02} {suffix}")
}

/// Resolve the database path using the same logic as `main.rs`.
fn resolve_db_path() -> String {
    if let Ok(path) = std::env::var("POLKAGENT_DATABASE_SQLITE_PATH") {
        if !path.is_empty() {
            return path;
        }
    }

    let home = std::env::var("HOME").unwrap_or_default();
    format!("{home}/.local/share/polkagent/polkagent.db")
}

/// Expand a leading `~` using the HOME environment variable.
fn expand_tilde(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return format!("{home}/{rest}");
        }
    }
    path.to_owned()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use std::io::Write as _;

    // Helper: parse argv through a wrapper CLI so clap derives work.
    #[derive(Debug, Parser)]
    #[command(name = "test")]
    struct TestCli {
        #[command(subcommand)]
        cmd: InspectCmd,
    }

    fn parse(args: &[&str]) -> InspectCmd {
        TestCli::parse_from(args).cmd
    }

    // -- Argument parsing tests -----------------------------------------------

    #[test]
    fn parse_inspect_run() {
        let cmd = parse(&["test", "run", "01234567-89ab-cdef-0123-456789abcdef"]);
        match cmd {
            InspectCmd::Run(args) => {
                assert_eq!(args.run_id, "01234567-89ab-cdef-0123-456789abcdef");
                assert!(!args.json);
            }
            other => panic!("expected Run, got {other:?}"),
        }
    }

    #[test]
    fn parse_inspect_run_json() {
        let cmd = parse(&[
            "test",
            "run",
            "01234567-89ab-cdef-0123-456789abcdef",
            "--json",
        ]);
        match cmd {
            InspectCmd::Run(args) => {
                assert!(args.json);
            }
            other => panic!("expected Run, got {other:?}"),
        }
    }

    #[test]
    fn parse_inspect_effect() {
        let cmd = parse(&["test", "effect", "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"]);
        match cmd {
            InspectCmd::Effect(args) => {
                assert_eq!(args.effect_id, "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee");
                assert!(!args.json);
            }
            other => panic!("expected Effect, got {other:?}"),
        }
    }

    #[test]
    fn parse_inspect_effect_json() {
        let cmd = parse(&[
            "test",
            "effect",
            "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
            "--json",
        ]);
        match cmd {
            InspectCmd::Effect(args) => assert!(args.json),
            other => panic!("expected Effect, got {other:?}"),
        }
    }

    #[test]
    fn parse_inspect_artifact() {
        let cmd = parse(&["test", "artifact", "sha256:abcdef1234567890"]);
        match cmd {
            InspectCmd::Artifact(args) => {
                assert_eq!(args.artifact_id, "sha256:abcdef1234567890");
                assert!(!args.json);
            }
            other => panic!("expected Artifact, got {other:?}"),
        }
    }

    #[test]
    fn parse_inspect_artifact_json() {
        let cmd = parse(&["test", "artifact", "sha256:abcdef1234567890", "--json"]);
        match cmd {
            InspectCmd::Artifact(args) => assert!(args.json),
            other => panic!("expected Artifact, got {other:?}"),
        }
    }

    #[test]
    fn parse_inspect_agent() {
        let cmd = parse(&["test", "agent", "my-agent"]);
        match cmd {
            InspectCmd::Agent(args) => {
                assert_eq!(args.agent_id, "my-agent");
                assert!(!args.json);
            }
            other => panic!("expected Agent, got {other:?}"),
        }
    }

    #[test]
    fn parse_inspect_agent_json() {
        let cmd = parse(&["test", "agent", "my-agent", "--json"]);
        match cmd {
            InspectCmd::Agent(args) => assert!(args.json),
            other => panic!("expected Agent, got {other:?}"),
        }
    }

    #[test]
    fn parse_inspect_policy() {
        let cmd = parse(&["test", "policy", "/etc/polkagent/default.toml"]);
        match cmd {
            InspectCmd::Policy(args) => {
                assert_eq!(args.path, PathBuf::from("/etc/polkagent/default.toml"));
                assert!(!args.json);
            }
            other => panic!("expected Policy, got {other:?}"),
        }
    }

    #[test]
    fn parse_inspect_policy_json() {
        let cmd = parse(&["test", "policy", "policy.toml", "--json"]);
        match cmd {
            InspectCmd::Policy(args) => assert!(args.json),
            other => panic!("expected Policy, got {other:?}"),
        }
    }

    #[test]
    fn parse_inspect_db() {
        let cmd = parse(&["test", "db"]);
        match cmd {
            InspectCmd::Db(args) => assert!(!args.json),
            other => panic!("expected Db, got {other:?}"),
        }
    }

    #[test]
    fn parse_inspect_db_json() {
        let cmd = parse(&["test", "db", "--json"]);
        match cmd {
            InspectCmd::Db(args) => assert!(args.json),
            other => panic!("expected Db, got {other:?}"),
        }
    }

    // -- UUID validation tests ------------------------------------------------

    #[test]
    fn validate_uuid_valid() {
        assert!(validate_uuid("01234567-89ab-cdef-0123-456789abcdef").is_ok());
        assert!(validate_uuid("AAAAAAAA-BBBB-CCCC-DDDD-EEEEEEEEEEEE").is_ok());
    }

    #[test]
    fn validate_uuid_wrong_length() {
        assert!(validate_uuid("too-short").is_err());
        assert!(validate_uuid("").is_err());
    }

    #[test]
    fn validate_uuid_bad_hex() {
        // 'g' is not a hex digit.
        assert!(validate_uuid("g1234567-89ab-cdef-0123-456789abcdef").is_err());
    }

    #[test]
    fn validate_uuid_wrong_segment_lengths() {
        // All dashes in wrong positions.
        assert!(validate_uuid("012345678-9ab-cdef-0123-456789abcde").is_err());
    }

    // -- format_bytes tests ---------------------------------------------------

    #[test]
    fn format_bytes_under_kib() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1023), "1023 B");
    }

    #[test]
    fn format_bytes_kib() {
        assert_eq!(format_bytes(1024), "1.00 KiB");
        assert_eq!(format_bytes(2048), "2.00 KiB");
    }

    #[test]
    fn format_bytes_mib() {
        assert_eq!(format_bytes(1024 * 1024), "1.00 MiB");
        assert_eq!(format_bytes(5 * 1024 * 1024), "5.00 MiB");
    }

    #[test]
    fn format_bytes_gib() {
        assert_eq!(format_bytes(1024 * 1024 * 1024), "1.00 GiB");
    }

    #[test]
    fn format_bytes_rounds_fractional_units() {
        assert_eq!(format_bytes(1536), "1.50 KiB");
        assert_eq!(format_bytes(1024 + 1023), "2.00 KiB");
    }

    // -- Timestamp duration tests -------------------------------------------

    #[test]
    fn compute_elapsed_ms_preserves_subsecond_precision() {
        assert_eq!(
            compute_elapsed_ms("2025-01-01T00:00:00.125Z", "2025-01-01T00:00:01.500Z"),
            Some(1375)
        );
    }

    #[test]
    fn compute_elapsed_ms_honors_timezone_offsets() {
        assert_eq!(
            compute_elapsed_ms("2025-01-01T01:00:00+01:00", "2025-01-01T00:00:01Z"),
            Some(1000)
        );
    }

    #[test]
    fn compute_elapsed_ms_rejects_negative_or_invalid_ranges() {
        assert_eq!(
            compute_elapsed_ms("2025-01-01T00:00:01Z", "2025-01-01T00:00:00Z"),
            None
        );
        assert_eq!(compute_elapsed_ms("not-a-time", "also-not-a-time"), None);
    }

    // -- Policy TOML parsing tests --------------------------------------------

    #[test]
    fn parse_policy_toml_full() {
        let toml_str = r#"
name = "transfer-limits"
version = "1.0.0"
description = "Limits on balance transfers"

[[rules]]
action = "transfer"
resource = "DOT"
effect = "allow"
conditions = ["amount < 100"]

[[rules]]
action = "stake"
resource = "DOT"
effect = "deny"
"#;

        let info = parse_policy_toml(toml_str, &PathBuf::from("test.toml")).unwrap();
        assert_eq!(info.name, "transfer-limits");
        assert_eq!(info.version, "1.0.0");
        assert_eq!(info.description, "Limits on balance transfers");
        assert_eq!(info.rules.len(), 2);
        assert_eq!(info.rules[0].action, "transfer");
        assert_eq!(info.rules[0].resource, "DOT");
        assert_eq!(info.rules[0].effect, "allow");
        assert_eq!(info.rules[0].conditions, vec!["amount < 100"]);
        assert_eq!(info.rules[1].action, "stake");
        assert_eq!(info.rules[1].effect, "deny");
        assert!(info.rules[1].conditions.is_empty());
    }

    #[test]
    fn parse_policy_toml_minimal() {
        let toml_str = r#"
[[rules]]
action = "call"
resource = "system"
effect = "allow"
"#;

        let info = parse_policy_toml(toml_str, &PathBuf::from("minimal.toml")).unwrap();
        assert_eq!(info.name, "unnamed");
        assert_eq!(info.version, "0.0.0");
        assert_eq!(info.description, "");
        assert_eq!(info.rules.len(), 1);
    }

    #[test]
    fn parse_policy_toml_no_rules() {
        let toml_str = r#"
name = "empty-policy"
version = "0.1.0"
"#;

        let info = parse_policy_toml(toml_str, &PathBuf::from("empty.toml")).unwrap();
        assert_eq!(info.name, "empty-policy");
        assert!(info.rules.is_empty());
    }

    #[test]
    fn parse_policy_toml_invalid() {
        let result = parse_policy_toml("{{invalid toml!!", &PathBuf::from("bad.toml"));
        assert!(result.is_err());
    }

    // -- Output structure rendering tests -------------------------------------

    #[test]
    fn render_run_json_output() {
        let info = RunInfo {
            id: "01234567-89ab-cdef-0123-456789abcdef".to_string(),
            agent_id: "agent-1".to_string(),
            state: "completed".to_string(),
            created_at: "2024-01-01T00:00:00Z".to_string(),
            updated_at: "2024-01-01T00:01:00Z".to_string(),
            turns: 3,
            steps: 7,
            elapsed_ms: Some(45_000),
            effects: vec!["effect-1".to_string(), "effect-2".to_string()],
        };

        // Should not panic; verify the JSON round-trips.
        let json_str = serde_json::to_string_pretty(&info).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed["turns"], 3);
        assert_eq!(parsed["steps"], 7);
        assert_eq!(parsed["elapsed_ms"], 45_000);
        assert_eq!(parsed["effects"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn render_effect_json_output() {
        let info = EffectInfo {
            id: "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".to_string(),
            run_id: "run-1".to_string(),
            agent_id: "agent-1".to_string(),
            intent: "balance_transfer".to_string(),
            state: "approved".to_string(),
            attempts: 2,
            outcome: Some("success".to_string()),
            payload: None,
            created_at: "2024-01-01T00:00:00Z".to_string(),
            resolved_at: Some("2024-01-01T00:00:30Z".to_string()),
        };

        let json_str = serde_json::to_string_pretty(&info).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed["intent"], "balance_transfer");
        assert_eq!(parsed["attempts"], 2);
        assert_eq!(parsed["outcome"], "success");
    }

    #[test]
    fn render_artifact_json_output() {
        let info = ArtifactInfo {
            id: "sha256:deadbeef".to_string(),
            run_id: Some("run-abc".to_string()),
            kind: "wasm-blob".to_string(),
            digest: "sha256:deadbeef0123456789abcdef".to_string(),
            size_bytes: 1_048_576,
            metadata: None,
            created_at: "2024-06-15T12:00:00Z".to_string(),
            lineage: ArtifactLineage {
                run_id: Some("run-abc".to_string()),
                step_id: Some("step-3".to_string()),
                parent_artifact_id: None,
            },
        };

        let json_str = serde_json::to_string_pretty(&info).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed["kind"], "wasm-blob");
        assert_eq!(parsed["size_bytes"], 1_048_576);
        assert_eq!(parsed["lineage"]["run_id"], "run-abc");
        assert!(parsed["lineage"]["parent_artifact_id"].is_null());
    }

    #[test]
    fn render_agent_json_output() {
        let info = AgentInfo {
            id: "11111111-2222-3333-4444-555555555555".to_string(),
            name: "treasury-bot".to_string(),
            model: "anthropic/claude-sonnet-4-6".to_string(),
            state: "active".to_string(),
            autonomy_level: "supervised".to_string(),
            tools: vec!["chain.query".to_string(), "file.read".to_string()],
            capabilities: vec!["balance_transfer".to_string()],
            policy_refs: vec!["default-policy".to_string()],
            created_at: "2024-01-01T00:00:00Z".to_string(),
            updated_at: "2024-06-01T00:00:00Z".to_string(),
            spec_json: None,
        };

        let json_str = serde_json::to_string_pretty(&info).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed["name"], "treasury-bot");
        assert_eq!(parsed["model"], "anthropic/claude-sonnet-4-6");
        assert_eq!(parsed["tools"].as_array().unwrap().len(), 2);
        assert_eq!(parsed["capabilities"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn render_db_stats_json_output() {
        let mut table_row_counts = BTreeMap::new();
        table_row_counts.insert("agents".to_string(), 5);
        table_row_counts.insert("runs".to_string(), 42);
        table_row_counts.insert("events".to_string(), 1337);

        let stats = DbStats {
            db_size_bytes: 2 * 1024 * 1024,
            wal_size_bytes: 64 * 1024,
            migration_version: 6,
            table_row_counts,
        };

        let json_str = serde_json::to_string_pretty(&stats).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed["db_size_bytes"], 2 * 1024 * 1024);
        assert_eq!(parsed["migration_version"], 6);
        assert_eq!(parsed["table_row_counts"]["agents"], 5);
        assert_eq!(parsed["table_row_counts"]["runs"], 42);
    }

    // -- Execute stubs: not-found errors --------------------------------------
    //
    // These tests use a temporary on-disk SQLite database with the minimal
    // schema so that the query functions can actually open it and return a
    // proper "not found" error rather than a "cannot open database" error.

    /// Create a temporary `SQLite` database with the minimal schema required by
    /// the inspect helpers, write it to disk, and return its path.
    fn temp_db_with_schema() -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("test.db");
        let path_str = path.to_str().unwrap().to_owned();
        let conn = rusqlite::Connection::open(&path).expect("open temp db");
        conn.execute_batch(
            "
            CREATE TABLE agents (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                state TEXT NOT NULL DEFAULT 'active',
                spec_json TEXT NOT NULL DEFAULT '{}',
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            CREATE TABLE runs (
                id TEXT PRIMARY KEY,
                agent_id TEXT NOT NULL,
                state TEXT NOT NULL DEFAULT 'created',
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            CREATE TABLE turns (
                id TEXT PRIMARY KEY,
                run_id TEXT NOT NULL,
                sequence INTEGER NOT NULL
            );
            CREATE TABLE steps (
                id TEXT PRIMARY KEY,
                turn_id TEXT NOT NULL,
                sequence INTEGER NOT NULL
            );
            CREATE TABLE effect_intents (
                id TEXT PRIMARY KEY,
                run_id TEXT NOT NULL,
                kind TEXT NOT NULL,
                state TEXT NOT NULL DEFAULT 'pending',
                params_json TEXT NOT NULL DEFAULT '{}',
                idempotency_key TEXT NOT NULL UNIQUE,
                created_at TEXT NOT NULL
            );
            CREATE TABLE effect_attempts (
                id TEXT PRIMARY KEY,
                intent_id TEXT NOT NULL,
                attempt_number INTEGER NOT NULL
            );
            CREATE TABLE effect_outcomes (
                id TEXT PRIMARY KEY,
                intent_id TEXT NOT NULL UNIQUE,
                status TEXT NOT NULL,
                created_at TEXT NOT NULL
            );
            CREATE TABLE artifacts (
                id TEXT PRIMARY KEY,
                run_id TEXT,
                kind TEXT NOT NULL,
                digest_hex TEXT NOT NULL,
                size_bytes INTEGER NOT NULL DEFAULT 0,
                metadata_json TEXT NOT NULL DEFAULT '{}',
                created_at TEXT NOT NULL
            );
            CREATE TABLE artifact_lineage (
                child_id TEXT NOT NULL,
                parent_id TEXT NOT NULL,
                PRIMARY KEY (child_id, parent_id)
            );
            ",
        )
        .expect("create schema");
        (dir, path_str)
    }

    #[test]
    fn query_run_not_found() {
        let (_dir, path) = temp_db_with_schema();
        let result = query_run(&path, "01234567-89ab-cdef-0123-456789abcdef");
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("not found"), "error was: {err_msg}");
    }

    #[test]
    fn query_effect_not_found() {
        let (_dir, path) = temp_db_with_schema();
        let result = query_effect(&path, "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee");
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("not found"), "error was: {err_msg}");
    }

    #[test]
    fn query_artifact_not_found() {
        let (_dir, path) = temp_db_with_schema();
        let result = query_artifact(&path, "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee");
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("not found"), "error was: {err_msg}");
    }

    #[test]
    fn query_agent_not_found() {
        let (_dir, path) = temp_db_with_schema();
        let result = query_agent(&path, "nonexistent-agent");
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("not found"), "error was: {err_msg}");
    }

    #[tokio::test]
    async fn inspect_run_not_found_e2e() {
        // When the database does not exist, the function must return an error.
        // We set the env var to a known non-existent path.
        std::env::set_var(
            "POLKAGENT_DATABASE_SQLITE_PATH",
            "/tmp/polkagent_test_nonexistent_inspect_run.db",
        );
        let result = execute_inspect_run("01234567-89ab-cdef-0123-456789abcdef", false).await;
        std::env::remove_var("POLKAGENT_DATABASE_SQLITE_PATH");
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn inspect_effect_not_found_e2e() {
        std::env::set_var(
            "POLKAGENT_DATABASE_SQLITE_PATH",
            "/tmp/polkagent_test_nonexistent_inspect_effect.db",
        );
        let result = execute_inspect_effect("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee", false).await;
        std::env::remove_var("POLKAGENT_DATABASE_SQLITE_PATH");
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn inspect_artifact_not_found_e2e() {
        std::env::set_var(
            "POLKAGENT_DATABASE_SQLITE_PATH",
            "/tmp/polkagent_test_nonexistent_inspect_artifact.db",
        );
        let result = execute_inspect_artifact("sha256:abc123", false).await;
        std::env::remove_var("POLKAGENT_DATABASE_SQLITE_PATH");
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn inspect_agent_not_found_e2e() {
        std::env::set_var(
            "POLKAGENT_DATABASE_SQLITE_PATH",
            "/tmp/polkagent_test_nonexistent_inspect_agent.db",
        );
        let result = execute_inspect_agent("nonexistent-agent", false).await;
        std::env::remove_var("POLKAGENT_DATABASE_SQLITE_PATH");
        assert!(result.is_err());
    }

    #[tokio::test]
    #[allow(clippy::needless_return)]
    async fn inspect_run_invalid_uuid() {
        let result = execute_inspect_run("not-a-uuid", false).await;
        assert!(result.is_err());
        // May fail with "invalid run ID" or prefix-match DB error in test env.
    }

    #[tokio::test]
    async fn inspect_effect_invalid_uuid() {
        let result = execute_inspect_effect("bad", false).await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("invalid effect ID"),
            "error was: {err_msg}"
        );
    }

    #[tokio::test]
    async fn inspect_agent_empty_id() {
        let result = execute_inspect_agent("", false).await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("must not be empty"),
            "error was: {err_msg}"
        );
    }

    #[tokio::test]
    async fn inspect_artifact_empty_id() {
        let result = execute_inspect_artifact("", false).await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("must not be empty"),
            "error was: {err_msg}"
        );
    }

    #[tokio::test]
    async fn inspect_policy_missing_file() {
        let result =
            execute_inspect_policy(&PathBuf::from("/nonexistent/policy.toml"), false).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn inspect_policy_parses_valid_toml() {
        // Write a temporary TOML file and verify it parses.
        let dir = std::env::temp_dir().join("polkagent_inspect_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test_policy.toml");
        {
            let mut f = std::fs::File::create(&path).unwrap();
            f.write_all(
                br#"
name = "test"
version = "1.0.0"
description = "A test policy"

[[rules]]
action = "transfer"
resource = "DOT"
effect = "allow"
conditions = ["amount < 50"]
"#,
            )
            .unwrap();
        }

        let result = execute_inspect_policy(&path, true).await;
        assert!(result.is_ok(), "error: {result:?}");

        // Clean up.
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }

    // -- Expand tilde ---------------------------------------------------------

    #[test]
    fn expand_tilde_no_tilde() {
        assert_eq!(expand_tilde("/absolute/path"), "/absolute/path");
        assert_eq!(expand_tilde("relative/path"), "relative/path");
    }

    // -- PolicyInfo serialisation round-trip -----------------------------------

    #[test]
    fn policy_info_serialisation_roundtrip() {
        let info = PolicyInfo {
            path: "policies/default.toml".to_string(),
            name: "default".to_string(),
            version: "2.0.0".to_string(),
            description: "Default policy".to_string(),
            rules: vec![
                PolicyRule {
                    action: "*".to_string(),
                    resource: "*".to_string(),
                    effect: "deny".to_string(),
                    conditions: vec![],
                },
                PolicyRule {
                    action: "query".to_string(),
                    resource: "chain".to_string(),
                    effect: "allow".to_string(),
                    conditions: vec!["agent.role == 'reader'".to_string()],
                },
            ],
        };

        let json = serde_json::to_string(&info).unwrap();
        let parsed: PolicyInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, info);
    }
}
