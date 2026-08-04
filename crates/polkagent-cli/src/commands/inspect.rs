//! `polkagent inspect` — Inspect runs, effects, artifacts, agents, and policies.
//!
//! A read-only diagnostic command that drills into individual entities stored
//! in the local SQLite database or on disk (policy TOML files).  Each
//! subcommand fetches a single entity by ID/path, formats it for human
//! consumption, and optionally emits structured JSON (`--json`).

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Args, Subcommand};
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
    pub intent: String,
    pub state: String,
    pub attempts: u32,
    pub outcome: Option<String>,
    pub created_at: String,
    pub resolved_at: Option<String>,
}

/// Serialisable representation of an artifact inspection result.
#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct ArtifactInfo {
    pub id: String,
    pub kind: String,
    pub digest: String,
    pub size_bytes: u64,
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

/// Inspect a run by ID.
///
/// In a real implementation this would use `AppService` or `SqliteRunStore` to
/// fetch the run record, its turns, steps, and associated effects.  For now
/// the output structure is defined and a not-found stub is returned.
pub async fn execute_inspect_run(run_id: &str, json_output: bool) -> Result<()> {
    validate_uuid(run_id).context("invalid run ID")?;

    // Stub: in production this reads from the store.
    let info = RunInfo {
        id: run_id.to_string(),
        agent_id: String::new(),
        state: "not_found".to_string(),
        created_at: String::new(),
        updated_at: String::new(),
        turns: 0,
        steps: 0,
        elapsed_ms: None,
        effects: Vec::new(),
    };

    if info.state == "not_found" {
        anyhow::bail!("Run not found: {run_id}");
    }

    render_run(&info, json_output)
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
/// In production this would query the `effect_intents` and `effect_attempts`
/// tables.
pub async fn execute_inspect_effect(effect_id: &str, json_output: bool) -> Result<()> {
    validate_uuid(effect_id).context("invalid effect ID")?;

    let info = EffectInfo {
        id: effect_id.to_string(),
        run_id: String::new(),
        intent: String::new(),
        state: "not_found".to_string(),
        attempts: 0,
        outcome: None,
        created_at: String::new(),
        resolved_at: None,
    };

    if info.state == "not_found" {
        anyhow::bail!("Effect not found: {effect_id}");
    }

    render_effect(&info, json_output)
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
    println!("  Intent:      {}", info.intent);
    println!("  State:       {}", info.state);
    println!("  Attempts:    {}", info.attempts);

    if let Some(ref outcome) = info.outcome {
        println!("  Outcome:     {outcome}");
    }

    println!("  Created:     {}", info.created_at);

    if let Some(ref resolved) = info.resolved_at {
        println!("  Resolved:    {resolved}");
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Artifact inspection
// ---------------------------------------------------------------------------

/// Inspect an artifact by ID.
///
/// In production this would read from an artifact store (blob storage or
/// local file system) and return metadata without streaming the content.
pub async fn execute_inspect_artifact(artifact_id: &str, json_output: bool) -> Result<()> {
    if artifact_id.is_empty() {
        anyhow::bail!("artifact ID must not be empty");
    }

    let info = ArtifactInfo {
        id: artifact_id.to_string(),
        kind: "not_found".to_string(),
        digest: String::new(),
        size_bytes: 0,
        created_at: String::new(),
        lineage: ArtifactLineage {
            run_id: None,
            step_id: None,
            parent_artifact_id: None,
        },
    };

    if info.kind == "not_found" {
        anyhow::bail!("Artifact not found: {artifact_id}");
    }

    render_artifact(&info, json_output)
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

    Ok(())
}

// ---------------------------------------------------------------------------
// Agent inspection
// ---------------------------------------------------------------------------

/// Inspect an agent by name or UUID.
///
/// In production this would query the `agents` table and deserialise the
/// `spec_json` column to extract model, tools, policies, and autonomy level.
pub async fn execute_inspect_agent(agent_id: &str, json_output: bool) -> Result<()> {
    if agent_id.is_empty() {
        anyhow::bail!("agent ID must not be empty");
    }

    let info = AgentInfo {
        id: agent_id.to_string(),
        name: String::new(),
        model: String::new(),
        state: "not_found".to_string(),
        autonomy_level: String::new(),
        tools: Vec::new(),
        capabilities: Vec::new(),
        policy_refs: Vec::new(),
        created_at: String::new(),
        updated_at: String::new(),
    };

    if info.state == "not_found" {
        anyhow::bail!("Agent not found: {agent_id}");
    }

    render_agent(&info, json_output)
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
pub async fn execute_inspect_policy(path: &PathBuf, json_output: bool) -> Result<()> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("reading policy file: {}", path.display()))?;

    let info = parse_policy_toml(&content, path)?;

    render_policy(&info, json_output)
}

/// Parse raw TOML content into a `PolicyInfo`.
fn parse_policy_toml(content: &str, path: &PathBuf) -> Result<PolicyInfo> {
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
/// SQLite database file.  Does not require the daemon to be running.
pub async fn execute_inspect_db(json_output: bool) -> Result<()> {
    let db_path = resolve_db_path();
    let expanded = expand_tilde(&db_path);

    let stats = gather_db_stats(&expanded)?;

    render_db_stats(&stats, json_output)
}

/// Gather database statistics from the SQLite file at `path`.
fn gather_db_stats(path: &str) -> Result<DbStats> {
    let conn =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .with_context(|| format!("opening database at {path}"))?;

    // Database file size.
    let db_size_bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);

    // WAL file size.
    let wal_path = format!("{path}-wal");
    let wal_size_bytes = std::fs::metadata(&wal_path).map(|m| m.len()).unwrap_or(0);

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
        .filter_map(|r| r.ok())
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
            println!("    {:<30} {count}", name);
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
        format!("{:.2} GiB", bytes as f64 / GIB as f64)
    } else if bytes >= MIB {
        format!("{:.2} MiB", bytes as f64 / MIB as f64)
    } else if bytes >= KIB {
        format!("{:.2} KiB", bytes as f64 / KIB as f64)
    } else {
        format!("{bytes} B")
    }
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
            intent: "balance_transfer".to_string(),
            state: "approved".to_string(),
            attempts: 2,
            outcome: Some("success".to_string()),
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
            kind: "wasm-blob".to_string(),
            digest: "sha256:deadbeef0123456789abcdef".to_string(),
            size_bytes: 1_048_576,
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

    #[tokio::test]
    async fn inspect_run_not_found() {
        let result = execute_inspect_run("01234567-89ab-cdef-0123-456789abcdef", false).await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("not found"), "error was: {err_msg}");
    }

    #[tokio::test]
    async fn inspect_effect_not_found() {
        let result = execute_inspect_effect("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee", false).await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("not found"), "error was: {err_msg}");
    }

    #[tokio::test]
    async fn inspect_artifact_not_found() {
        let result = execute_inspect_artifact("sha256:abc123", false).await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("not found"), "error was: {err_msg}");
    }

    #[tokio::test]
    async fn inspect_agent_not_found() {
        let result = execute_inspect_agent("nonexistent-agent", false).await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("not found"), "error was: {err_msg}");
    }

    #[tokio::test]
    async fn inspect_run_invalid_uuid() {
        let result = execute_inspect_run("not-a-uuid", false).await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("invalid run ID"), "error was: {err_msg}");
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
