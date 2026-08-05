//! `polkagent logs` — tail the event log.
//!
//! Reads events from the `run_events` table.  When `--follow` is set the
//! command polls the table every second and prints new events as they arrive.
//! The daemon does not need to be running; events are read directly from SQLite.

use anyhow::Result;

use polkagent_store_sqlite::SqlitePool;

use crate::cli::LogsCmd;

/// Execute the `logs` subcommand.
pub fn run(cmd: &LogsCmd, pool: &SqlitePool) -> Result<()> {
    if cmd.follow {
        follow(cmd, pool)
    } else {
        tail(cmd, pool)
    }
}

// ---------------------------------------------------------------------------
// tail (single-shot)
// ---------------------------------------------------------------------------

fn tail(cmd: &LogsCmd, pool: &SqlitePool) -> Result<()> {
    let reader = pool
        .reader()
        .map_err(|e| anyhow::anyhow!("opening database reader: {e}"))?;

    let rows = fetch_events(&reader, cmd, None)?;

    if rows.is_empty() {
        println!("No events found.");
    } else {
        for (rowid, kind, run_id, data, timestamp) in &rows {
            print_event(*rowid, kind, run_id, data, timestamp);
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// follow (polling loop)
// ---------------------------------------------------------------------------

fn follow(cmd: &LogsCmd, pool: &SqlitePool) -> Result<()> {
    let reader = pool
        .reader()
        .map_err(|e| anyhow::anyhow!("opening database reader: {e}"))?;

    // First: show the last `lines` events.
    let initial = fetch_events(&reader, cmd, None)?;
    // Cursor is the highest rowid seen so far; new poll uses WHERE rowid > last_rowid.
    let mut last_rowid: i64 = 0;

    for (rowid, kind, run_id, data, timestamp) in &initial {
        print_event(*rowid, kind, run_id, data, timestamp);
        last_rowid = last_rowid.max(*rowid);
    }

    // Then poll for new events.
    loop {
        std::thread::sleep(std::time::Duration::from_secs(1));

        let new_events = fetch_events(&reader, cmd, Some(last_rowid))?;

        for (rowid, kind, run_id, data, timestamp) in &new_events {
            print_event(*rowid, kind, run_id, data, timestamp);
            last_rowid = last_rowid.max(*rowid);
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Row type: (rowid, kind, run_id, data_json, timestamp).
type EventRow = (i64, String, String, String, String);

fn fetch_events(
    reader: &rusqlite::Connection,
    cmd: &LogsCmd,
    after_rowid: Option<i64>,
) -> Result<Vec<EventRow>> {
    // Build the SQL query dynamically to avoid the stmt lifetime issue.
    let (sql, params_vec) = build_query(cmd, after_rowid);

    let mut stmt = reader.prepare(&sql)?;

    #[allow(clippy::cast_possible_wrap)]
    let mut rows: Vec<EventRow> = stmt
        .query_map(rusqlite::params_from_iter(params_vec.iter()), |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    // For the "tail" queries (no after_rowid) we fetched in DESC order then
    // reverse to display chronologically.
    if after_rowid.is_none() {
        rows.reverse();
    }

    Ok(rows)
}

/// Map a `--level` string to the set of event `kind` values that should be
/// shown at that level and above (higher severity levels are always included).
///
/// Level hierarchy (ascending severity): trace < debug < info < warn < error
///
/// Returns `None` when no filtering is needed (trace = show everything).
fn level_kind_filter(level: &str) -> Option<Vec<&'static str>> {
    // error-level event kinds.
    const ERROR_KINDS: &[&str] = &["run_failed", "run_timed_out", "approval_denied"];

    // warn-level adds cancellation on top of error.
    const WARN_EXTRA: &[&str] = &["run_cancelled"];

    // info-level adds normal lifecycle events.
    const INFO_EXTRA: &[&str] = &[
        "run_created",
        "run_queued",
        "run_started",
        "run_completing",
        "run_completed",
        "run_retry_queued",
        "approval_requested",
        "approval_granted",
        "turn_started",
        "turn_completed",
    ];

    // debug-level adds effect pipeline and tool streaming events.
    const DEBUG_EXTRA: &[&str] = &[
        "effect_intent_created",
        "effect_attempt_started",
        "effect_outcome_recorded",
        "tool_call_started",
        "tool_call_completed",
        "progress_update",
    ];

    match level.to_ascii_lowercase().as_str() {
        "trace" => None, // no filter — show everything
        "debug" => {
            let mut kinds = Vec::with_capacity(
                ERROR_KINDS.len() + WARN_EXTRA.len() + INFO_EXTRA.len() + DEBUG_EXTRA.len(),
            );
            kinds.extend_from_slice(ERROR_KINDS);
            kinds.extend_from_slice(WARN_EXTRA);
            kinds.extend_from_slice(INFO_EXTRA);
            kinds.extend_from_slice(DEBUG_EXTRA);
            Some(kinds)
        }
        "warn" => {
            let mut kinds = Vec::with_capacity(ERROR_KINDS.len() + WARN_EXTRA.len());
            kinds.extend_from_slice(ERROR_KINDS);
            kinds.extend_from_slice(WARN_EXTRA);
            Some(kinds)
        }
        "error" => Some(ERROR_KINDS.to_vec()),
        // "info" is the default; include all lifecycle events.
        _ => {
            let mut kinds =
                Vec::with_capacity(ERROR_KINDS.len() + WARN_EXTRA.len() + INFO_EXTRA.len());
            kinds.extend_from_slice(ERROR_KINDS);
            kinds.extend_from_slice(WARN_EXTRA);
            kinds.extend_from_slice(INFO_EXTRA);
            Some(kinds)
        }
    }
}

/// Build the SQL string and parameter list.
///
/// We avoid parameterised limit/offset for the branch distinction and instead
/// embed the limit directly since it is controlled by CLI input (not user data).
///
/// The cursor (`after_rowid`) uses the SQLite implicit `rowid` column, which is
/// a global monotonic integer across all rows in `run_events`.  This is correct
/// for follow-mode polling: `sequence` is per-run and cannot be used as a
/// global cursor because two different runs can share the same sequence number.
fn build_query(cmd: &LogsCmd, after_rowid: Option<i64>) -> (String, Vec<String>) {
    // Bug fix #2: lines == 0 means "no limit"; use -1 which SQLite treats as
    // unlimited rather than LIMIT 0 which returns zero rows.
    let limit_clause = if cmd.lines == 0 {
        String::new() // omit LIMIT entirely
    } else {
        format!("LIMIT {}", cmd.lines)
    };

    // Bug fix #1: apply --level filter as a WHERE clause on the `kind` column.
    // `level_kind_filter` returns None for "trace" (show all) and Some(kinds)
    // otherwise.  We materialise the IN list directly into the SQL string
    // because the kind values come from our own static tables (not user input).
    let level_clause = level_kind_filter(&cmd.level).map(|kinds| {
        let quoted: Vec<String> = kinds.iter().map(|k| format!("'{k}'")).collect();
        format!("AND kind IN ({})", quoted.join(", "))
    });
    let level_sql = level_clause.as_deref().unwrap_or("");

    let mut params: Vec<String> = Vec::new();

    // Bug fix #3: use `rowid` (global monotonic) as the follow-mode cursor
    // instead of `sequence` (per-run monotonic).  The SELECT list now returns
    // `rowid` as column 0 so the caller can advance the cursor correctly.
    let sql = if let Some(run_id) = &cmd.run_id {
        params.push(run_id.clone());
        if let Some(rowid) = after_rowid {
            params.push(rowid.to_string());
            format!(
                "SELECT rowid, kind, run_id, data_json, timestamp
                 FROM run_events
                 WHERE run_id = ?1 AND rowid > ?2
                 {level_sql}
                 ORDER BY rowid ASC
                 {limit_clause}"
            )
        } else {
            format!(
                "SELECT rowid, kind, run_id, data_json, timestamp
                 FROM run_events
                 WHERE run_id = ?1
                 {level_sql}
                 ORDER BY rowid DESC
                 {limit_clause}"
            )
        }
    } else if let Some(rowid) = after_rowid {
        params.push(rowid.to_string());
        format!(
            "SELECT rowid, kind, run_id, data_json, timestamp
             FROM run_events
             WHERE rowid > ?1
             {level_sql}
             ORDER BY rowid ASC
             {limit_clause}"
        )
    } else {
        format!(
            "SELECT rowid, kind, run_id, data_json, timestamp
             FROM run_events
             WHERE 1=1
             {level_sql}
             ORDER BY rowid DESC
             {limit_clause}"
        )
    };

    (sql, params)
}

fn print_event(rowid: i64, kind: &str, run_id: &str, data: &str, timestamp: &str) {
    // Shorten the timestamp to just the time portion for readability.
    let ts = timestamp
        .split('T')
        .nth(1)
        .and_then(|s| s.split('.').next())
        .unwrap_or(timestamp);

    // Truncate run_id to first 8 chars.
    let short_run = &run_id[..run_id.len().min(8)];

    // Truncate data for human display.
    let short_data = truncate(data, 80);

    println!("[{ts}] #{rowid:<6} run={short_run}  {kind:<24}  {short_data}");
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_owned()
    } else {
        format!("{}...", &s[..max.saturating_sub(3)])
    }
}
