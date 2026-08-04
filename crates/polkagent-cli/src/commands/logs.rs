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
        for (seq, kind, run_id, data, timestamp) in &rows {
            print_event(*seq, kind, run_id, data, timestamp);
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
    let mut last_seq: i64 = 0;

    for (seq, kind, run_id, data, timestamp) in &initial {
        print_event(*seq, kind, run_id, data, timestamp);
        last_seq = last_seq.max(*seq);
    }

    // Then poll for new events.
    loop {
        std::thread::sleep(std::time::Duration::from_secs(1));

        let new_events = fetch_events(&reader, cmd, Some(last_seq))?;

        for (seq, kind, run_id, data, timestamp) in &new_events {
            print_event(*seq, kind, run_id, data, timestamp);
            last_seq = last_seq.max(*seq);
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

type EventRow = (i64, String, String, String, String);

fn fetch_events(
    reader: &rusqlite::Connection,
    cmd: &LogsCmd,
    after_seq: Option<i64>,
) -> Result<Vec<EventRow>> {
    // Build the SQL query dynamically to avoid the stmt lifetime issue.
    let (sql, params_vec) = build_query(cmd, after_seq);

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

    // For the "tail" queries (no after_seq) we fetched in DESC order then
    // reverse to display chronologically.
    if after_seq.is_none() {
        rows.reverse();
    }

    Ok(rows)
}

/// Build the SQL string and parameter list.
///
/// We avoid parameterised limit/offset for the branch distinction and instead
/// embed the limit directly since it is controlled by CLI input (not user data).
fn build_query(cmd: &LogsCmd, after_seq: Option<i64>) -> (String, Vec<String>) {
    #[allow(clippy::cast_possible_wrap)]
    let limit = cmd.lines as i64;
    let mut params: Vec<String> = Vec::new();

    let sql = if let Some(run_id) = &cmd.run_id {
        params.push(run_id.clone());
        if let Some(seq) = after_seq {
            params.push(seq.to_string());
            format!(
                "SELECT sequence, kind, run_id, data_json, timestamp
                 FROM run_events
                 WHERE run_id = ?1 AND sequence > ?2
                 ORDER BY sequence ASC
                 LIMIT {limit}"
            )
        } else {
            format!(
                "SELECT sequence, kind, run_id, data_json, timestamp
                 FROM run_events
                 WHERE run_id = ?1
                 ORDER BY sequence DESC
                 LIMIT {limit}"
            )
        }
    } else if let Some(seq) = after_seq {
        params.push(seq.to_string());
        format!(
            "SELECT sequence, kind, run_id, data_json, timestamp
             FROM run_events
             WHERE sequence > ?1
             ORDER BY sequence ASC
             LIMIT {limit}"
        )
    } else {
        format!(
            "SELECT sequence, kind, run_id, data_json, timestamp
             FROM run_events
             ORDER BY sequence DESC
             LIMIT {limit}"
        )
    };

    (sql, params)
}

fn print_event(seq: i64, kind: &str, run_id: &str, data: &str, timestamp: &str) {
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

    println!("[{ts}] #{seq:<6} run={short_run}  {kind:<24}  {short_data}");
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_owned()
    } else {
        format!("{}...", &s[..max.saturating_sub(3)])
    }
}
