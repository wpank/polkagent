//! Lightweight SQLite query helpers and chain polling for the TUI.
//!
//! The TUI reads from the same database as the daemon. All queries are
//! read-only and use rusqlite directly (no async) since they run on the
//! main thread between frames.
//!
//! The data returned is already in the TUI's `state::*` summary types to
//! avoid leaking raw row structs into the view layer.
//!
//! ## Chain polling
//!
//! [`ChainPoller`] reads `POLKAGENT_RPC_URL` from the environment and
//! periodically queries the JSON-RPC endpoint for chain status (best block,
//! finalized block, chain name, node version). When the variable is unset
//! or empty the poller is a no-op and the TUI gracefully shows
//! "Not connected".

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::Connection;

use polkagent_store_sqlite::SqlitePool;

use crate::tui::state::{
    AgentSummary, ApprovalItem, AuditEvent, EventSummary, MemoryEntry as TuiMemoryEntry,
    RunDetail, RunSummary, SystemHealth, TurnSummary,
};

// ---------------------------------------------------------------------------
// Database handle
// ---------------------------------------------------------------------------

/// Read-only database handle for the TUI.
pub struct TuiDb {
    conn: Connection,
}

impl TuiDb {
    /// Open a read-only connection from the pool.
    pub fn from_pool(pool: &SqlitePool) -> Result<Self> {
        let conn = pool
            .reader()
            .with_context(|| "opening read-only connection from pool")?;
        Ok(Self { conn })
    }

    // ── Agents ────────────────────────────────────────────────────────────

    /// Load all agent summaries ordered by name.
    pub fn agents(&self) -> Result<Vec<AgentSummary>> {
        let mut stmt = self.conn.prepare(
            "SELECT a.id, a.name, a.state, a.spec_json, a.updated_at,
                    COUNT(r.id) AS total_runs,
                    SUM(CASE WHEN r.state NOT IN ('completed','failed','cancelled','timed_out')
                              THEN 1 ELSE 0 END) AS active_runs
             FROM agents a
             LEFT JOIN runs r ON r.agent_id = a.id
             WHERE a.state != 'archived'
             GROUP BY a.id
             ORDER BY a.name ASC",
        )?;

        let summaries = stmt
            .query_map([], |row| {
                let spec_json: String = row.get(3)?;
                let updated_at_str: String = row.get(4)?;
                let total_runs: u32 = row.get::<_, i64>(5)? as u32;
                let active_runs: u32 = row.get::<_, i64>(6)? as u32;

                // Extract model from spec JSON — gracefully degrade if absent.
                let model = extract_json_string(&spec_json, "model")
                    .unwrap_or_else(|| String::from("unknown"));

                let updated_at = parse_datetime(&updated_at_str)
                    .unwrap_or_else(Utc::now);

                Ok(AgentSummary {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    state: row.get(2)?,
                    model,
                    total_runs,
                    active_runs,
                    updated_at,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();

        Ok(summaries)
    }

    // ── Runs ─────────────────────────────────────────────────────────────

    /// Load the most recent `limit` runs ordered newest-first.
    pub fn recent_runs(&self, limit: usize) -> Result<Vec<RunSummary>> {
        let mut stmt = self.conn.prepare(
            "SELECT r.id, r.agent_id, a.name, r.state, r.created_at, r.completed_at,
                    COUNT(DISTINCT t.id) AS turn_count,
                    COALESCE(SUM(t.input_tokens), 0) AS input_tokens,
                    COALESCE(SUM(t.output_tokens), 0) AS output_tokens
             FROM runs r
             JOIN agents a ON a.id = r.agent_id
             LEFT JOIN turns t ON t.run_id = r.id
             GROUP BY r.id
             ORDER BY r.created_at DESC
             LIMIT ?1",
        )?;

        let summaries = stmt
            .query_map([limit as i64], |row| {
                let id: String = row.get(0)?;
                let short_id = id[..8.min(id.len())].to_owned();
                let created_at_str: String = row.get(4)?;
                let completed_at_str: Option<String> = row.get(5)?;
                let turn_count: u32 = row.get::<_, i64>(6)? as u32;
                let input_tokens: u64 = row.get::<_, i64>(7)? as u64;
                let output_tokens: u64 = row.get::<_, i64>(8)? as u64;

                let created_at =
                    parse_datetime(&created_at_str).unwrap_or_else(Utc::now);
                let completed_at = completed_at_str.as_deref().and_then(parse_datetime);

                Ok(RunSummary {
                    id,
                    short_id,
                    agent_name: row.get(2)?,
                    state: row.get(3)?,
                    created_at,
                    completed_at,
                    turn_count,
                    input_tokens,
                    output_tokens,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();

        Ok(summaries)
    }

    // ── Run detail ─────────────────────────────────────────────────────

    /// Load full detail for a single run including turns and effect counts.
    pub fn run_detail(&self, run_id: &str) -> Result<Option<RunDetail>> {
        // Main run row with aggregated token/turn counts.
        let mut stmt = self.conn.prepare(
            "SELECT r.id, a.name, r.state, r.created_at, r.updated_at, r.completed_at,
                    COUNT(DISTINCT t.id) AS turn_count,
                    COALESCE(SUM(t.input_tokens), 0) AS input_tokens,
                    COALESCE(SUM(t.output_tokens), 0) AS output_tokens
             FROM runs r
             JOIN agents a ON a.id = r.agent_id
             LEFT JOIN turns t ON t.run_id = r.id
             WHERE r.id = ?1
             GROUP BY r.id",
        )?;

        let detail = stmt
            .query_row([run_id], |row| {
                let id: String = row.get(0)?;
                let short_id = id[..8.min(id.len())].to_owned();
                let created_at_str: String = row.get(3)?;
                let updated_at_str: String = row.get(4)?;
                let completed_at_str: Option<String> = row.get(5)?;
                let turn_count: u32 = row.get::<_, i64>(6)? as u32;
                let input_tokens: u64 = row.get::<_, i64>(7)? as u64;
                let output_tokens: u64 = row.get::<_, i64>(8)? as u64;

                let created_at = parse_datetime(&created_at_str).unwrap_or_else(Utc::now);
                let updated_at = parse_datetime(&updated_at_str).unwrap_or_else(Utc::now);
                let completed_at = completed_at_str.as_deref().and_then(parse_datetime);

                Ok(RunDetail {
                    id,
                    short_id,
                    agent_name: row.get(1)?,
                    state: row.get(2)?,
                    created_at,
                    updated_at,
                    completed_at,
                    turn_count,
                    input_tokens,
                    output_tokens,
                    effect_count: 0,
                    effects_succeeded: 0,
                    effects_failed: 0,
                    effects_pending: 0,
                    turns: Vec::new(),
                })
            })
            .optional()?;

        let Some(mut detail) = detail else {
            return Ok(None);
        };

        // Effect counts.
        let effect_counts: (u32, u32, u32, u32) = self
            .conn
            .query_row(
                "SELECT COUNT(*) AS total,
                        SUM(CASE WHEN eo.status = 'success' THEN 1 ELSE 0 END),
                        SUM(CASE WHEN eo.status IN ('failure','timeout') THEN 1 ELSE 0 END),
                        SUM(CASE WHEN eo.id IS NULL THEN 1 ELSE 0 END)
                 FROM effect_intents ei
                 LEFT JOIN effect_outcomes eo ON eo.intent_id = ei.id
                 WHERE ei.run_id = ?1",
                [run_id],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)? as u32,
                        row.get::<_, i64>(1)? as u32,
                        row.get::<_, i64>(2)? as u32,
                        row.get::<_, i64>(3)? as u32,
                    ))
                },
            )
            .unwrap_or((0, 0, 0, 0));

        detail.effect_count = effect_counts.0;
        detail.effects_succeeded = effect_counts.1;
        detail.effects_failed = effect_counts.2;
        detail.effects_pending = effect_counts.3;

        // Turn list.
        let mut turn_stmt = self.conn.prepare(
            "SELECT sequence, role, started_at, completed_at, input_tokens, output_tokens
             FROM turns
             WHERE run_id = ?1
             ORDER BY sequence ASC",
        )?;

        detail.turns = turn_stmt
            .query_map([run_id], |row| {
                let started_str: String = row.get(2)?;
                let completed_str: Option<String> = row.get(3)?;
                Ok(TurnSummary {
                    sequence: row.get::<_, i64>(0)? as u32,
                    role: row.get(1)?,
                    started_at: parse_datetime(&started_str).unwrap_or_else(Utc::now),
                    completed_at: completed_str.as_deref().and_then(parse_datetime),
                    input_tokens: row.get::<_, i64>(4)? as u64,
                    output_tokens: row.get::<_, i64>(5)? as u64,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();

        Ok(Some(detail))
    }

    // ── Run events ──────────────────────────────────────────────────────

    /// Load events for a specific run, ordered by sequence.
    pub fn run_events(&self, run_id: &str, limit: usize) -> Result<Vec<EventSummary>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, kind, timestamp, data_json
             FROM run_events
             WHERE run_id = ?1
             ORDER BY sequence ASC
             LIMIT ?2",
        )?;

        let events = stmt
            .query_map(rusqlite::params![run_id, limit as i64], |row| {
                let ts_str: String = row.get(2)?;
                let payload: String = row.get(3)?;
                let event_type: String = row.get(1)?;
                let description = event_description(&event_type, &payload);
                Ok(EventSummary {
                    id: row.get(0)?,
                    timestamp: parse_datetime(&ts_str).unwrap_or_else(Utc::now),
                    event_type,
                    description,
                    payload,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();

        Ok(events)
    }

    // ── Pending effects (approval queue) ─────────────────────────────────

    /// Load pending effect intents that have no outcome yet.
    pub fn pending_effects(&self, limit: usize) -> Result<Vec<ApprovalItem>> {
        let mut stmt = self.conn.prepare(
            "SELECT ei.id, ei.kind, ei.run_id, a.name, ei.created_at, ei.state
             FROM effect_intents ei
             JOIN runs r ON r.id = ei.run_id
             JOIN agents a ON a.id = r.agent_id
             LEFT JOIN effect_outcomes eo ON eo.intent_id = ei.id
             WHERE eo.id IS NULL
             ORDER BY ei.created_at DESC
             LIMIT ?1",
        )?;

        let items = stmt
            .query_map([limit as i64], |row| {
                let created_str: String = row.get(4)?;
                Ok(ApprovalItem {
                    effect_id: row.get(0)?,
                    kind: row.get(1)?,
                    run_id: row.get(2)?,
                    agent_name: row.get(3)?,
                    created_at: parse_datetime(&created_str).unwrap_or_else(Utc::now),
                    state: row.get(5)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();

        Ok(items)
    }

    // ── System health ────────────────────────────────────────────────────

    /// Read aggregate counts for the system health panel.
    pub fn system_health(&self, db_path: &str) -> Result<SystemHealth> {
        let (agent_count, active_agent_count): (u32, u32) = self
            .conn
            .query_row(
                "SELECT COUNT(*) FILTER (WHERE state != 'archived'),
                        COUNT(*) FILTER (WHERE state = 'active')
                 FROM agents",
                [],
                |row| Ok((row.get::<_, i64>(0)? as u32, row.get::<_, i64>(1)? as u32)),
            )
            .unwrap_or((0, 0));

        let (total_run_count, active_run_count): (u32, u32) = self
            .conn
            .query_row(
                "SELECT COUNT(*),
                        COUNT(*) FILTER (WHERE state NOT IN ('completed','failed','cancelled','timed_out'))
                 FROM runs",
                [],
                |row| Ok((row.get::<_, i64>(0)? as u32, row.get::<_, i64>(1)? as u32)),
            )
            .unwrap_or((0, 0));

        Ok(SystemHealth {
            db_ok: true,
            db_path: db_path.to_owned(),
            agent_count,
            active_agent_count,
            total_run_count,
            active_run_count,
            sampled_at: Utc::now(),
        })
    }

    // ── Memory browser ───────────────────────────────────────────────────

    /// Load memory entries from the memory SQLite database.
    ///
    /// When `query` is `None` all recent entries are returned ordered by
    /// creation time descending. When a query is given, FTS/LIKE search is
    /// performed.
    pub fn memory_entries(
        &self,
        query: Option<&str>,
        limit: usize,
    ) -> Result<Vec<TuiMemoryEntry>> {
        // The memory store lives in a separate database file; open it
        // read-only here.
        let home = std::env::var("HOME").unwrap_or_default();
        let default_path = format!("{home}/.local/share/polkagent/memory.db");
        let mem_path = std::env::var("POLKAGENT_MEMORY_DB_PATH").unwrap_or(default_path);

        let mem_conn = Connection::open_with_flags(
            &mem_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        );

        let conn = match mem_conn {
            Ok(c) => c,
            // If the memory DB doesn't exist yet return an empty list.
            Err(_) => return Ok(vec![]),
        };

        let (sql, has_query) = if query.is_some() {
            (
                "SELECT id, memory_type, agent_id, content, relevance_score, created_at
                 FROM memories
                 WHERE content LIKE '%' || ?1 || '%'
                 ORDER BY relevance_score DESC
                 LIMIT ?2"
                    .to_owned(),
                true,
            )
        } else {
            (
                "SELECT id, memory_type, agent_id, content, relevance_score, created_at
                 FROM memories
                 ORDER BY created_at DESC
                 LIMIT ?1"
                    .to_owned(),
                false,
            )
        };

        let mut stmt = conn.prepare(&sql)?;
        let entries = if has_query {
            stmt.query_map(
                rusqlite::params![query.unwrap_or_default(), limit as i64],
                |row| {
                    let created_str: String = row.get(5)?;
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, f64>(4)?,
                        created_str,
                    ))
                },
            )?
            .collect::<Result<Vec<_>, _>>()?
        } else {
            stmt.query_map(rusqlite::params![limit as i64], |row| {
                let created_str: String = row.get(5)?;
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, f64>(4)?,
                    created_str,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?
        };

        let parsed = entries
            .into_iter()
            .filter_map(|(id, memory_type, agent_name, content, relevance_score, created_str)| {
                let created_at = DateTime::parse_from_rfc3339(&created_str)
                    .map(|dt| dt.with_timezone(&Utc))
                    .ok()?;
                Some(TuiMemoryEntry {
                    id,
                    memory_type,
                    agent_name,
                    content,
                    relevance_score,
                    created_at,
                })
            })
            .collect();

        Ok(parsed)
    }

    /// Delete a memory entry by its string ID.
    pub fn delete_memory_entry(&self, entry_id: &str) -> Result<()> {
        let home = std::env::var("HOME").unwrap_or_default();
        let default_path = format!("{home}/.local/share/polkagent/memory.db");
        let mem_path = std::env::var("POLKAGENT_MEMORY_DB_PATH").unwrap_or(default_path);

        let conn = Connection::open(&mem_path)
            .with_context(|| format!("opening memory database at {mem_path}"))?;

        conn.execute("DELETE FROM memories WHERE id = ?1", rusqlite::params![entry_id])?;
        Ok(())
    }

    // ── Audit log ────────────────────────────────────────────────────────

    /// Load recent audit events from the run_events table.
    pub fn audit_events(&self, limit: usize) -> Result<Vec<AuditEvent>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT id, run_id, kind, data_json, timestamp
             FROM run_events
             ORDER BY sequence DESC
             LIMIT {limit}",
            limit = limit,
        ))?;

        let raw: Vec<(String, String, String, String, String)> = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        let events = raw
            .into_iter()
            .filter_map(|(id, run_id, kind, payload, ts_str)| {
                let timestamp = DateTime::parse_from_rfc3339(&ts_str)
                    .map(|dt| dt.with_timezone(&Utc))
                    .ok()?;
                let short_run = run_id[..run_id.len().min(8)].to_owned();
                let kind_lower = kind.to_lowercase();
                let severity = if kind_lower.contains("error") || kind_lower.contains("fail") {
                    "error"
                } else if kind_lower.contains("warn") {
                    "warn"
                } else {
                    "info"
                };
                Some(AuditEvent {
                    id,
                    kind,
                    run_id: Some(run_id),
                    agent_name: format!("run:{short_run}"),
                    message: payload,
                    severity: severity.to_owned(),
                    timestamp,
                })
            })
            .collect();

        Ok(events)
    }

    // ── Token usage history ────────────────────────────────────────────────

    /// Return per-turn total token counts (input + output) for a run, ordered
    /// by sequence. Used by the TUI to draw usage sparklines.
    #[allow(dead_code)]
    pub fn token_usage_history(&self, run_id: &str) -> Result<Vec<u64>> {
        let mut stmt = self.conn.prepare(
            "SELECT input_tokens + output_tokens AS total
             FROM turns
             WHERE run_id = ?1
             ORDER BY sequence ASC",
        )?;

        let totals = stmt
            .query_map([run_id], |row| row.get::<_, i64>(0).map(|v| v as u64))?
            .filter_map(|r| r.ok())
            .collect();

        Ok(totals)
    }

    // ── Recent error count ───────────────────────────────────────────────

    /// Count run events whose `kind` contains "error" (case-insensitive).
    /// Returns 0 on any DB error so it is safe to call in non-critical UI paths.
    #[allow(dead_code)]
    pub fn recent_error_count(&self) -> u32 {
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM run_events WHERE LOWER(kind) LIKE '%error%'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap_or(0) as u32
    }

    // ── Budget status ────────────────────────────────────────────────────

    /// Rough spend estimate: sums all token usage across turns and applies a
    /// flat per-token rate. Returns `(spent_dollars, ceiling_dollars)`.
    ///
    /// The ceiling is a hardcoded default; there is no per-agent budget table
    /// yet, so we simply return $10 as the default.
    #[allow(dead_code)]
    pub fn budget_status(&self) -> (f64, f64) {
        const DOLLARS_PER_TOKEN: f64 = 0.000_003;
        const DEFAULT_CEILING: f64 = 10.0;

        let total_tokens: i64 = self
            .conn
            .query_row(
                "SELECT COALESCE(SUM(input_tokens + output_tokens), 0) FROM turns",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);

        let spent = total_tokens as f64 * DOLLARS_PER_TOKEN;
        (spent, DEFAULT_CEILING)
    }

    // ── Approvals (write) ─────────────────────────────────────────────────

    /// Mark an effect intent as approved.
    pub fn approve_effect(&self, effect_id: &str) -> Result<()> {
        // We need a writer connection, but TuiDb only holds a reader.
        // Open a separate writable connection to the same DB file.
        let db_path = self.conn.path().unwrap_or_default().to_owned();
        let writer = Connection::open(&db_path)
            .with_context(|| "opening writable connection for approve")?;

        let outcome_id = uuid::Uuid::now_v7().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        writer.execute(
            "INSERT INTO effect_outcomes (id, intent_id, status, result_json, created_at)
             VALUES (?1, ?2, 'approved', '{\"source\":\"tui\"}', ?3)",
            rusqlite::params![outcome_id, effect_id, now],
        )?;
        Ok(())
    }

    /// Mark an effect intent as denied.
    pub fn deny_effect(&self, effect_id: &str) -> Result<()> {
        let db_path = self.conn.path().unwrap_or_default().to_owned();
        let writer = Connection::open(&db_path)
            .with_context(|| "opening writable connection for deny")?;

        let outcome_id = uuid::Uuid::now_v7().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        writer.execute(
            "INSERT INTO effect_outcomes (id, intent_id, status, result_json, created_at)
             VALUES (?1, ?2, 'denied', '{\"source\":\"tui\"}', ?3)",
            rusqlite::params![outcome_id, effect_id, now],
        )?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Parse an ISO-8601 / RFC-3339 datetime string.
fn parse_datetime(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

/// Pull a string value from a JSON object string without fully deserializing.
fn extract_json_string(json: &str, key: &str) -> Option<String> {
    // Simple pattern: `"key":"value"` or `"key": "value"`.
    let needle = format!("\"{key}\"");
    let start = json.find(&needle)? + needle.len();
    let rest = json[start..].trim_start();
    let rest = rest.strip_prefix(':')?.trim_start();
    let rest = rest.strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(rest[..end].to_owned())
}

/// Build a short human-readable description for an event from its type and payload.
fn event_description(event_type: &str, payload: &str) -> String {
    // Try to extract a meaningful field from the payload.
    let detail = extract_json_string(payload, "kind")
        .or_else(|| extract_json_string(payload, "status"))
        .or_else(|| extract_json_string(payload, "role"))
        .unwrap_or_default();

    if detail.is_empty() {
        humanize_event_type(event_type)
    } else {
        format!("{}: {detail}", humanize_event_type(event_type))
    }
}

/// Convert a PascalCase or snake_case event type into a readable label.
fn humanize_event_type(event_type: &str) -> String {
    // Simple approach: insert spaces before uppercase letters in PascalCase.
    let mut out = String::with_capacity(event_type.len() + 4);
    for (i, ch) in event_type.chars().enumerate() {
        if ch.is_uppercase() && i > 0 {
            out.push(' ');
        }
        if i == 0 {
            out.extend(ch.to_uppercase());
        } else {
            out.push(ch);
        }
    }
    out
}

/// Extension trait to provide `.optional()` on rusqlite results.
trait OptionalExt<T> {
    fn optional(self) -> Result<Option<T>>;
}

impl<T> OptionalExt<T> for rusqlite::Result<T> {
    fn optional(self) -> Result<Option<T>> {
        match self {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
}

// ---------------------------------------------------------------------------
// Chain poller
// ---------------------------------------------------------------------------

/// Snapshot of chain status fetched from a Substrate JSON-RPC endpoint.
#[derive(Debug, Clone)]
pub struct ChainStatus {
    /// Human-readable chain name (from `system_chain`).
    pub chain_name: String,
    /// Node implementation version (from `system_version`).
    pub node_version: String,
    /// Best (head) block number.
    pub best_block: u64,
    /// Last finalized block number.
    pub finalized_block: u64,
}

/// Polls a Substrate JSON-RPC endpoint for chain status on a fixed interval.
///
/// Reads `POLKAGENT_RPC_URL` from the environment at construction time. If the
/// variable is absent or empty, all poll calls are no-ops and the chain status
/// fields on [`crate::tui::state::TuiState`] remain at their defaults
/// ("Not connected", block 0).
pub struct ChainPoller {
    /// The RPC URL to query, or `None` if not configured.
    rpc_url: Option<String>,
    /// Timestamp of the last successful poll (monotonic).
    last_poll: Option<std::time::Instant>,
    /// How often to re-poll (default 6 seconds).
    interval: std::time::Duration,
}

impl ChainPoller {
    /// Polling interval — one poll per finality period (6 seconds).
    const DEFAULT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(6);

    /// Create a new poller, reading `POLKAGENT_RPC_URL` from the environment.
    pub fn new() -> Self {
        let rpc_url = std::env::var("POLKAGENT_RPC_URL")
            .ok()
            .filter(|s| !s.is_empty());

        Self {
            rpc_url,
            last_poll: None,
            interval: Self::DEFAULT_INTERVAL,
        }
    }

    /// Create a poller with a specific URL (useful for testing).
    #[cfg(test)]
    pub fn with_url(url: Option<String>) -> Self {
        Self {
            rpc_url: url,
            last_poll: None,
            interval: Self::DEFAULT_INTERVAL,
        }
    }

    /// Returns `true` if enough time has elapsed since the last poll.
    pub fn should_poll(&self) -> bool {
        if self.rpc_url.is_none() {
            return false;
        }
        match self.last_poll {
            None => true,
            Some(t) => t.elapsed() >= self.interval,
        }
    }

    /// Poll the RPC endpoint and update `state` with fresh chain data.
    ///
    /// If no RPC URL is configured, this is a no-op. If the RPC call fails
    /// the state is marked as disconnected but no error is propagated.
    pub fn poll(&mut self, state: &mut crate::tui::state::TuiState) {
        let Some(url) = &self.rpc_url else {
            state.chain_connected = false;
            state.chain_name = String::from("Not connected");
            state.node_version = String::new();
            return;
        };

        match self.fetch_chain_status(url) {
            Ok(status) => {
                state.chain_connected = true;
                state.chain_name = status.chain_name;
                state.node_version = status.node_version;
                state.best_block = status.best_block;
                state.finalized_block = status.finalized_block;
                state.mark_dirty();
            }
            Err(_) => {
                state.chain_connected = false;
                state.mark_dirty();
            }
        }

        self.last_poll = Some(std::time::Instant::now());
    }

    /// Execute the JSON-RPC calls to fetch chain status.
    ///
    /// Makes three sequential calls:
    /// - `system_chain` — chain name
    /// - `system_version` — node version
    /// - `chain_getHeader` — best block header (block number)
    /// - `chain_getFinalizedHead` + `chain_getHeader` — finalized block number
    fn fetch_chain_status(&self, url: &str) -> Result<ChainStatus> {
        let chain_name = self.rpc_call_string(url, "system_chain", "[]")?;
        let node_version = self.rpc_call_string(url, "system_version", "[]")?;

        // Best block: chain_getHeader returns the latest header.
        let best_header_json = self.rpc_call_raw(url, "chain_getHeader", "[]")?;
        let best_block = parse_block_number_from_header(&best_header_json);

        // Finalized block: get hash, then header.
        let finalized_hash = self.rpc_call_string(url, "chain_getFinalizedHead", "[]")?;
        let fin_header_json = self.rpc_call_raw(
            url,
            "chain_getHeader",
            &format!("[\"{finalized_hash}\"]"),
        )?;
        let finalized_block = parse_block_number_from_header(&fin_header_json);

        Ok(ChainStatus {
            chain_name,
            node_version,
            best_block,
            finalized_block,
        })
    }

    /// Make a JSON-RPC call and return the `result` field as a string.
    fn rpc_call_string(&self, url: &str, method: &str, params: &str) -> Result<String> {
        let body = format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"{method}","params":{params}}}"#,
        );
        let resp_body = ureq::post(url)
            .set("Content-Type", "application/json")
            .send_string(&body)
            .map_err(|e| anyhow::anyhow!("RPC request to {method} failed: {e}"))?
            .into_string()
            .map_err(|e| anyhow::anyhow!("reading RPC response body: {e}"))?;

        // Extract "result" value — for string results it is a quoted JSON string.
        extract_json_string(&resp_body, "result")
            .ok_or_else(|| anyhow::anyhow!("missing 'result' in RPC response for {method}"))
    }

    /// Make a JSON-RPC call and return the raw `result` value as a JSON string.
    fn rpc_call_raw(&self, url: &str, method: &str, params: &str) -> Result<String> {
        let body = format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"{method}","params":{params}}}"#,
        );
        let resp_body = ureq::post(url)
            .set("Content-Type", "application/json")
            .send_string(&body)
            .map_err(|e| anyhow::anyhow!("RPC request to {method} failed: {e}"))?
            .into_string()
            .map_err(|e| anyhow::anyhow!("reading RPC response body: {e}"))?;

        Ok(resp_body)
    }
}

impl Default for ChainPoller {
    fn default() -> Self {
        Self::new()
    }
}

/// Parse a block number from a Substrate block header JSON-RPC response.
///
/// The header contains `"number": "0x..."` as a hex-encoded string inside
/// the `"result"` object. We extract it with simple string matching.
fn parse_block_number_from_header(header_json: &str) -> u64 {
    // Look for "number":"0x..." pattern.
    extract_json_string(header_json, "number")
        .and_then(|hex| {
            let stripped = hex.strip_prefix("0x").unwrap_or(&hex);
            u64::from_str_radix(stripped, 16).ok()
        })
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    // ── Helpers ───────────────────────────────────────────────────────────────

    /// Create an in-memory SQLite database with the minimal schema needed for
    /// the TUI query helpers.
    fn in_memory_db() -> Connection {
        let conn = Connection::open_in_memory().expect("open in-memory db");
        conn.execute_batch(
            "
            CREATE TABLE agents (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                state TEXT NOT NULL,
                spec_json TEXT NOT NULL DEFAULT '{}',
                updated_at TEXT NOT NULL
            );
            CREATE TABLE runs (
                id TEXT PRIMARY KEY,
                agent_id TEXT NOT NULL,
                state TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                completed_at TEXT
            );
            CREATE TABLE turns (
                id TEXT PRIMARY KEY,
                run_id TEXT NOT NULL,
                sequence INTEGER NOT NULL,
                role TEXT NOT NULL,
                started_at TEXT NOT NULL,
                completed_at TEXT,
                input_tokens INTEGER NOT NULL DEFAULT 0,
                output_tokens INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE effect_intents (
                id TEXT PRIMARY KEY,
                run_id TEXT NOT NULL,
                kind TEXT NOT NULL,
                state TEXT NOT NULL DEFAULT 'pending',
                created_at TEXT NOT NULL,
                claimed_by TEXT,
                claimed_until TEXT
            );
            CREATE TABLE effect_outcomes (
                id TEXT PRIMARY KEY,
                intent_id TEXT NOT NULL,
                status TEXT NOT NULL,
                result_json TEXT NOT NULL DEFAULT '{}',
                created_at TEXT NOT NULL
            );
            CREATE TABLE run_events (
                id TEXT PRIMARY KEY,
                run_id TEXT NOT NULL,
                kind TEXT NOT NULL,
                data_json TEXT NOT NULL DEFAULT '{}',
                timestamp TEXT NOT NULL,
                sequence INTEGER NOT NULL DEFAULT 0
            );
            ",
        )
        .expect("create schema");
        conn
    }

    fn wrap(conn: Connection) -> TuiDb {
        TuiDb { conn }
    }

    // ── token_usage_history ───────────────────────────────────────────────────

    #[test]
    fn test_token_usage_history_empty_run() {
        let db = wrap(in_memory_db());
        let result = db.token_usage_history("nonexistent-run").unwrap();
        assert!(result.is_empty(), "empty db should return empty history");
    }

    #[test]
    fn test_token_usage_history_sums_turn_tokens() {
        let db = wrap(in_memory_db());
        let conn = &db.conn;

        // Insert a run.
        conn.execute(
            "INSERT INTO runs (id, agent_id, state, created_at, updated_at) VALUES ('r1', 'a1', 'working', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z')",
            [],
        ).unwrap();

        // Insert three turns with known token counts.
        for (seq, inp, out) in [(1i64, 100i64, 50i64), (2, 200, 80), (3, 300, 120)] {
            conn.execute(
                "INSERT INTO turns (id, run_id, sequence, role, started_at, input_tokens, output_tokens) VALUES (?, 'r1', ?, 'assistant', '2024-01-01T00:00:00Z', ?, ?)",
                rusqlite::params![format!("t{seq}"), seq, inp, out],
            ).unwrap();
        }

        let history = db.token_usage_history("r1").unwrap();
        assert_eq!(history, vec![150u64, 280, 420]);
    }

    #[test]
    fn test_token_usage_history_ordered_by_sequence() {
        let db = wrap(in_memory_db());
        let conn = &db.conn;

        conn.execute(
            "INSERT INTO runs (id, agent_id, state, created_at, updated_at) VALUES ('r2', 'a1', 'working', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z')",
            [],
        ).unwrap();

        // Insert in reverse sequence order.
        for (seq, inp, out) in [(3i64, 300i64, 0i64), (1, 100, 0), (2, 200, 0)] {
            conn.execute(
                "INSERT INTO turns (id, run_id, sequence, role, started_at, input_tokens, output_tokens) VALUES (?, 'r2', ?, 'user', '2024-01-01T00:00:00Z', ?, ?)",
                rusqlite::params![format!("t{seq}"), seq, inp, out],
            ).unwrap();
        }

        let history = db.token_usage_history("r2").unwrap();
        // Should come back in sequence order: 1, 2, 3.
        assert_eq!(history, vec![100u64, 200, 300]);
    }

    // ── recent_error_count ────────────────────────────────────────────────────

    #[test]
    fn test_recent_error_count_empty_db() {
        let db = wrap(in_memory_db());
        let count = db.recent_error_count();
        assert_eq!(count, 0, "empty db should report 0 errors");
    }

    #[test]
    fn test_recent_error_count_with_error_events() {
        let db = wrap(in_memory_db());
        let conn = &db.conn;

        conn.execute(
            "INSERT INTO runs (id, agent_id, state, created_at, updated_at) VALUES ('r3', 'a1', 'failed', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z')",
            [],
        ).unwrap();

        // Insert events — some are errors, some are not.
        let now_iso = "2099-12-31T00:00:00Z"; // far future so within "last hour" check fails
        // We use the current time so the window check passes.
        let now = chrono::Utc::now().to_rfc3339();
        for (id, event_type) in [("e1", "TurnError"), ("e2", "RunComplete"), ("e3", "ToolError")] {
            conn.execute(
                "INSERT INTO run_events (id, run_id, kind, data_json, timestamp) VALUES (?, 'r3', ?, '{}', ?)",
                rusqlite::params![id, event_type, now],
            ).unwrap();
        }
        let _ = now_iso; // suppress unused warning

        let count = db.recent_error_count();
        // "TurnError" and "ToolError" both contain "error" (case-insensitive).
        assert_eq!(count, 2);
    }

    // ── budget_status ─────────────────────────────────────────────────────────

    #[test]
    fn test_budget_status_empty_db() {
        let db = wrap(in_memory_db());
        let (spent, ceiling) = db.budget_status();
        assert_eq!(spent, 0.0, "no tokens = no spend");
        assert_eq!(ceiling, 10.0, "default ceiling is $10");
    }

    #[test]
    fn test_budget_status_with_tokens_charges_at_rate() {
        let db = wrap(in_memory_db());
        let conn = &db.conn;

        conn.execute(
            "INSERT INTO agents (id, name, state, updated_at) VALUES ('a1', 'agt', 'active', '2024-01-01T00:00:00Z')",
            [],
        ).unwrap();
        // Use a recent created_at so it falls within "last day".
        let recent = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO runs (id, agent_id, state, created_at, updated_at) VALUES ('r4', 'a1', 'completed', ?, ?)",
            rusqlite::params![recent, recent],
        ).unwrap();
        conn.execute(
            "INSERT INTO turns (id, run_id, sequence, role, started_at, input_tokens, output_tokens) VALUES ('t1', 'r4', 1, 'assistant', ?, 1000000, 0)",
            rusqlite::params![recent],
        ).unwrap();

        let (spent, ceiling) = db.budget_status();
        // 1_000_000 tokens * 0.000_003 = 3.0
        assert!((spent - 3.0).abs() < 0.001, "spent should be ~$3.00, got {spent}");
        assert_eq!(ceiling, 10.0);
    }

    // ── humanize_event_type ───────────────────────────────────────────────────

    #[test]
    fn test_humanize_event_type_pascal_case() {
        assert_eq!(humanize_event_type("TurnCompleted"), "Turn Completed");
        assert_eq!(humanize_event_type("RunStarted"), "Run Started");
        assert_eq!(humanize_event_type("ToolError"), "Tool Error");
    }

    #[test]
    fn test_humanize_event_type_single_word() {
        assert_eq!(humanize_event_type("run"), "Run");
        assert_eq!(humanize_event_type("error"), "Error");
    }

    // ── extract_json_string ───────────────────────────────────────────────────

    #[test]
    fn test_extract_json_string_simple() {
        let json = r#"{"model":"claude-sonnet","role":"assistant"}"#;
        assert_eq!(
            extract_json_string(json, "model"),
            Some("claude-sonnet".to_owned())
        );
        assert_eq!(
            extract_json_string(json, "role"),
            Some("assistant".to_owned())
        );
    }

    #[test]
    fn test_extract_json_string_missing_key() {
        let json = r#"{"model":"claude-sonnet"}"#;
        assert_eq!(extract_json_string(json, "missing"), None);
    }

    #[test]
    fn test_extract_json_string_with_spaces() {
        let json = r#"{"kind": "sign"}"#;
        assert_eq!(
            extract_json_string(json, "kind"),
            Some("sign".to_owned())
        );
    }

    // ── parse_block_number_from_header ────────────────────────────────────────

    #[test]
    fn test_parse_block_number_from_header_hex() {
        let header = r#"{"jsonrpc":"2.0","result":{"number":"0x157A3C0"},"id":1}"#;
        let block = parse_block_number_from_header(header);
        assert_eq!(block, 0x157A3C0);
    }

    #[test]
    fn test_parse_block_number_from_header_zero() {
        let header = r#"{"jsonrpc":"2.0","result":{"number":"0x0"},"id":1}"#;
        let block = parse_block_number_from_header(header);
        assert_eq!(block, 0);
    }

    #[test]
    fn test_parse_block_number_from_header_missing_returns_zero() {
        let header = r#"{"jsonrpc":"2.0","result":{},"id":1}"#;
        let block = parse_block_number_from_header(header);
        assert_eq!(block, 0, "missing number field should return 0");
    }

    #[test]
    fn test_parse_block_number_from_header_garbage_returns_zero() {
        let block = parse_block_number_from_header("not json at all");
        assert_eq!(block, 0, "garbage input should return 0");
    }

    // ── ChainPoller ──────────────────────────────────────────────────────────

    #[test]
    fn test_chain_poller_no_url_should_not_poll() {
        let poller = ChainPoller::with_url(None);
        assert!(!poller.should_poll());
    }

    #[test]
    fn test_chain_poller_with_url_should_poll_immediately() {
        let poller = ChainPoller::with_url(Some("wss://rpc.polkadot.io".to_owned()));
        assert!(poller.should_poll(), "should poll immediately on first call");
    }

    #[test]
    fn test_chain_poller_poll_no_url_sets_not_connected() {
        let mut poller = ChainPoller::with_url(None);
        let mut state = crate::tui::state::TuiState::default();

        // Pre-set some values to verify they get overwritten.
        state.chain_connected = true;
        state.chain_name = "Polkadot".to_owned();

        poller.poll(&mut state);

        assert!(!state.chain_connected, "should be disconnected when no URL");
        assert_eq!(state.chain_name, "Not connected");
    }

    #[test]
    fn test_chain_poller_poll_bad_url_marks_disconnected() {
        let mut poller = ChainPoller::with_url(
            Some("http://127.0.0.1:1".to_owned()), // unreachable port
        );
        let mut state = crate::tui::state::TuiState::default();
        state.chain_connected = true; // pre-set to true

        poller.poll(&mut state);

        // Unreachable endpoint should mark disconnected.
        assert!(!state.chain_connected, "unreachable endpoint should disconnect");
    }
}
