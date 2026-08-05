//! Safe, exact read models for projecting durable tool effects to surfaces.

use chrono::{DateTime, Utc};
use polkagent_core::{EffectAttemptId, EffectId, RunId};
use rusqlite::OptionalExtension as _;

use crate::{SqlitePool, StoreError, StoreResult};

/// Surface-safe terminal classification derived from a persisted effect outcome.
///
/// Deliberately excludes result data and human-authored error messages so a
/// projection cannot accidentally copy tool payloads or secrets to a client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DurableToolOutcome {
    /// The handler returned successfully.
    Success,
    /// The handler failed with the persisted typed error class.
    Failure { error_class: String },
    /// The handler timed out.
    Timeout,
    /// The handler was cancelled.
    Cancelled,
    /// The external outcome is genuinely indeterminate.
    Unknown,
}

/// One durable tool intent after at least one exact execution attempt exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableToolCall {
    /// Stable effect intent identity used as the cross-surface tool call ID.
    pub intent_id: EffectId,
    /// Owning run.
    pub run_id: RunId,
    /// Canonical registry tool name. Arguments are intentionally not loaded.
    pub tool_name: String,
    /// Exact persisted attempts in attempt order.
    pub attempts: Vec<DurableToolAttempt>,
    /// Exact immutable outcome, when present.
    pub outcome: Option<DurableToolCallOutcome>,
}

/// One exact persisted attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableToolAttempt {
    /// Stable attempt identity.
    pub attempt_id: EffectAttemptId,
    /// Persisted attempt start time.
    pub started_at: DateTime<Utc>,
}

/// One immutable outcome linked to its exact persisted attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableToolCallOutcome {
    /// Attempt that produced the outcome.
    pub attempt_id: EffectAttemptId,
    /// Surface-safe terminal classification.
    pub result: DurableToolOutcome,
    /// Persisted observation time.
    pub observed_at: DateTime<Utc>,
}

impl SqlitePool {
    /// Load registered-tool effects for one run without exposing arguments or output.
    ///
    /// Legacy outcome rows created before exact `attempt_id` persistence are
    /// accepted only when their intent has exactly one attempt, making the
    /// relationship unambiguous. Ambiguous rows fail closed.
    pub async fn durable_tool_calls(&self, run_id: RunId) -> StoreResult<Vec<DurableToolCall>> {
        let pool = self.clone();
        tokio::task::spawn_blocking(move || load_tool_calls(&pool, run_id))
            .await
            .map_err(|error| StoreError::Pool(format!("load durable tool calls task: {error}")))?
    }
}

fn load_tool_calls(pool: &SqlitePool, run_id: RunId) -> StoreResult<Vec<DurableToolCall>> {
    let writer = pool.writer();
    let mut intents = writer.prepare(
        "SELECT id, params_json
         FROM effect_intents
         WHERE run_id = ?1 AND kind = 'tool_call'
         ORDER BY created_at ASC, id ASC",
    )?;
    let rows = intents
        .query_map([run_id.to_string()], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(intents);

    rows.into_iter()
        .map(|(intent_id, params_json)| {
            let intent_id = intent_id.parse::<EffectId>()?;
            let params: serde_json::Value = serde_json::from_str(&params_json)?;
            let tool_name = params
                .get("tool_name")
                .and_then(serde_json::Value::as_str)
                .filter(|name| !name.is_empty())
                .ok_or_else(|| {
                    invalid_projection("durable tool intent has no canonical tool_name")
                })?
                .to_owned();

            let mut attempt_query = writer.prepare(
                "SELECT id, started_at
                 FROM effect_attempts
                 WHERE intent_id = ?1
                 ORDER BY attempt_number ASC, started_at ASC, id ASC",
            )?;
            let attempts = attempt_query
                .query_map([intent_id.to_string()], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?
                .map(|row| {
                    let (attempt_id, started_at) = row?;
                    Ok(DurableToolAttempt {
                        attempt_id: attempt_id.parse()?,
                        started_at: parse_timestamp(&started_at)?,
                    })
                })
                .collect::<StoreResult<Vec<_>>>()?;

            let outcome = writer
                .query_row(
                    "SELECT attempt_id, result_json, created_at
                     FROM effect_outcomes WHERE intent_id = ?1",
                    [intent_id.to_string()],
                    |row| {
                        Ok((
                            row.get::<_, Option<String>>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    },
                )
                .optional()?
                .map(|(attempt_id, result_json, observed_at)| {
                    let attempt_id = match attempt_id {
                        Some(attempt_id) => attempt_id.parse::<EffectAttemptId>()?,
                        None if attempts.len() == 1 => attempts[0].attempt_id,
                        None => {
                            return Err(invalid_projection(
                                "durable tool outcome has no unambiguous attempt",
                            ));
                        }
                    };
                    if !attempts
                        .iter()
                        .any(|attempt| attempt.attempt_id == attempt_id)
                    {
                        return Err(invalid_projection(
                            "durable tool outcome references an attempt from another intent",
                        ));
                    }
                    Ok(DurableToolCallOutcome {
                        attempt_id,
                        result: decode_outcome(&result_json)?,
                        observed_at: parse_timestamp(&observed_at)?,
                    })
                })
                .transpose()?;

            Ok(DurableToolCall {
                intent_id,
                run_id,
                tool_name,
                attempts,
                outcome,
            })
        })
        .collect()
}

fn decode_outcome(result_json: &str) -> StoreResult<DurableToolOutcome> {
    let payload: serde_json::Value = serde_json::from_str(result_json)?;
    match payload.get("variant").and_then(serde_json::Value::as_str) {
        Some("success") => Ok(DurableToolOutcome::Success),
        Some("failure") => {
            let error_class = payload
                .get("error_class")
                .and_then(serde_json::Value::as_str)
                .filter(|class| {
                    matches!(
                        *class,
                        "client_error"
                            | "server_error"
                            | "network_error"
                            | "authorization_error"
                            | "resource_exhaustion"
                            | "chain_error"
                    )
                })
                .unwrap_or("unknown")
                .to_owned();
            Ok(DurableToolOutcome::Failure { error_class })
        }
        Some("timeout") => Ok(DurableToolOutcome::Timeout),
        Some("cancelled") => Ok(DurableToolOutcome::Cancelled),
        Some("unknown") => Ok(DurableToolOutcome::Unknown),
        _ => Err(invalid_projection(
            "durable tool outcome has no recognized variant",
        )),
    }
}

fn invalid_projection(message: &'static str) -> StoreError {
    StoreError::Json(serde_json::Error::io(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        message,
    )))
}

fn parse_timestamp(value: &str) -> StoreResult<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|timestamp| timestamp.with_timezone(&Utc))
        .map_err(|_| StoreError::InvalidTimestamp(value.to_owned()))
}
