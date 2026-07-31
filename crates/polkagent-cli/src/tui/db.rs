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
use rusqlite::Connection;

use polkagent_store_sqlite::SqlitePool;

use crate::tui::state::{
    AgentSummary, ApprovalItem, EventSummary, RunDetail, RunSummary, SystemHealth, TurnSummary,
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
    ///
    /// Prefers the `durable_events` table (V2 event store) and falls back
    /// to the legacy `run_events` table when the V2 table is absent.
    pub fn run_events(&self, run_id: &str, limit: usize) -> Result<Vec<EventSummary>> {
        // Try durable_events first (V2 schema).
        let has_durable: bool = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='durable_events'",
                [],
                |row| row.get::<_, i64>(0).map(|n| n > 0),
            )
            .unwrap_or(false);

        if has_durable {
            let mut stmt = self.conn.prepare(
                "SELECT id, event_type, timestamp, payload
                 FROM durable_events
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

            return Ok(events);
        }

        // Fallback: legacy run_events table.
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
