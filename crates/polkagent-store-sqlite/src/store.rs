//! SQLite implementations of the Polkagent store traits.
//!
//! Four stores are defined here, each wrapping [`SqlitePool`]:
//!
//! - [`SqliteRunStore`] — CRUD for agents, runs, turns, and steps.
//! - [`SqliteEffectStore`] — effect intents, attempts, outcomes, and lease management.
//! - [`SqliteArtifactStore`] — content-addressed artifact metadata and bodies.
//! - [`SqliteEventStore`] — ordered durable run events.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::{debug, instrument};
use uuid::Uuid;

use crate::error::{StoreError, StoreResult};
use crate::pool::SqlitePool;

// ---------------------------------------------------------------------------
// Small data-transfer types (mirroring schema columns)
// ---------------------------------------------------------------------------

/// An agent record as stored in the `agents` table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRow {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub state: String,
    pub spec_json: String,
    pub created_at: String,
    pub updated_at: String,
}

/// A run record as stored in the `runs` table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunRow {
    pub id: String,
    pub agent_id: String,
    pub conversation_id: Option<String>,
    pub state: String,
    pub params_json: String,
    pub created_at: String,
    pub updated_at: String,
    pub completed_at: Option<String>,
}

/// A turn record as stored in the `turns` table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnRow {
    pub id: String,
    pub run_id: String,
    pub sequence: i64,
    pub role: String,
    pub started_at: String,
    pub completed_at: Option<String>,
    pub input_tokens: i64,
    pub output_tokens: i64,
}

/// A step record as stored in the `steps` table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepRow {
    pub id: String,
    pub turn_id: String,
    pub sequence: i64,
    pub kind: String,
    pub started_at: String,
    pub completed_at: Option<String>,
}

/// An effect intent as stored in the `effect_intents` table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EffectIntentRow {
    pub id: String,
    pub run_id: String,
    pub turn_id: Option<String>,
    pub step_id: Option<String>,
    pub kind: String,
    pub params_json: String,
    pub idempotency_key: String,
    pub created_at: String,
    pub claimed_by: Option<String>,
    pub claimed_until: Option<String>,
}

/// An effect attempt as stored in the `effect_attempts` table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EffectAttemptRow {
    pub id: String,
    pub intent_id: String,
    pub attempt_number: i64,
    pub started_at: String,
    pub completed_at: Option<String>,
}

/// An effect outcome as stored in the `effect_outcomes` table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EffectOutcomeRow {
    pub id: String,
    pub intent_id: String,
    pub status: String,
    pub result_json: String,
    pub created_at: String,
}

/// An artifact metadata record as stored in the `artifacts` table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactRow {
    pub id: String,
    pub run_id: Option<String>,
    pub kind: String,
    pub digest_hex: String,
    pub size_bytes: i64,
    pub metadata_json: String,
    pub created_at: String,
}

/// A run event as stored in the `run_events` table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunEventRow {
    pub id: String,
    pub run_id: String,
    pub sequence: i64,
    pub kind: String,
    pub data_json: String,
    pub timestamp: String,
    pub correlation_id: Option<String>,
    pub schema_version: i64,
}

// ---------------------------------------------------------------------------
// Helper: generate a new UUID v7 string
// ---------------------------------------------------------------------------

fn new_id() -> String {
    Uuid::now_v7().to_string()
}

fn now_rfc3339() -> String {
    Utc::now().to_rfc3339()
}

// ---------------------------------------------------------------------------
// SqliteRunStore
// ---------------------------------------------------------------------------

/// SQLite-backed store for agents, runs, turns, and steps.
#[derive(Debug, Clone)]
pub struct SqliteRunStore {
    pool: SqlitePool,
}

impl SqliteRunStore {
    /// Create a new run store backed by `pool`.
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    // ------------------------------------------------------------------
    // Agents
    // ------------------------------------------------------------------

    /// Insert a new agent.
    #[instrument(skip(self))]
    pub fn create_agent(
        &self,
        name: &str,
        description: Option<&str>,
        spec_json: &str,
    ) -> StoreResult<AgentRow> {
        let row = AgentRow {
            id: new_id(),
            name: name.to_string(),
            description: description.map(str::to_string),
            state: "active".to_string(),
            spec_json: spec_json.to_string(),
            created_at: now_rfc3339(),
            updated_at: now_rfc3339(),
        };

        let writer = self.pool.writer();
        writer.execute(
            "INSERT INTO agents (id, name, description, state, spec_json, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                row.id,
                row.name,
                row.description,
                row.state,
                row.spec_json,
                row.created_at,
                row.updated_at,
            ],
        )?;

        debug!(agent_id = %row.id, "agent created");
        Ok(row)
    }

    /// Retrieve an agent by ID.
    pub fn get_agent(&self, id: &str) -> StoreResult<AgentRow> {
        let writer = self.pool.writer();
        writer
            .query_row(
                "SELECT id, name, description, state, spec_json, created_at, updated_at
                 FROM agents WHERE id = ?1",
                [id],
                |r| {
                    Ok(AgentRow {
                        id: r.get(0)?,
                        name: r.get(1)?,
                        description: r.get(2)?,
                        state: r.get(3)?,
                        spec_json: r.get(4)?,
                        created_at: r.get(5)?,
                        updated_at: r.get(6)?,
                    })
                },
            )
            .map_err(|e| {
                if matches!(e, rusqlite::Error::QueryReturnedNoRows) {
                    StoreError::NotFound(format!("agent {id}"))
                } else {
                    StoreError::Sqlite(e)
                }
            })
    }

    /// Update an agent's state.
    pub fn update_agent_state(&self, id: &str, state: &str) -> StoreResult<()> {
        let updated_at = now_rfc3339();
        let writer = self.pool.writer();
        let n = writer.execute(
            "UPDATE agents SET state = ?1, updated_at = ?2 WHERE id = ?3",
            rusqlite::params![state, updated_at, id],
        )?;
        if n == 0 {
            return Err(StoreError::NotFound(format!("agent {id}")));
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // Runs
    // ------------------------------------------------------------------

    /// Insert a new run.
    #[instrument(skip(self))]
    pub fn create_run(
        &self,
        agent_id: &str,
        conversation_id: Option<&str>,
        params_json: &str,
    ) -> StoreResult<RunRow> {
        let row = RunRow {
            id: new_id(),
            agent_id: agent_id.to_string(),
            conversation_id: conversation_id.map(str::to_string),
            state: "created".to_string(),
            params_json: params_json.to_string(),
            created_at: now_rfc3339(),
            updated_at: now_rfc3339(),
            completed_at: None,
        };

        let writer = self.pool.writer();
        writer.execute(
            "INSERT INTO runs (id, agent_id, conversation_id, state, params_json, created_at, updated_at, completed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![
                row.id,
                row.agent_id,
                row.conversation_id,
                row.state,
                row.params_json,
                row.created_at,
                row.updated_at,
                row.completed_at,
            ],
        )?;

        debug!(run_id = %row.id, "run created");
        Ok(row)
    }

    /// Retrieve a run by ID.
    pub fn get_run(&self, id: &str) -> StoreResult<RunRow> {
        let writer = self.pool.writer();
        writer
            .query_row(
                "SELECT id, agent_id, conversation_id, state, params_json, created_at, updated_at, completed_at
                 FROM runs WHERE id = ?1",
                [id],
                |r| {
                    Ok(RunRow {
                        id: r.get(0)?,
                        agent_id: r.get(1)?,
                        conversation_id: r.get(2)?,
                        state: r.get(3)?,
                        params_json: r.get(4)?,
                        created_at: r.get(5)?,
                        updated_at: r.get(6)?,
                        completed_at: r.get(7)?,
                    })
                },
            )
            .map_err(|e| {
                if matches!(e, rusqlite::Error::QueryReturnedNoRows) {
                    StoreError::NotFound(format!("run {id}"))
                } else {
                    StoreError::Sqlite(e)
                }
            })
    }

    /// List all runs for an agent.
    pub fn runs_for_agent(&self, agent_id: &str) -> StoreResult<Vec<RunRow>> {
        let writer = self.pool.writer();
        let mut stmt = writer.prepare(
            "SELECT id, agent_id, conversation_id, state, params_json, created_at, updated_at, completed_at
             FROM runs WHERE agent_id = ?1 ORDER BY created_at ASC",
        )?;
        let rows = stmt
            .query_map([agent_id], |r| {
                Ok(RunRow {
                    id: r.get(0)?,
                    agent_id: r.get(1)?,
                    conversation_id: r.get(2)?,
                    state: r.get(3)?,
                    params_json: r.get(4)?,
                    created_at: r.get(5)?,
                    updated_at: r.get(6)?,
                    completed_at: r.get(7)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Update a run's state.
    pub fn update_run_state(
        &self,
        id: &str,
        state: &str,
        completed_at: Option<&str>,
    ) -> StoreResult<()> {
        let updated_at = now_rfc3339();
        let writer = self.pool.writer();
        let n = writer.execute(
            "UPDATE runs SET state = ?1, updated_at = ?2, completed_at = ?3 WHERE id = ?4",
            rusqlite::params![state, updated_at, completed_at, id],
        )?;
        if n == 0 {
            return Err(StoreError::NotFound(format!("run {id}")));
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // Turns
    // ------------------------------------------------------------------

    /// Insert a new turn for a run.
    #[instrument(skip(self))]
    pub fn create_turn(
        &self,
        run_id: &str,
        sequence: i64,
        role: &str,
    ) -> StoreResult<TurnRow> {
        let row = TurnRow {
            id: new_id(),
            run_id: run_id.to_string(),
            sequence,
            role: role.to_string(),
            started_at: now_rfc3339(),
            completed_at: None,
            input_tokens: 0,
            output_tokens: 0,
        };

        let writer = self.pool.writer();
        writer
            .execute(
                "INSERT INTO turns (id, run_id, sequence, role, started_at, completed_at, input_tokens, output_tokens)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                rusqlite::params![
                    row.id,
                    row.run_id,
                    row.sequence,
                    row.role,
                    row.started_at,
                    row.completed_at,
                    row.input_tokens,
                    row.output_tokens,
                ],
            )
            .map_err(|e| {
                if StoreError::is_unique_violation(&e) {
                    StoreError::Duplicate(format!(
                        "turn sequence {sequence} already exists for run {run_id}"
                    ))
                } else {
                    StoreError::Sqlite(e)
                }
            })?;

        debug!(turn_id = %row.id, sequence, "turn created");
        Ok(row)
    }

    /// Retrieve a turn by ID.
    pub fn get_turn(&self, id: &str) -> StoreResult<TurnRow> {
        let writer = self.pool.writer();
        writer
            .query_row(
                "SELECT id, run_id, sequence, role, started_at, completed_at, input_tokens, output_tokens
                 FROM turns WHERE id = ?1",
                [id],
                |r| {
                    Ok(TurnRow {
                        id: r.get(0)?,
                        run_id: r.get(1)?,
                        sequence: r.get(2)?,
                        role: r.get(3)?,
                        started_at: r.get(4)?,
                        completed_at: r.get(5)?,
                        input_tokens: r.get(6)?,
                        output_tokens: r.get(7)?,
                    })
                },
            )
            .map_err(|e| {
                if matches!(e, rusqlite::Error::QueryReturnedNoRows) {
                    StoreError::NotFound(format!("turn {id}"))
                } else {
                    StoreError::Sqlite(e)
                }
            })
    }

    /// Complete a turn (set completed_at, input_tokens, output_tokens).
    pub fn complete_turn(
        &self,
        id: &str,
        input_tokens: i64,
        output_tokens: i64,
    ) -> StoreResult<()> {
        let completed_at = now_rfc3339();
        let writer = self.pool.writer();
        let n = writer.execute(
            "UPDATE turns SET completed_at = ?1, input_tokens = ?2, output_tokens = ?3 WHERE id = ?4",
            rusqlite::params![completed_at, input_tokens, output_tokens, id],
        )?;
        if n == 0 {
            return Err(StoreError::NotFound(format!("turn {id}")));
        }
        Ok(())
    }

    /// List all turns for a run in sequence order.
    pub fn turns_for_run(&self, run_id: &str) -> StoreResult<Vec<TurnRow>> {
        let writer = self.pool.writer();
        let mut stmt = writer.prepare(
            "SELECT id, run_id, sequence, role, started_at, completed_at, input_tokens, output_tokens
             FROM turns WHERE run_id = ?1 ORDER BY sequence ASC",
        )?;
        let rows = stmt
            .query_map([run_id], |r| {
                Ok(TurnRow {
                    id: r.get(0)?,
                    run_id: r.get(1)?,
                    sequence: r.get(2)?,
                    role: r.get(3)?,
                    started_at: r.get(4)?,
                    completed_at: r.get(5)?,
                    input_tokens: r.get(6)?,
                    output_tokens: r.get(7)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    // ------------------------------------------------------------------
    // Steps
    // ------------------------------------------------------------------

    /// Insert a new step for a turn.
    pub fn create_step(&self, turn_id: &str, sequence: i64, kind: &str) -> StoreResult<StepRow> {
        let row = StepRow {
            id: new_id(),
            turn_id: turn_id.to_string(),
            sequence,
            kind: kind.to_string(),
            started_at: now_rfc3339(),
            completed_at: None,
        };

        let writer = self.pool.writer();
        writer
            .execute(
                "INSERT INTO steps (id, turn_id, sequence, kind, started_at, completed_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![
                    row.id,
                    row.turn_id,
                    row.sequence,
                    row.kind,
                    row.started_at,
                    row.completed_at,
                ],
            )
            .map_err(|e| {
                if StoreError::is_unique_violation(&e) {
                    StoreError::Duplicate(format!(
                        "step sequence {sequence} already exists for turn {turn_id}"
                    ))
                } else {
                    StoreError::Sqlite(e)
                }
            })?;

        Ok(row)
    }

    /// Complete a step.
    pub fn complete_step(&self, id: &str) -> StoreResult<()> {
        let completed_at = now_rfc3339();
        let writer = self.pool.writer();
        let n = writer.execute(
            "UPDATE steps SET completed_at = ?1 WHERE id = ?2",
            rusqlite::params![completed_at, id],
        )?;
        if n == 0 {
            return Err(StoreError::NotFound(format!("step {id}")));
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// SqliteEffectStore
// ---------------------------------------------------------------------------

/// SQLite-backed store for effect intents, attempts, and outcomes.
#[derive(Debug, Clone)]
pub struct SqliteEffectStore {
    pool: SqlitePool,
}

impl SqliteEffectStore {
    /// Create a new effect store backed by `pool`.
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    // ------------------------------------------------------------------
    // Intents
    // ------------------------------------------------------------------

    /// Insert a new effect intent.
    ///
    /// Returns `StoreError::Duplicate` if the idempotency key already exists.
    #[instrument(skip(self, params_json))]
    pub fn create_intent(
        &self,
        run_id: &str,
        turn_id: Option<&str>,
        step_id: Option<&str>,
        kind: &str,
        params_json: &str,
        idempotency_key: &str,
    ) -> StoreResult<EffectIntentRow> {
        let row = EffectIntentRow {
            id: new_id(),
            run_id: run_id.to_string(),
            turn_id: turn_id.map(str::to_string),
            step_id: step_id.map(str::to_string),
            kind: kind.to_string(),
            params_json: params_json.to_string(),
            idempotency_key: idempotency_key.to_string(),
            created_at: now_rfc3339(),
            claimed_by: None,
            claimed_until: None,
        };

        let writer = self.pool.writer();
        writer
            .execute(
                "INSERT INTO effect_intents
                 (id, run_id, turn_id, step_id, kind, params_json, idempotency_key, created_at, claimed_by, claimed_until)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                rusqlite::params![
                    row.id,
                    row.run_id,
                    row.turn_id,
                    row.step_id,
                    row.kind,
                    row.params_json,
                    row.idempotency_key,
                    row.created_at,
                    row.claimed_by,
                    row.claimed_until,
                ],
            )
            .map_err(|e| {
                if StoreError::is_unique_violation(&e) {
                    StoreError::Duplicate(format!(
                        "idempotency_key '{idempotency_key}' already exists"
                    ))
                } else {
                    StoreError::Sqlite(e)
                }
            })?;

        debug!(intent_id = %row.id, %kind, "effect intent created");
        Ok(row)
    }

    /// Retrieve an intent by ID.
    pub fn get_intent(&self, id: &str) -> StoreResult<EffectIntentRow> {
        let writer = self.pool.writer();
        self.query_intent_by(&writer, "id = ?1", [id])
    }

    /// Retrieve an intent by idempotency key.
    pub fn get_intent_by_key(&self, key: &str) -> StoreResult<EffectIntentRow> {
        let writer = self.pool.writer();
        self.query_intent_by(&writer, "idempotency_key = ?1", [key])
    }

    fn query_intent_by(
        &self,
        conn: &rusqlite::Connection,
        where_clause: &str,
        params: impl rusqlite::Params,
    ) -> StoreResult<EffectIntentRow> {
        let sql = format!(
            "SELECT id, run_id, turn_id, step_id, kind, params_json, idempotency_key, created_at, claimed_by, claimed_until
             FROM effect_intents WHERE {where_clause}",
        );
        conn.query_row(&sql, params, |r| {
            Ok(EffectIntentRow {
                id: r.get(0)?,
                run_id: r.get(1)?,
                turn_id: r.get(2)?,
                step_id: r.get(3)?,
                kind: r.get(4)?,
                params_json: r.get(5)?,
                idempotency_key: r.get(6)?,
                created_at: r.get(7)?,
                claimed_by: r.get(8)?,
                claimed_until: r.get(9)?,
            })
        })
        .map_err(|e| {
            if matches!(e, rusqlite::Error::QueryReturnedNoRows) {
                StoreError::NotFound("effect_intent".to_string())
            } else {
                StoreError::Sqlite(e)
            }
        })
    }

    /// Claim an unclaimed intent (or one whose lease has expired) for a
    /// specific worker.  Sets `claimed_by` and `claimed_until`.
    ///
    /// `lease_until` must be an RFC-3339 datetime string.
    ///
    /// Returns `StoreError::NotFound` if the intent does not exist, or if it
    /// is already claimed by a different worker with an active lease.
    #[instrument(skip(self))]
    pub fn claim_intent(
        &self,
        intent_id: &str,
        worker_id: &str,
        lease_until: DateTime<Utc>,
    ) -> StoreResult<EffectIntentRow> {
        let lease_str = lease_until.to_rfc3339();
        let now_str = now_rfc3339();
        let writer = self.pool.writer();

        // Attempt to claim: update only if unclaimed OR lease expired.
        let n = writer.execute(
            "UPDATE effect_intents
             SET claimed_by = ?1, claimed_until = ?2
             WHERE id = ?3
               AND (claimed_by IS NULL OR claimed_until < ?4)",
            rusqlite::params![worker_id, lease_str, intent_id, now_str],
        )?;

        if n == 0 {
            // Either the intent does not exist, or it is actively claimed.
            return Err(StoreError::NotFound(format!(
                "effect_intent {intent_id} not found or already claimed"
            )));
        }

        self.query_intent_by(&writer, "id = ?1", [intent_id])
    }

    /// Release the claim on an intent (set `claimed_by` and `claimed_until`
    /// back to NULL).  No-op if `worker_id` does not own the current claim.
    pub fn release_intent(&self, intent_id: &str, worker_id: &str) -> StoreResult<()> {
        let writer = self.pool.writer();
        writer.execute(
            "UPDATE effect_intents SET claimed_by = NULL, claimed_until = NULL
             WHERE id = ?1 AND claimed_by = ?2",
            rusqlite::params![intent_id, worker_id],
        )?;
        Ok(())
    }

    /// Return all unclaimed intents for a run.
    pub fn unclaimed_intents_for_run(&self, run_id: &str) -> StoreResult<Vec<EffectIntentRow>> {
        let writer = self.pool.writer();
        let mut stmt = writer.prepare(
            "SELECT id, run_id, turn_id, step_id, kind, params_json, idempotency_key,
                    created_at, claimed_by, claimed_until
             FROM effect_intents
             WHERE run_id = ?1 AND claimed_by IS NULL
             ORDER BY created_at ASC",
        )?;
        let rows = stmt
            .query_map([run_id], |r| {
                Ok(EffectIntentRow {
                    id: r.get(0)?,
                    run_id: r.get(1)?,
                    turn_id: r.get(2)?,
                    step_id: r.get(3)?,
                    kind: r.get(4)?,
                    params_json: r.get(5)?,
                    idempotency_key: r.get(6)?,
                    created_at: r.get(7)?,
                    claimed_by: r.get(8)?,
                    claimed_until: r.get(9)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Return all intents whose lease has expired (claimed but lease < now).
    pub fn expired_leases(&self, cutoff: DateTime<Utc>) -> StoreResult<Vec<EffectIntentRow>> {
        let cutoff_str = cutoff.to_rfc3339();
        let writer = self.pool.writer();
        let mut stmt = writer.prepare(
            "SELECT id, run_id, turn_id, step_id, kind, params_json, idempotency_key,
                    created_at, claimed_by, claimed_until
             FROM effect_intents
             WHERE claimed_by IS NOT NULL AND claimed_until < ?1
             ORDER BY claimed_until ASC",
        )?;
        let rows = stmt
            .query_map([&cutoff_str], |r| {
                Ok(EffectIntentRow {
                    id: r.get(0)?,
                    run_id: r.get(1)?,
                    turn_id: r.get(2)?,
                    step_id: r.get(3)?,
                    kind: r.get(4)?,
                    params_json: r.get(5)?,
                    idempotency_key: r.get(6)?,
                    created_at: r.get(7)?,
                    claimed_by: r.get(8)?,
                    claimed_until: r.get(9)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    // ------------------------------------------------------------------
    // Attempts
    // ------------------------------------------------------------------

    /// Record the start of an attempt for an intent.
    pub fn create_attempt(
        &self,
        intent_id: &str,
        attempt_number: i64,
    ) -> StoreResult<EffectAttemptRow> {
        let row = EffectAttemptRow {
            id: new_id(),
            intent_id: intent_id.to_string(),
            attempt_number,
            started_at: now_rfc3339(),
            completed_at: None,
        };

        let writer = self.pool.writer();
        writer
            .execute(
                "INSERT INTO effect_attempts (id, intent_id, attempt_number, started_at, completed_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![
                    row.id,
                    row.intent_id,
                    row.attempt_number,
                    row.started_at,
                    row.completed_at,
                ],
            )
            .map_err(|e| {
                if StoreError::is_unique_violation(&e) {
                    StoreError::Duplicate(format!(
                        "attempt_number {attempt_number} already exists for intent {intent_id}"
                    ))
                } else {
                    StoreError::Sqlite(e)
                }
            })?;

        Ok(row)
    }

    /// Complete an attempt.
    pub fn complete_attempt(&self, id: &str) -> StoreResult<()> {
        let completed_at = now_rfc3339();
        let writer = self.pool.writer();
        let n = writer.execute(
            "UPDATE effect_attempts SET completed_at = ?1 WHERE id = ?2",
            rusqlite::params![completed_at, id],
        )?;
        if n == 0 {
            return Err(StoreError::NotFound(format!("effect_attempt {id}")));
        }
        Ok(())
    }

    /// Fetch all attempts for an intent.
    pub fn attempts_for_intent(&self, intent_id: &str) -> StoreResult<Vec<EffectAttemptRow>> {
        let writer = self.pool.writer();
        let mut stmt = writer.prepare(
            "SELECT id, intent_id, attempt_number, started_at, completed_at
             FROM effect_attempts WHERE intent_id = ?1 ORDER BY attempt_number ASC",
        )?;
        let rows = stmt
            .query_map([intent_id], |r| {
                Ok(EffectAttemptRow {
                    id: r.get(0)?,
                    intent_id: r.get(1)?,
                    attempt_number: r.get(2)?,
                    started_at: r.get(3)?,
                    completed_at: r.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    // ------------------------------------------------------------------
    // Outcomes
    // ------------------------------------------------------------------

    /// Record an immutable outcome for an intent.
    ///
    /// Returns `StoreError::Duplicate` if an outcome for this `intent_id`
    /// already exists (the UNIQUE constraint on the column enforces this).
    #[instrument(skip(self, result_json))]
    pub fn record_outcome(
        &self,
        intent_id: &str,
        status: &str,
        result_json: &str,
    ) -> StoreResult<EffectOutcomeRow> {
        let row = EffectOutcomeRow {
            id: new_id(),
            intent_id: intent_id.to_string(),
            status: status.to_string(),
            result_json: result_json.to_string(),
            created_at: now_rfc3339(),
        };

        let writer = self.pool.writer();
        writer
            .execute(
                "INSERT INTO effect_outcomes (id, intent_id, status, result_json, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![
                    row.id,
                    row.intent_id,
                    row.status,
                    row.result_json,
                    row.created_at,
                ],
            )
            .map_err(|e| {
                if StoreError::is_unique_violation(&e) {
                    StoreError::Duplicate(format!(
                        "outcome already exists for intent {intent_id}"
                    ))
                } else {
                    StoreError::Sqlite(e)
                }
            })?;

        debug!(outcome_id = %row.id, %intent_id, %status, "outcome recorded");
        Ok(row)
    }

    /// Retrieve an outcome by intent ID.
    pub fn get_outcome_for_intent(&self, intent_id: &str) -> StoreResult<EffectOutcomeRow> {
        let writer = self.pool.writer();
        writer
            .query_row(
                "SELECT id, intent_id, status, result_json, created_at
                 FROM effect_outcomes WHERE intent_id = ?1",
                [intent_id],
                |r| {
                    Ok(EffectOutcomeRow {
                        id: r.get(0)?,
                        intent_id: r.get(1)?,
                        status: r.get(2)?,
                        result_json: r.get(3)?,
                        created_at: r.get(4)?,
                    })
                },
            )
            .map_err(|e| {
                if matches!(e, rusqlite::Error::QueryReturnedNoRows) {
                    StoreError::NotFound(format!("outcome for intent {intent_id}"))
                } else {
                    StoreError::Sqlite(e)
                }
            })
    }
}

// ---------------------------------------------------------------------------
// SqliteArtifactStore
// ---------------------------------------------------------------------------

/// SQLite-backed store for content-addressed artifacts and bodies.
#[derive(Debug, Clone)]
pub struct SqliteArtifactStore {
    pool: SqlitePool,
}

impl SqliteArtifactStore {
    /// Create a new artifact store backed by `pool`.
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    // ------------------------------------------------------------------
    // Artifacts
    // ------------------------------------------------------------------

    /// Insert a new artifact (metadata only).
    #[instrument(skip(self, metadata_json))]
    pub fn create_artifact(
        &self,
        run_id: Option<&str>,
        kind: &str,
        digest_hex: &str,
        size_bytes: i64,
        metadata_json: &str,
    ) -> StoreResult<ArtifactRow> {
        let row = ArtifactRow {
            id: new_id(),
            run_id: run_id.map(str::to_string),
            kind: kind.to_string(),
            digest_hex: digest_hex.to_string(),
            size_bytes,
            metadata_json: metadata_json.to_string(),
            created_at: now_rfc3339(),
        };

        let writer = self.pool.writer();
        writer.execute(
            "INSERT INTO artifacts (id, run_id, kind, digest_hex, size_bytes, metadata_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                row.id,
                row.run_id,
                row.kind,
                row.digest_hex,
                row.size_bytes,
                row.metadata_json,
                row.created_at,
            ],
        )?;

        debug!(artifact_id = %row.id, %kind, %digest_hex, "artifact created");
        Ok(row)
    }

    /// Retrieve artifact metadata by ID.
    pub fn get_artifact(&self, id: &str) -> StoreResult<ArtifactRow> {
        let writer = self.pool.writer();
        writer
            .query_row(
                "SELECT id, run_id, kind, digest_hex, size_bytes, metadata_json, created_at
                 FROM artifacts WHERE id = ?1",
                [id],
                |r| {
                    Ok(ArtifactRow {
                        id: r.get(0)?,
                        run_id: r.get(1)?,
                        kind: r.get(2)?,
                        digest_hex: r.get(3)?,
                        size_bytes: r.get(4)?,
                        metadata_json: r.get(5)?,
                        created_at: r.get(6)?,
                    })
                },
            )
            .map_err(|e| {
                if matches!(e, rusqlite::Error::QueryReturnedNoRows) {
                    StoreError::NotFound(format!("artifact {id}"))
                } else {
                    StoreError::Sqlite(e)
                }
            })
    }

    /// List all artifacts for a run.
    pub fn artifacts_for_run(&self, run_id: &str) -> StoreResult<Vec<ArtifactRow>> {
        let writer = self.pool.writer();
        let mut stmt = writer.prepare(
            "SELECT id, run_id, kind, digest_hex, size_bytes, metadata_json, created_at
             FROM artifacts WHERE run_id = ?1 ORDER BY created_at ASC",
        )?;
        let rows = stmt
            .query_map([run_id], |r| {
                Ok(ArtifactRow {
                    id: r.get(0)?,
                    run_id: r.get(1)?,
                    kind: r.get(2)?,
                    digest_hex: r.get(3)?,
                    size_bytes: r.get(4)?,
                    metadata_json: r.get(5)?,
                    created_at: r.get(6)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    // ------------------------------------------------------------------
    // Bodies (content-addressed, stored by digest)
    // ------------------------------------------------------------------

    /// Store or overwrite an artifact body identified by `digest_hex`.
    ///
    /// Content addressing means the same bytes always produce the same key;
    /// if the body is already stored, this is a no-op (INSERT OR IGNORE).
    pub fn store_body(&self, digest_hex: &str, body: &[u8]) -> StoreResult<()> {
        let writer = self.pool.writer();
        writer.execute(
            "INSERT OR IGNORE INTO artifact_bodies (digest_hex, body) VALUES (?1, ?2)",
            rusqlite::params![digest_hex, body],
        )?;
        Ok(())
    }

    /// Retrieve an artifact body by digest.
    pub fn get_body(&self, digest_hex: &str) -> StoreResult<Vec<u8>> {
        let writer = self.pool.writer();
        writer
            .query_row(
                "SELECT body FROM artifact_bodies WHERE digest_hex = ?1",
                [digest_hex],
                |r| r.get(0),
            )
            .map_err(|e| {
                if matches!(e, rusqlite::Error::QueryReturnedNoRows) {
                    StoreError::NotFound(format!("artifact body {digest_hex}"))
                } else {
                    StoreError::Sqlite(e)
                }
            })
    }

    // ------------------------------------------------------------------
    // Lineage
    // ------------------------------------------------------------------

    /// Record that `child_id` was derived from `parent_id`.
    pub fn add_lineage_edge(&self, child_id: &str, parent_id: &str) -> StoreResult<()> {
        let writer = self.pool.writer();
        writer.execute(
            "INSERT OR IGNORE INTO artifact_lineage (child_id, parent_id) VALUES (?1, ?2)",
            rusqlite::params![child_id, parent_id],
        )?;
        Ok(())
    }

    /// Return all parent IDs for a given artifact.
    pub fn parents_of(&self, child_id: &str) -> StoreResult<Vec<String>> {
        let writer = self.pool.writer();
        let mut stmt = writer.prepare(
            "SELECT parent_id FROM artifact_lineage WHERE child_id = ?1",
        )?;
        let ids = stmt
            .query_map([child_id], |r| r.get(0))?
            .collect::<Result<Vec<String>, _>>()?;
        Ok(ids)
    }

    /// Return all child IDs for a given artifact.
    pub fn children_of(&self, parent_id: &str) -> StoreResult<Vec<String>> {
        let writer = self.pool.writer();
        let mut stmt = writer.prepare(
            "SELECT child_id FROM artifact_lineage WHERE parent_id = ?1",
        )?;
        let ids = stmt
            .query_map([parent_id], |r| r.get(0))?
            .collect::<Result<Vec<String>, _>>()?;
        Ok(ids)
    }
}

// ---------------------------------------------------------------------------
// SqliteEventStore
// ---------------------------------------------------------------------------

/// SQLite-backed ordered event log.
#[derive(Debug, Clone)]
pub struct SqliteEventStore {
    pool: SqlitePool,
}

impl SqliteEventStore {
    /// Create a new event store backed by `pool`.
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Append a new event to the log.
    ///
    /// `sequence` must be strictly increasing per `run_id`.  Returns
    /// `StoreError::Duplicate` if `(run_id, sequence)` already exists.
    #[instrument(skip(self, data_json))]
    pub fn append_event(
        &self,
        run_id: &str,
        sequence: i64,
        kind: &str,
        data_json: &str,
        correlation_id: Option<&str>,
        schema_version: i64,
    ) -> StoreResult<RunEventRow> {
        let row = RunEventRow {
            id: new_id(),
            run_id: run_id.to_string(),
            sequence,
            kind: kind.to_string(),
            data_json: data_json.to_string(),
            timestamp: now_rfc3339(),
            correlation_id: correlation_id.map(str::to_string),
            schema_version,
        };

        let writer = self.pool.writer();
        writer
            .execute(
                "INSERT INTO run_events
                 (id, run_id, sequence, kind, data_json, timestamp, correlation_id, schema_version)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                rusqlite::params![
                    row.id,
                    row.run_id,
                    row.sequence,
                    row.kind,
                    row.data_json,
                    row.timestamp,
                    row.correlation_id,
                    row.schema_version,
                ],
            )
            .map_err(|e| {
                if StoreError::is_unique_violation(&e) {
                    StoreError::Duplicate(format!(
                        "event sequence {sequence} already exists for run {run_id}"
                    ))
                } else {
                    StoreError::Sqlite(e)
                }
            })?;

        debug!(event_id = %row.id, %run_id, sequence, %kind, "event appended");
        Ok(row)
    }

    /// Retrieve all events for a run in sequence order.
    pub fn events_for_run(&self, run_id: &str) -> StoreResult<Vec<RunEventRow>> {
        let writer = self.pool.writer();
        let mut stmt = writer.prepare(
            "SELECT id, run_id, sequence, kind, data_json, timestamp, correlation_id, schema_version
             FROM run_events WHERE run_id = ?1 ORDER BY sequence ASC",
        )?;
        let rows = stmt
            .query_map([run_id], |r| {
                Ok(RunEventRow {
                    id: r.get(0)?,
                    run_id: r.get(1)?,
                    sequence: r.get(2)?,
                    kind: r.get(3)?,
                    data_json: r.get(4)?,
                    timestamp: r.get(5)?,
                    correlation_id: r.get(6)?,
                    schema_version: r.get(7)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Retrieve events for a run starting from a given sequence number
    /// (exclusive lower bound — i.e., events with `sequence > after`).
    pub fn events_after(&self, run_id: &str, after_sequence: i64) -> StoreResult<Vec<RunEventRow>> {
        let writer = self.pool.writer();
        let mut stmt = writer.prepare(
            "SELECT id, run_id, sequence, kind, data_json, timestamp, correlation_id, schema_version
             FROM run_events WHERE run_id = ?1 AND sequence > ?2 ORDER BY sequence ASC",
        )?;
        let rows = stmt
            .query_map(rusqlite::params![run_id, after_sequence], |r| {
                Ok(RunEventRow {
                    id: r.get(0)?,
                    run_id: r.get(1)?,
                    sequence: r.get(2)?,
                    kind: r.get(3)?,
                    data_json: r.get(4)?,
                    timestamp: r.get(5)?,
                    correlation_id: r.get(6)?,
                    schema_version: r.get(7)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Return the next available sequence number for a run (max + 1, or 1 if
    /// no events exist yet).
    pub fn next_sequence(&self, run_id: &str) -> StoreResult<i64> {
        let writer = self.pool.writer();
        let max: Option<i64> = writer.query_row(
            "SELECT MAX(sequence) FROM run_events WHERE run_id = ?1",
            [run_id],
            |r| r.get(0),
        )?;
        Ok(max.map_or(1, |m| m + 1))
    }
}
