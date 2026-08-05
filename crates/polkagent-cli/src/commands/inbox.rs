//! `polkagent inbox` — manage pending effects awaiting approval.

use anyhow::Result;
use tracing::info;

use polkagent_store_sqlite::SqlitePool;

use crate::cli::{
    InboxApproveCmd, InboxCmd, InboxDenyCmd, InboxHistoryCmd, InboxListCmd, InboxShowCmd,
    InboxSubCmd,
};

/// Dispatch the inbox subcommand.
///
/// When no subcommand is provided, defaults to `inbox list`.
pub fn run(cmd: &InboxCmd, pool: &SqlitePool) -> Result<()> {
    match &cmd.subcommand {
        None | Some(InboxSubCmd::List(_)) => {
            let default_list = InboxListCmd { json: false };
            let c = match &cmd.subcommand {
                Some(InboxSubCmd::List(c)) => c,
                _ => &default_list,
            };
            list(c, pool)
        }
        Some(InboxSubCmd::Show(c)) => show(c, pool),
        Some(InboxSubCmd::Approve(c)) => approve(c, pool),
        Some(InboxSubCmd::Deny(c)) => deny(c, pool),
        Some(InboxSubCmd::History(c)) => history(c, pool),
    }
}

// ---------------------------------------------------------------------------
// list
// ---------------------------------------------------------------------------

fn list(cmd: &InboxListCmd, pool: &SqlitePool) -> Result<()> {
    let reader = pool.reader()?;
    let mut stmt = reader.prepare(
        "SELECT id, run_id, kind, params_json, created_at
         FROM effect_intents
         WHERE claimed_by IS NULL
         ORDER BY created_at ASC",
    )?;

    let rows: Vec<(String, String, String, String, String)> = stmt
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

    if cmd.json {
        let items: Vec<serde_json::Value> = rows
            .iter()
            .map(|(id, run_id, kind, _params, created)| {
                serde_json::json!({
                    "id": id,
                    "run_id": run_id,
                    "kind": kind,
                    "created_at": created,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&items)?);
        return Ok(());
    }

    if rows.is_empty() {
        println!("No pending effects in inbox.");
        return Ok(());
    }

    println!(
        "{:<36}  {:<36}  {:<18}  Created",
        "Effect ID", "Run ID", "Kind"
    );
    println!("{}", "-".repeat(110));
    for (id, run_id, kind, _params, created) in &rows {
        println!("{id:<36}  {run_id:<36}  {kind:<18}  {created}");
    }
    println!();
    println!("{} pending effect(s)", rows.len());

    Ok(())
}

// ---------------------------------------------------------------------------
// show
// ---------------------------------------------------------------------------

fn show(cmd: &InboxShowCmd, pool: &SqlitePool) -> Result<()> {
    let reader = pool.reader()?;
    let row: Option<(
        String,
        String,
        String,
        String,
        String,
        Option<String>,
        Option<String>,
    )> = reader
        .query_row(
            "SELECT id, run_id, kind, params_json, created_at, claimed_by, claimed_until
                 FROM effect_intents
                 WHERE id = ?1",
            rusqlite::params![cmd.effect_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ))
            },
        )
        .map(Some)
        .unwrap_or(None);

    let Some((id, run_id, kind, params_json, created, claimed_by, claimed_until)) = row else {
        anyhow::bail!("Effect not found: {}", cmd.effect_id);
    };

    if cmd.json {
        let params: serde_json::Value =
            serde_json::from_str(&params_json).unwrap_or(serde_json::Value::Null);
        let out = serde_json::json!({
            "id": id,
            "run_id": run_id,
            "kind": kind,
            "params": params,
            "created_at": created,
            "claimed_by": claimed_by,
            "claimed_until": claimed_until,
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        println!("Effect: {id}");
        println!("  Run ID:       {run_id}");
        println!("  Kind:         {kind}");
        println!("  Created:      {created}");
        if let Some(ref by) = claimed_by {
            println!("  Claimed by:   {by}");
        }
        if let Some(ref until) = claimed_until {
            println!("  Claimed until: {until}");
        }
        println!("  Params:");
        let params: serde_json::Value =
            serde_json::from_str(&params_json).unwrap_or(serde_json::Value::Null);
        println!("{}", serde_json::to_string_pretty(&params)?);
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// approve
// ---------------------------------------------------------------------------

fn approve(cmd: &InboxApproveCmd, pool: &SqlitePool) -> Result<()> {
    // Fetch and print effect details before applying.
    let reader = pool.reader()?;
    let effect: Option<(String, String, String, String)> = reader
        .query_row(
            "SELECT id, run_id, kind, params_json FROM effect_intents WHERE id = ?1",
            rusqlite::params![cmd.effect_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .map(Some)
        .unwrap_or(None);
    drop(reader);

    let Some((id, run_id, kind, params_json)) = effect else {
        anyhow::bail!("Effect not found: {}", cmd.effect_id);
    };

    println!("Effect to approve:");
    println!("  ID:     {id}");
    println!("  Run:    {run_id}");
    println!("  Kind:   {kind}");
    let params: serde_json::Value =
        serde_json::from_str(&params_json).unwrap_or(serde_json::Value::Null);
    println!("  Params: {}", serde_json::to_string_pretty(&params)?);
    println!();

    let writer = pool.writer();

    // Check the intent has no outcome yet (i.e. not already approved/denied).
    let already_resolved: bool = writer
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM effect_outcomes WHERE intent_id = ?1)",
            rusqlite::params![cmd.effect_id],
            |r| r.get(0),
        )
        .unwrap_or(false);
    if already_resolved {
        anyhow::bail!("Effect already resolved: {}", cmd.effect_id);
    }

    // Record the approval in effect_outcomes (matches TUI pattern).
    let outcome_id = uuid::Uuid::now_v7().to_string();
    writer.execute(
        "INSERT INTO effect_outcomes (id, intent_id, status, result_json, created_at)
         VALUES (?1, ?2, 'approved', '{\"source\":\"cli\"}', datetime('now'))",
        rusqlite::params![outcome_id, cmd.effect_id],
    )?;

    // Record an approval event in the run_events table.
    let event_id = uuid::Uuid::now_v7().to_string();
    let now = chrono::Utc::now().to_rfc3339();
    let event_payload = serde_json::json!({
        "effect_id": id,
        "kind": kind,
        "approved_by": "cli",
        "timestamp": now,
    })
    .to_string();

    // Determine the next sequence number for this run.
    let next_seq: i64 = writer
        .query_row(
            "SELECT COALESCE(MAX(sequence), 0) + 1 FROM run_events WHERE run_id = ?1",
            rusqlite::params![run_id],
            |r| r.get(0),
        )
        .unwrap_or(1);

    if let Err(e) = writer.execute(
        "INSERT INTO run_events (id, run_id, sequence, kind, data_json, timestamp)
         VALUES (?1, ?2, ?3, 'effect.approved', ?4, ?5)",
        rusqlite::params![event_id, run_id, next_seq, event_payload, now],
    ) {
        eprintln!("Warning: failed to record event: {e}");
    }

    println!("Effect '{}' approved.", cmd.effect_id);
    info!(effect_id = %cmd.effect_id, kind = %kind, "effect approved via CLI");
    Ok(())
}

// ---------------------------------------------------------------------------
// deny
// ---------------------------------------------------------------------------

fn deny(cmd: &InboxDenyCmd, pool: &SqlitePool) -> Result<()> {
    let reason = cmd.reason.as_deref().unwrap_or("denied via CLI");

    // Fetch and print effect details before applying.
    let reader = pool.reader()?;
    let effect: Option<(String, String, String, String)> = reader
        .query_row(
            "SELECT id, run_id, kind, params_json FROM effect_intents WHERE id = ?1",
            rusqlite::params![cmd.effect_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .map(Some)
        .unwrap_or(None);
    drop(reader);

    let Some((id, run_id, kind, params_json)) = effect else {
        anyhow::bail!("Effect not found: {}", cmd.effect_id);
    };

    println!("Effect to deny:");
    println!("  ID:     {id}");
    println!("  Run:    {run_id}");
    println!("  Kind:   {kind}");
    let params: serde_json::Value =
        serde_json::from_str(&params_json).unwrap_or(serde_json::Value::Null);
    println!("  Params: {}", serde_json::to_string_pretty(&params)?);
    println!("  Reason: {reason}");
    println!();

    let writer = pool.writer();

    // Check the intent has no outcome yet (i.e. not already approved/denied).
    let already_resolved: bool = writer
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM effect_outcomes WHERE intent_id = ?1)",
            rusqlite::params![cmd.effect_id],
            |r| r.get(0),
        )
        .unwrap_or(false);
    if already_resolved {
        anyhow::bail!("Effect already resolved: {}", cmd.effect_id);
    }

    // Record the denial in effect_outcomes (matches TUI pattern).
    let outcome_id = uuid::Uuid::now_v7().to_string();
    writer.execute(
        "INSERT INTO effect_outcomes (id, intent_id, status, result_json, created_at)
         VALUES (?1, ?2, 'denied', '{\"source\":\"cli\"}', datetime('now'))",
        rusqlite::params![outcome_id, cmd.effect_id],
    )?;

    // Record a denial event in the run_events table.
    let event_id = uuid::Uuid::now_v7().to_string();
    let now = chrono::Utc::now().to_rfc3339();
    let event_payload = serde_json::json!({
        "effect_id": id,
        "kind": kind,
        "denied_by": "cli",
        "reason": reason,
        "timestamp": now,
    })
    .to_string();

    let next_seq: i64 = writer
        .query_row(
            "SELECT COALESCE(MAX(sequence), 0) + 1 FROM run_events WHERE run_id = ?1",
            rusqlite::params![run_id],
            |r| r.get(0),
        )
        .unwrap_or(1);

    if let Err(e) = writer.execute(
        "INSERT INTO run_events (id, run_id, sequence, kind, data_json, timestamp)
         VALUES (?1, ?2, ?3, 'effect.denied', ?4, ?5)",
        rusqlite::params![event_id, run_id, next_seq, event_payload, now],
    ) {
        eprintln!("Warning: failed to record event: {e}");
    }

    println!("Effect '{}' denied. Reason: {reason}", cmd.effect_id);
    info!(effect_id = %cmd.effect_id, kind = %kind, reason = reason, "effect denied via CLI");
    Ok(())
}

// ---------------------------------------------------------------------------
// history
// ---------------------------------------------------------------------------

fn history(cmd: &InboxHistoryCmd, pool: &SqlitePool) -> Result<()> {
    let limit = cmd.limit;
    let reader = pool.reader()?;
    let mut stmt = reader.prepare(
        "SELECT id, run_id, kind, claimed_by, created_at
         FROM effect_intents
         WHERE claimed_by IS NOT NULL
         ORDER BY created_at DESC
         LIMIT ?1",
    )?;

    #[allow(clippy::cast_possible_wrap)]
    let rows: Vec<(String, String, String, String, String)> = stmt
        .query_map(rusqlite::params![limit as i64], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    if cmd.json {
        let items: Vec<serde_json::Value> = rows
            .iter()
            .map(|(id, run_id, kind, status, created)| {
                serde_json::json!({
                    "id": id,
                    "run_id": run_id,
                    "kind": kind,
                    "resolution": status,
                    "created_at": created,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&items)?);
        return Ok(());
    }

    if rows.is_empty() {
        println!("No resolved effects in history.");
        return Ok(());
    }

    println!(
        "{:<36}  {:<36}  {:<18}  {:<16}  Created",
        "Effect ID", "Run ID", "Kind", "Resolution"
    );
    println!("{}", "-".repeat(130));
    for (id, run_id, kind, status, created) in &rows {
        println!("{id:<36}  {run_id:<36}  {kind:<18}  {status:<16}  {created}");
    }
    println!();
    println!("{} resolved effect(s)", rows.len());

    Ok(())
}
