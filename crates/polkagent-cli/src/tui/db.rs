//! Lightweight SQLite query helpers for the TUI.
//!
//! The TUI reads from the same database as the daemon. All queries are
//! read-only and use rusqlite directly (no async) since they run on the
//! main thread between frames.
//!
//! The data returned is already in the TUI's `state::*` summary types to
//! avoid leaking raw row structs into the view layer.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OpenFlags};

use crate::tui::state::{AgentSummary, RunSummary, SystemHealth};

// ---------------------------------------------------------------------------
// Database handle
// ---------------------------------------------------------------------------

/// Read-only database handle for the TUI.
pub struct TuiDb {
    conn: Connection,
}

impl TuiDb {
    /// Open the database at `path` in read-only mode.
    pub fn open(path: &str) -> Result<Self> {
        let expanded = expand_tilde(path);
        let conn = Connection::open_with_flags(
            &expanded,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
        )
        .with_context(|| format!("opening database at {expanded}"))?;

        // Enable WAL reader and optimise for read-heavy workload.
        conn.pragma_update(None, "journal_mode", "WAL")
            .ok(); // Ignore — we may be read-only on a WAL file.
        conn.pragma_update(None, "temp_store", "MEMORY").ok();

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
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Expand a leading `~` in a path using the HOME environment variable.
fn expand_tilde(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return format!("{home}/{rest}");
        }
    }
    path.to_owned()
}

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
