//! SQLite implementations of the Polkagent store traits.
//!
//! Four stores are defined here, each wrapping [`SqlitePool`]:
//!
//! - [`SqliteRunStore`] — CRUD for agents, runs, turns, and steps.
//! - [`SqliteEffectStore`] — effect intents, attempts, outcomes, and lease management.
//! - [`SqliteArtifactStore`] — content-addressed artifact metadata and bodies.
//! - [`SqliteEventStore`] — ordered durable run events.

use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::{debug, instrument};
use uuid::Uuid;

use polkagent_core::{EffectAttemptId, EffectId, EffectOutcomeId, RunId, StepId, WorkerId};
use polkagent_store_trait::{
    EffectStore, StoredIntent, StoredOutcome, StoreRetryClass,
    StoreError as TraitStoreError,
};

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

    /// List agents, excluding archived unless `include_archived` is true.
    ///
    /// An optional `state_filter` narrows to rows whose `state` column
    /// matches the given value.
    pub fn list_agents(
        &self,
        state_filter: Option<&str>,
        include_archived: bool,
    ) -> StoreResult<Vec<AgentRow>> {
        let writer = self.pool.writer();

        // Build the query dynamically based on filter options.
        let (sql, params): (String, Vec<Box<dyn rusqlite::types::ToSql>>) =
            match (state_filter, include_archived) {
                (Some(filter), _) => (
                    "SELECT id, name, description, state, spec_json, created_at, updated_at \
                     FROM agents WHERE state = ?1 ORDER BY name ASC"
                        .to_string(),
                    vec![Box::new(filter.to_string()) as Box<dyn rusqlite::types::ToSql>],
                ),
                (None, true) => (
                    "SELECT id, name, description, state, spec_json, created_at, updated_at \
                     FROM agents ORDER BY name ASC"
                        .to_string(),
                    vec![],
                ),
                (None, false) => (
                    "SELECT id, name, description, state, spec_json, created_at, updated_at \
                     FROM agents WHERE state != 'archived' ORDER BY name ASC"
                        .to_string(),
                    vec![],
                ),
            };

        let mut stmt = writer.prepare(&sql)?;
        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();
        let rows = stmt
            .query_map(param_refs.as_slice(), |r| {
                Ok(AgentRow {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    description: r.get(2)?,
                    state: r.get(3)?,
                    spec_json: r.get(4)?,
                    created_at: r.get(5)?,
                    updated_at: r.get(6)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Look up an agent by either its UUID id or its name.
    ///
    /// Returns the first matching row, or `StoreError::NotFound`.
    pub fn get_agent_by_name_or_id(&self, identifier: &str) -> StoreResult<AgentRow> {
        let writer = self.pool.writer();
        writer
            .query_row(
                "SELECT id, name, description, state, spec_json, created_at, updated_at
                 FROM agents WHERE (id = ?1 OR name = ?1) LIMIT 1",
                [identifier],
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
                    StoreError::NotFound(format!("agent {identifier}"))
                } else {
                    StoreError::Sqlite(e)
                }
            })
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

// ---------------------------------------------------------------------------
// EffectStore trait implementation on SqlitePool
// ---------------------------------------------------------------------------

/// Parse a `StoreRetryClass` from its stored text representation.
fn parse_retry_class(s: &str) -> StoreRetryClass {
    match s {
        "check_before_retry" => StoreRetryClass::CheckBeforeRetry,
        "no_auto_retry" => StoreRetryClass::NoAutoRetry,
        // "idempotent" and any unrecognised value default to Idempotent.
        _ => StoreRetryClass::Idempotent,
    }
}

/// Serialize a `StoreRetryClass` to its stored text representation.
fn retry_class_to_str(rc: StoreRetryClass) -> &'static str {
    match rc {
        StoreRetryClass::Idempotent => "idempotent",
        StoreRetryClass::CheckBeforeRetry => "check_before_retry",
        StoreRetryClass::NoAutoRetry => "no_auto_retry",
    }
}

/// Parse an RFC-3339 timestamp string into a `DateTime<Utc>`.
fn parse_ts(s: &str) -> Result<DateTime<Utc>, TraitStoreError> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| TraitStoreError::Internal {
            message: format!("invalid timestamp '{s}': {e}"),
        })
}

/// Parse a UUID string into a typed ID, returning a `TraitStoreError` on failure.
fn parse_id<T: From<Uuid>>(s: &str, resource_type: &'static str) -> Result<T, TraitStoreError> {
    Uuid::parse_str(s)
        .map(T::from)
        .map_err(|e| TraitStoreError::Internal {
            message: format!("invalid {resource_type} UUID '{s}': {e}"),
        })
}

/// Map a `rusqlite::Error` to the appropriate `TraitStoreError` variant.
fn map_sqlite_err(e: rusqlite::Error) -> TraitStoreError {
    TraitStoreError::Internal {
        message: format!("sqlite error: {e}"),
    }
}


/// Read a `StoredIntent` from a row.  The SELECT columns must be:
///
/// 0: id, 1: `run_id`, 2: `step_id`, 3: state, 4: `claimed_by`, 5: `claimed_until`,
/// 6: `retry_class`, 7: kind, 8: `params_json`, 9: `idempotency_key`, 10: `created_at`
fn row_to_stored_intent(r: &rusqlite::Row<'_>) -> rusqlite::Result<StoredIntentRaw> {
    Ok(StoredIntentRaw {
        id: r.get(0)?,
        run_id: r.get(1)?,
        step_id: r.get(2)?,
        state: r.get(3)?,
        claimed_by: r.get(4)?,
        claimed_until: r.get(5)?,
        retry_class: r.get(6)?,
        kind: r.get(7)?,
        params_json: r.get(8)?,
        idempotency_key: r.get(9)?,
        created_at: r.get(10)?,
    })
}

/// Intermediate raw row before parsing into `StoredIntent`.
struct StoredIntentRaw {
    id: String,
    run_id: String,
    step_id: Option<String>,
    state: String,
    claimed_by: Option<String>,
    claimed_until: Option<String>,
    retry_class: String,
    kind: String,
    params_json: String,
    idempotency_key: String,
    created_at: String,
}

impl StoredIntentRaw {
    fn into_stored_intent(self) -> Result<StoredIntent, TraitStoreError> {
        let payload_inner: serde_json::Value =
            serde_json::from_str(&self.params_json).map_err(|e| TraitStoreError::Serialisation {
                message: format!("intent params_json: {e}"),
            })?;

        // Build the payload as { "kind": "<kind>", "params": <params_json> }
        let payload = serde_json::json!({
            "kind": self.kind,
            "params": payload_inner,
        });

        let lease_owner = self
            .claimed_by
            .map(|s| parse_id::<WorkerId>(&s, "WorkerId"))
            .transpose()?;

        let lease_expires = self
            .claimed_until
            .map(|s| parse_ts(&s))
            .transpose()?;

        let step_id = self
            .step_id
            .as_deref()
            .map(|s| parse_id::<StepId>(s, "StepId"))
            .transpose()?
            // If no step_id is stored, generate a nil/default one.
            .unwrap_or_else(StepId::new);

        Ok(StoredIntent {
            id: parse_id(&self.id, "EffectId")?,
            run_id: parse_id(&self.run_id, "RunId")?,
            step_id,
            state: self.state,
            lease_owner,
            lease_expires,
            retry_class: parse_retry_class(&self.retry_class),
            payload,
            idempotency_key: self.idempotency_key,
            created_at: parse_ts(&self.created_at)?,
        })
    }
}

/// The SELECT clause used by all intent queries.
const INTENT_SELECT: &str =
    "SELECT id, run_id, step_id, state, claimed_by, claimed_until, \
            retry_class, kind, params_json, idempotency_key, created_at \
     FROM effect_intents";

#[async_trait]
impl EffectStore for SqlitePool {
    // ------------------------------------------------------------------
    // Intent lifecycle
    // ------------------------------------------------------------------

    async fn propose_intent(&self, intent: StoredIntent) -> Result<(), TraitStoreError> {
        let pool = self.clone();
        tokio::task::spawn_blocking(move || {
            let id_str = intent.id.to_string();
            let run_id_str = intent.run_id.to_string();
            let step_id_str = intent.step_id.to_string();
            let created_at_str = intent.created_at.to_rfc3339();
            let retry_class_str = retry_class_to_str(intent.retry_class);

            // Extract kind and params from the payload envelope.
            let kind = intent
                .payload
                .get("kind")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown")
                .to_string();
            let params = intent
                .payload
                .get("params")
                .cloned()
                .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
            let params_json = serde_json::to_string(&params).map_err(|e| {
                TraitStoreError::Serialisation {
                    message: format!("params: {e}"),
                }
            })?;

            let writer = pool.writer();
            writer
                .execute(
                    "INSERT INTO effect_intents \
                     (id, run_id, step_id, kind, params_json, idempotency_key, \
                      created_at, state, retry_class) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    rusqlite::params![
                        id_str,
                        run_id_str,
                        step_id_str,
                        kind,
                        params_json,
                        intent.idempotency_key,
                        created_at_str,
                        intent.state,
                        retry_class_str,
                    ],
                )
                .map_err(|e| {
                    if StoreError::is_unique_violation(&e) {
                        TraitStoreError::Conflict {
                            resource_type: "EffectIntent",
                            id: id_str.clone(),
                        }
                    } else {
                        map_sqlite_err(e)
                    }
                })?;

            debug!(intent_id = %id_str, %kind, "effect intent proposed via EffectStore");
            Ok(())
        })
        .await
        .map_err(|e| TraitStoreError::Internal {
            message: format!("spawn_blocking join: {e}"),
        })?
    }

    async fn claim_intent(
        &self,
        worker_id: WorkerId,
        lease_duration: Duration,
    ) -> Result<Option<StoredIntent>, TraitStoreError> {
        let pool = self.clone();
        tokio::task::spawn_blocking(move || {
            let worker_str = worker_id.to_string();
            let lease_until = (Utc::now() + chrono::Duration::from_std(lease_duration)
                .map_err(|e| TraitStoreError::Internal {
                    message: format!("duration conversion: {e}"),
                })?)
            .to_rfc3339();

            let writer = pool.writer();

            // BEGIN IMMEDIATE to serialise concurrent writers.
            writer.execute_batch("BEGIN IMMEDIATE").map_err(map_sqlite_err)?;

            let result = (|| -> Result<Option<StoredIntent>, TraitStoreError> {
                // Find the first pending intent.
                let maybe_id: Option<String> = writer
                    .query_row(
                        "SELECT id FROM effect_intents \
                         WHERE state = 'pending' \
                         ORDER BY created_at ASC LIMIT 1",
                        [],
                        |r| r.get(0),
                    )
                    .map(Some)
                    .or_else(|e| {
                        if matches!(e, rusqlite::Error::QueryReturnedNoRows) {
                            Ok(None)
                        } else {
                            Err(map_sqlite_err(e))
                        }
                    })?;

                let Some(intent_id) = maybe_id else {
                    return Ok(None);
                };

                // Claim it.
                writer
                    .execute(
                        "UPDATE effect_intents \
                         SET state = 'claimed', claimed_by = ?1, claimed_until = ?2 \
                         WHERE id = ?3 AND state = 'pending'",
                        rusqlite::params![worker_str, lease_until, intent_id],
                    )
                    .map_err(map_sqlite_err)?;

                // Read back the full row.
                let raw = writer
                    .query_row(
                        &format!("{INTENT_SELECT} WHERE id = ?1"),
                        [&intent_id],
                        row_to_stored_intent,
                    )
                    .map_err(map_sqlite_err)?;

                Ok(Some(raw.into_stored_intent()?))
            })();

            match &result {
                Ok(_) => writer.execute_batch("COMMIT").map_err(map_sqlite_err)?,
                Err(_) => {
                    let _ = writer.execute_batch("ROLLBACK");
                }
            }

            result
        })
        .await
        .map_err(|e| TraitStoreError::Internal {
            message: format!("spawn_blocking join: {e}"),
        })?
    }

    async fn claim_intent_by_id(
        &self,
        intent_id: EffectId,
        worker_id: WorkerId,
        lease_duration: Duration,
    ) -> Result<StoredIntent, TraitStoreError> {
        let pool = self.clone();
        tokio::task::spawn_blocking(move || {
            let id_str = intent_id.to_string();
            let worker_str = worker_id.to_string();
            let lease_until = (Utc::now() + chrono::Duration::from_std(lease_duration)
                .map_err(|e| TraitStoreError::Internal {
                    message: format!("duration conversion: {e}"),
                })?)
            .to_rfc3339();
            let now_str = Utc::now().to_rfc3339();

            let writer = pool.writer();
            writer.execute_batch("BEGIN IMMEDIATE").map_err(map_sqlite_err)?;

            let result = (|| -> Result<StoredIntent, TraitStoreError> {
                // Attempt to claim: only if pending, or the lease has expired.
                let n = writer
                    .execute(
                        "UPDATE effect_intents \
                         SET state = 'claimed', claimed_by = ?1, claimed_until = ?2 \
                         WHERE id = ?3 \
                           AND (state = 'pending' OR (state = 'claimed' AND claimed_until < ?4))",
                        rusqlite::params![worker_str, lease_until, id_str, now_str],
                    )
                    .map_err(map_sqlite_err)?;

                if n == 0 {
                    // Check if the intent exists at all.
                    let exists: bool = writer
                        .query_row(
                            "SELECT COUNT(*) FROM effect_intents WHERE id = ?1",
                            [&id_str],
                            |r| r.get::<_, i64>(0),
                        )
                        .map(|c| c > 0)
                        .map_err(map_sqlite_err)?;

                    if !exists {
                        return Err(TraitStoreError::NotFound {
                            resource_type: "EffectIntent",
                            id: id_str.clone(),
                        });
                    }
                    return Err(TraitStoreError::InvalidTransition {
                        message: format!(
                            "intent {id_str} is not claimable (already claimed or resolved)"
                        ),
                    });
                }

                let raw = writer
                    .query_row(
                        &format!("{INTENT_SELECT} WHERE id = ?1"),
                        [&id_str],
                        row_to_stored_intent,
                    )
                    .map_err(map_sqlite_err)?;

                raw.into_stored_intent()
            })();

            match &result {
                Ok(_) => writer.execute_batch("COMMIT").map_err(map_sqlite_err)?,
                Err(_) => {
                    let _ = writer.execute_batch("ROLLBACK");
                }
            }

            result
        })
        .await
        .map_err(|e| TraitStoreError::Internal {
            message: format!("spawn_blocking join: {e}"),
        })?
    }

    async fn release_claim(
        &self,
        intent_id: EffectId,
        worker_id: WorkerId,
    ) -> Result<(), TraitStoreError> {
        let pool = self.clone();
        tokio::task::spawn_blocking(move || {
            let id_str = intent_id.to_string();
            let worker_str = worker_id.to_string();
            let writer = pool.writer();

            // No-op if already resolved or owned by a different worker.
            writer
                .execute(
                    "UPDATE effect_intents \
                     SET state = 'pending', claimed_by = NULL, claimed_until = NULL \
                     WHERE id = ?1 AND claimed_by = ?2 AND state = 'claimed'",
                    rusqlite::params![id_str, worker_str],
                )
                .map_err(map_sqlite_err)?;

            Ok(())
        })
        .await
        .map_err(|e| TraitStoreError::Internal {
            message: format!("spawn_blocking join: {e}"),
        })?
    }

    async fn get_intent(&self, intent_id: EffectId) -> Result<StoredIntent, TraitStoreError> {
        let pool = self.clone();
        tokio::task::spawn_blocking(move || {
            let id_str = intent_id.to_string();
            let writer = pool.writer();

            let raw = writer
                .query_row(
                    &format!("{INTENT_SELECT} WHERE id = ?1"),
                    [&id_str],
                    row_to_stored_intent,
                )
                .map_err(|e| {
                    if matches!(e, rusqlite::Error::QueryReturnedNoRows) {
                        TraitStoreError::NotFound {
                            resource_type: "EffectIntent",
                            id: id_str.clone(),
                        }
                    } else {
                        map_sqlite_err(e)
                    }
                })?;

            raw.into_stored_intent()
        })
        .await
        .map_err(|e| TraitStoreError::Internal {
            message: format!("spawn_blocking join: {e}"),
        })?
    }

    async fn get_by_run(&self, run_id: RunId) -> Result<Vec<StoredIntent>, TraitStoreError> {
        let pool = self.clone();
        tokio::task::spawn_blocking(move || {
            let run_str = run_id.to_string();
            let writer = pool.writer();

            let mut stmt = writer
                .prepare(&format!("{INTENT_SELECT} WHERE run_id = ?1 ORDER BY created_at ASC"))
                .map_err(map_sqlite_err)?;

            let raw_rows: Vec<StoredIntentRaw> = stmt
                .query_map([&run_str], row_to_stored_intent)
                .map_err(map_sqlite_err)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(map_sqlite_err)?;

            raw_rows
                .into_iter()
                .map(StoredIntentRaw::into_stored_intent)
                .collect()
        })
        .await
        .map_err(|e| TraitStoreError::Internal {
            message: format!("spawn_blocking join: {e}"),
        })?
    }

    async fn expired_leases(
        &self,
        cutoff: polkagent_core::Timestamp,
    ) -> Result<Vec<StoredIntent>, TraitStoreError> {
        let pool = self.clone();
        tokio::task::spawn_blocking(move || {
            let cutoff_str = cutoff.to_rfc3339();
            let writer = pool.writer();

            let mut stmt = writer
                .prepare(&format!(
                    "{INTENT_SELECT} WHERE state = 'claimed' AND claimed_until < ?1 \
                     ORDER BY claimed_until ASC"
                ))
                .map_err(map_sqlite_err)?;

            let raw_rows: Vec<StoredIntentRaw> = stmt
                .query_map([&cutoff_str], row_to_stored_intent)
                .map_err(map_sqlite_err)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(map_sqlite_err)?;

            raw_rows
                .into_iter()
                .map(StoredIntentRaw::into_stored_intent)
                .collect()
        })
        .await
        .map_err(|e| TraitStoreError::Internal {
            message: format!("spawn_blocking join: {e}"),
        })?
    }

    // ------------------------------------------------------------------
    // Attempt lifecycle
    // ------------------------------------------------------------------

    async fn record_attempt_start(
        &self,
        attempt_id: EffectAttemptId,
        intent_id: EffectId,
        worker_id: WorkerId,
        payload: serde_json::Value,
    ) -> Result<(), TraitStoreError> {
        let pool = self.clone();
        tokio::task::spawn_blocking(move || {
            let attempt_str = attempt_id.to_string();
            let intent_str = intent_id.to_string();
            let worker_str = worker_id.to_string();
            let payload_json = serde_json::to_string(&payload).map_err(|e| {
                TraitStoreError::Serialisation {
                    message: format!("attempt payload: {e}"),
                }
            })?;
            let now = now_rfc3339();

            let writer = pool.writer();

            // Determine the next attempt_number for this intent.
            let attempt_number: i64 = writer
                .query_row(
                    "SELECT COALESCE(MAX(attempt_number), 0) + 1 \
                     FROM effect_attempts WHERE intent_id = ?1",
                    [&intent_str],
                    |r| r.get(0),
                )
                .map_err(map_sqlite_err)?;

            writer
                .execute(
                    "INSERT INTO effect_attempts \
                     (id, intent_id, attempt_number, started_at, worker_id, payload_json) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    rusqlite::params![
                        attempt_str,
                        intent_str,
                        attempt_number,
                        now,
                        worker_str,
                        payload_json,
                    ],
                )
                .map_err(|e| {
                    if StoreError::is_unique_violation(&e) {
                        TraitStoreError::Conflict {
                            resource_type: "EffectAttempt",
                            id: attempt_str.clone(),
                        }
                    } else {
                        map_sqlite_err(e)
                    }
                })?;

            Ok(())
        })
        .await
        .map_err(|e| TraitStoreError::Internal {
            message: format!("spawn_blocking join: {e}"),
        })?
    }

    // ------------------------------------------------------------------
    // Outcome lifecycle
    // ------------------------------------------------------------------

    async fn record_outcome(&self, outcome: StoredOutcome) -> Result<(), TraitStoreError> {
        let pool = self.clone();
        tokio::task::spawn_blocking(move || {
            let id_str = outcome.id.to_string();
            let intent_str = outcome.intent_id.to_string();
            let attempt_str = outcome.attempt_id.to_string();
            let run_str = outcome.run_id.to_string();
            let observed_at_str = outcome.observed_at.to_rfc3339();
            let payload_json = serde_json::to_string(&outcome.payload).map_err(|e| {
                TraitStoreError::Serialisation {
                    message: format!("outcome payload: {e}"),
                }
            })?;

            // Derive status from the payload (look for a "status" field).
            let status = outcome
                .payload
                .get("status")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown")
                .to_string();

            let consumed_int: i32 = i32::from(outcome.consumed);

            let writer = pool.writer();
            writer.execute_batch("BEGIN IMMEDIATE").map_err(map_sqlite_err)?;

            let result = (|| -> Result<(), TraitStoreError> {
                writer
                    .execute(
                        "INSERT INTO effect_outcomes \
                         (id, intent_id, status, result_json, created_at, \
                          attempt_id, run_id, consumed) \
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                        rusqlite::params![
                            id_str,
                            intent_str,
                            status,
                            payload_json,
                            observed_at_str,
                            attempt_str,
                            run_str,
                            consumed_int,
                        ],
                    )
                    .map_err(|e| {
                        if StoreError::is_unique_violation(&e) {
                            TraitStoreError::Conflict {
                                resource_type: "EffectOutcome",
                                id: id_str.clone(),
                            }
                        } else {
                            map_sqlite_err(e)
                        }
                    })?;

                // Transition the intent to resolved.
                writer
                    .execute(
                        "UPDATE effect_intents SET state = 'resolved' WHERE id = ?1",
                        [&intent_str],
                    )
                    .map_err(map_sqlite_err)?;

                Ok(())
            })();

            match &result {
                Ok(()) => writer.execute_batch("COMMIT").map_err(map_sqlite_err)?,
                Err(_) => {
                    let _ = writer.execute_batch("ROLLBACK");
                }
            }

            debug!(outcome_id = %outcome.id, intent_id = %intent_str, "outcome recorded via EffectStore");
            result
        })
        .await
        .map_err(|e| TraitStoreError::Internal {
            message: format!("spawn_blocking join: {e}"),
        })?
    }

    async fn unconsumed_outcomes(
        &self,
        run_id: RunId,
    ) -> Result<Vec<StoredOutcome>, TraitStoreError> {
        let pool = self.clone();
        tokio::task::spawn_blocking(move || {
            let run_str = run_id.to_string();
            let writer = pool.writer();

            let mut stmt = writer
                .prepare(
                    "SELECT id, intent_id, attempt_id, run_id, consumed, result_json, created_at \
                     FROM effect_outcomes \
                     WHERE run_id = ?1 AND consumed = 0 \
                     ORDER BY created_at ASC",
                )
                .map_err(map_sqlite_err)?;

            let rows: Vec<StoredOutcome> = stmt
                .query_map([&run_str], |r| {
                    let id_s: String = r.get(0)?;
                    let intent_id_s: String = r.get(1)?;
                    let attempt_id_s: Option<String> = r.get(2)?;
                    let run_id_s: Option<String> = r.get(3)?;
                    let consumed_i: i32 = r.get(4)?;
                    let result_json_s: String = r.get(5)?;
                    let created_at_s: String = r.get(6)?;
                    Ok((
                        id_s,
                        intent_id_s,
                        attempt_id_s,
                        run_id_s,
                        consumed_i,
                        result_json_s,
                        created_at_s,
                    ))
                })
                .map_err(map_sqlite_err)?
                .map(|row_result| {
                    let (id_s, intent_id_s, attempt_id_s, run_id_s, consumed_i, result_json_s, created_at_s) =
                        row_result.map_err(map_sqlite_err)?;
                    let payload: serde_json::Value =
                        serde_json::from_str(&result_json_s).map_err(|e| {
                            TraitStoreError::Serialisation {
                                message: format!("outcome result_json: {e}"),
                            }
                        })?;
                    Ok(StoredOutcome {
                        id: parse_id(&id_s, "EffectOutcomeId")?,
                        intent_id: parse_id(&intent_id_s, "EffectId")?,
                        attempt_id: parse_id(
                            attempt_id_s.as_deref().unwrap_or(&EffectAttemptId::new().to_string()),
                            "EffectAttemptId",
                        )?,
                        run_id: parse_id(
                            run_id_s.as_deref().unwrap_or(&run_str),
                            "RunId",
                        )?,
                        consumed: consumed_i != 0,
                        payload,
                        observed_at: parse_ts(&created_at_s)?,
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;

            Ok(rows)
        })
        .await
        .map_err(|e| TraitStoreError::Internal {
            message: format!("spawn_blocking join: {e}"),
        })?
    }

    async fn mark_outcomes_consumed(
        &self,
        outcome_ids: &[EffectOutcomeId],
    ) -> Result<(), TraitStoreError> {
        let ids: Vec<String> = outcome_ids.iter().map(ToString::to_string).collect();
        let pool = self.clone();
        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();
            for id_str in &ids {
                writer
                    .execute(
                        "UPDATE effect_outcomes SET consumed = 1 WHERE id = ?1",
                        [id_str],
                    )
                    .map_err(map_sqlite_err)?;
            }
            Ok(())
        })
        .await
        .map_err(|e| TraitStoreError::Internal {
            message: format!("spawn_blocking join: {e}"),
        })?
    }
}

// ---------------------------------------------------------------------------
// Tests — EffectStore trait on SqlitePool
// ---------------------------------------------------------------------------

#[cfg(test)]
mod effect_store_tests {
    use super::*;
    use crate::migrations;
    use polkagent_store_trait::EffectStore;

    /// Create an in-memory pool with all migrations applied and FK-parent rows
    /// inserted so that effect rows satisfy foreign-key constraints.
    fn test_pool() -> SqlitePool {
        let pool = SqlitePool::open_in_memory().expect("open in-memory pool");
        {
            let writer = pool.writer();
            migrations::migrate(&writer).expect("migrate");
            writer
                .execute(
                    "INSERT INTO agents (id, name, state, spec_json, created_at, updated_at) \
                     VALUES ('test-agent', 'Test Agent', 'active', '{}', \
                             '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z')",
                    [],
                )
                .expect("insert test agent");
        }
        pool
    }

    /// Scaffolding row IDs returned by `insert_run_scaffold`.
    struct TestScaffold {
        run_id: RunId,
        step_id: StepId,
    }

    /// Insert the full FK chain: run -> turn -> step.
    /// Returns the run and step IDs for use in intent construction.
    fn insert_run_scaffold(pool: &SqlitePool, run_id: RunId) -> TestScaffold {
        let turn_id = uuid::Uuid::now_v7().to_string();
        let step_id = StepId::new();
        let writer = pool.writer();
        writer
            .execute(
                "INSERT INTO runs (id, agent_id, state, params_json, created_at, updated_at) \
                 VALUES (?1, 'test-agent', 'created', '{}', \
                         '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z')",
                rusqlite::params![run_id.to_string()],
            )
            .expect("insert test run");
        writer
            .execute(
                "INSERT INTO turns (id, run_id, sequence, role, started_at) \
                 VALUES (?1, ?2, 1, 'assistant', '2024-01-01T00:00:00Z')",
                rusqlite::params![turn_id, run_id.to_string()],
            )
            .expect("insert test turn");
        writer
            .execute(
                "INSERT INTO steps (id, turn_id, sequence, kind, started_at) \
                 VALUES (?1, ?2, 1, 'tool_call', '2024-01-01T00:00:00Z')",
                rusqlite::params![step_id.to_string(), turn_id],
            )
            .expect("insert test step");
        TestScaffold { run_id, step_id }
    }

    /// Build a minimal `StoredIntent` for testing, using the scaffold's step_id.
    fn make_intent(scaffold: &TestScaffold) -> StoredIntent {
        StoredIntent {
            id: EffectId::new(),
            run_id: scaffold.run_id,
            step_id: scaffold.step_id,
            state: "pending".to_string(),
            lease_owner: None,
            lease_expires: None,
            retry_class: StoreRetryClass::Idempotent,
            payload: serde_json::json!({
                "kind": "tool",
                "params": { "name": "read_file", "path": "/tmp/test" },
            }),
            idempotency_key: format!("key-{}", uuid::Uuid::now_v7()),
            created_at: chrono::Utc::now(),
        }
    }

    // ------------------------------------------------------------------
    // propose_intent
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn propose_and_get_intent() {
        let pool = test_pool();
        let run_id = RunId::new();
        let scaffold = insert_run_scaffold(&pool, run_id);

        let intent = make_intent(&scaffold);
        let intent_id = intent.id;

        EffectStore::propose_intent(&pool, intent)
            .await
            .expect("propose_intent");

        let fetched = EffectStore::get_intent(&pool, intent_id)
            .await
            .expect("get_intent");

        assert_eq!(fetched.id, intent_id);
        assert_eq!(fetched.run_id, run_id);
        assert_eq!(fetched.state, "pending");
        assert_eq!(fetched.retry_class, StoreRetryClass::Idempotent);
        assert!(fetched.lease_owner.is_none());
        assert!(fetched.lease_expires.is_none());
    }

    #[tokio::test]
    async fn propose_intent_duplicate_returns_conflict() {
        let pool = test_pool();
        let run_id = RunId::new();
        let scaffold = insert_run_scaffold(&pool, run_id);

        let intent = make_intent(&scaffold);
        EffectStore::propose_intent(&pool, intent.clone())
            .await
            .expect("first propose");

        let err = EffectStore::propose_intent(&pool, intent)
            .await
            .expect_err("duplicate should fail");

        assert!(
            matches!(err, TraitStoreError::Conflict { .. }),
            "expected Conflict, got: {err:?}"
        );
    }

    // ------------------------------------------------------------------
    // claim_intent
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn claim_intent_returns_first_pending() {
        let pool = test_pool();
        let run_id = RunId::new();
        let scaffold = insert_run_scaffold(&pool, run_id);

        let intent = make_intent(&scaffold);
        let intent_id = intent.id;
        EffectStore::propose_intent(&pool, intent)
            .await
            .expect("propose");

        let worker = WorkerId::new();
        let claimed = EffectStore::claim_intent(&pool, worker, Duration::from_secs(60))
            .await
            .expect("claim_intent");

        assert!(claimed.is_some());
        let claimed = claimed.expect("just asserted Some");
        assert_eq!(claimed.id, intent_id);
        assert_eq!(claimed.state, "claimed");
        assert_eq!(claimed.lease_owner, Some(worker));
        assert!(claimed.lease_expires.is_some());
    }

    #[tokio::test]
    async fn claim_intent_returns_none_when_empty() {
        let pool = test_pool();
        let worker = WorkerId::new();

        let claimed = EffectStore::claim_intent(&pool, worker, Duration::from_secs(60))
            .await
            .expect("claim_intent");

        assert!(claimed.is_none());
    }

    #[tokio::test]
    async fn claim_intent_skips_already_claimed() {
        let pool = test_pool();
        let run_id = RunId::new();
        let scaffold = insert_run_scaffold(&pool, run_id);

        let intent = make_intent(&scaffold);
        EffectStore::propose_intent(&pool, intent)
            .await
            .expect("propose");

        let w1 = WorkerId::new();
        let w2 = WorkerId::new();

        // First worker claims.
        let c1 = EffectStore::claim_intent(&pool, w1, Duration::from_secs(600))
            .await
            .expect("claim 1");
        assert!(c1.is_some());

        // Second worker finds nothing.
        let c2 = EffectStore::claim_intent(&pool, w2, Duration::from_secs(600))
            .await
            .expect("claim 2");
        assert!(c2.is_none());
    }

    // ------------------------------------------------------------------
    // claim_intent_by_id
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn claim_intent_by_id_succeeds() {
        let pool = test_pool();
        let run_id = RunId::new();
        let scaffold = insert_run_scaffold(&pool, run_id);

        let intent = make_intent(&scaffold);
        let intent_id = intent.id;
        EffectStore::propose_intent(&pool, intent)
            .await
            .expect("propose");

        let worker = WorkerId::new();
        let claimed =
            EffectStore::claim_intent_by_id(&pool, intent_id, worker, Duration::from_secs(60))
                .await
                .expect("claim_intent_by_id");

        assert_eq!(claimed.id, intent_id);
        assert_eq!(claimed.state, "claimed");
        assert_eq!(claimed.lease_owner, Some(worker));
    }

    #[tokio::test]
    async fn claim_intent_by_id_not_found() {
        let pool = test_pool();
        let worker = WorkerId::new();
        let missing_id = EffectId::new();

        let err =
            EffectStore::claim_intent_by_id(&pool, missing_id, worker, Duration::from_secs(60))
                .await
                .expect_err("should fail");

        assert!(
            matches!(err, TraitStoreError::NotFound { .. }),
            "expected NotFound, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn claim_intent_by_id_already_claimed() {
        let pool = test_pool();
        let run_id = RunId::new();
        let scaffold = insert_run_scaffold(&pool, run_id);

        let intent = make_intent(&scaffold);
        let intent_id = intent.id;
        EffectStore::propose_intent(&pool, intent)
            .await
            .expect("propose");

        let w1 = WorkerId::new();
        let w2 = WorkerId::new();

        EffectStore::claim_intent_by_id(&pool, intent_id, w1, Duration::from_secs(600))
            .await
            .expect("first claim");

        let err =
            EffectStore::claim_intent_by_id(&pool, intent_id, w2, Duration::from_secs(600))
                .await
                .expect_err("second claim should fail");

        assert!(
            matches!(err, TraitStoreError::InvalidTransition { .. }),
            "expected InvalidTransition, got: {err:?}"
        );
    }

    // ------------------------------------------------------------------
    // release_claim
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn release_claim_returns_to_pending() {
        let pool = test_pool();
        let run_id = RunId::new();
        let scaffold = insert_run_scaffold(&pool, run_id);

        let intent = make_intent(&scaffold);
        let intent_id = intent.id;
        EffectStore::propose_intent(&pool, intent)
            .await
            .expect("propose");

        let worker = WorkerId::new();
        EffectStore::claim_intent_by_id(&pool, intent_id, worker, Duration::from_secs(60))
            .await
            .expect("claim");

        EffectStore::release_claim(&pool, intent_id, worker)
            .await
            .expect("release_claim");

        let fetched = EffectStore::get_intent(&pool, intent_id)
            .await
            .expect("get_intent");
        assert_eq!(fetched.state, "pending");
        assert!(fetched.lease_owner.is_none());
    }

    #[tokio::test]
    async fn release_claim_is_noop_for_different_worker() {
        let pool = test_pool();
        let run_id = RunId::new();
        let scaffold = insert_run_scaffold(&pool, run_id);

        let intent = make_intent(&scaffold);
        let intent_id = intent.id;
        EffectStore::propose_intent(&pool, intent)
            .await
            .expect("propose");

        let owner = WorkerId::new();
        let other = WorkerId::new();
        EffectStore::claim_intent_by_id(&pool, intent_id, owner, Duration::from_secs(60))
            .await
            .expect("claim");

        // Release by a different worker should be a no-op.
        EffectStore::release_claim(&pool, intent_id, other)
            .await
            .expect("release_claim by other");

        let fetched = EffectStore::get_intent(&pool, intent_id)
            .await
            .expect("get_intent");
        assert_eq!(fetched.state, "claimed");
        assert_eq!(fetched.lease_owner, Some(owner));
    }

    // ------------------------------------------------------------------
    // get_by_run
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn get_by_run_returns_all_intents() {
        let pool = test_pool();
        let run_id = RunId::new();
        let scaffold = insert_run_scaffold(&pool, run_id);

        for _ in 0..3 {
            EffectStore::propose_intent(&pool, make_intent(&scaffold))
                .await
                .expect("propose");
        }

        let intents = EffectStore::get_by_run(&pool, run_id)
            .await
            .expect("get_by_run");
        assert_eq!(intents.len(), 3);
    }

    #[tokio::test]
    async fn get_by_run_excludes_other_runs() {
        let pool = test_pool();
        let r1 = RunId::new();
        let r2 = RunId::new();
        let s1 = insert_run_scaffold(&pool, r1);
        let s2 = insert_run_scaffold(&pool, r2);

        EffectStore::propose_intent(&pool, make_intent(&s1))
            .await
            .expect("propose r1");
        EffectStore::propose_intent(&pool, make_intent(&s2))
            .await
            .expect("propose r2");

        let intents = EffectStore::get_by_run(&pool, r1)
            .await
            .expect("get_by_run r1");
        assert_eq!(intents.len(), 1);
        assert_eq!(intents[0].run_id, r1);
    }

    // ------------------------------------------------------------------
    // expired_leases
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn expired_leases_finds_expired_claims() {
        let pool = test_pool();
        let run_id = RunId::new();
        let scaffold = insert_run_scaffold(&pool, run_id);

        let intent = make_intent(&scaffold);
        let intent_id = intent.id;
        EffectStore::propose_intent(&pool, intent)
            .await
            .expect("propose");

        let worker = WorkerId::new();
        // Claim with a very short lease (1 ms effectively already expired).
        EffectStore::claim_intent_by_id(&pool, intent_id, worker, Duration::from_millis(1))
            .await
            .expect("claim");

        // Small sleep to ensure the lease expires.
        tokio::time::sleep(Duration::from_millis(10)).await;

        let cutoff = chrono::Utc::now();
        let expired = EffectStore::expired_leases(&pool, cutoff)
            .await
            .expect("expired_leases");

        assert_eq!(expired.len(), 1);
        assert_eq!(expired[0].id, intent_id);
    }

    // ------------------------------------------------------------------
    // record_attempt_start
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn record_attempt_start_succeeds() {
        let pool = test_pool();
        let run_id = RunId::new();
        let scaffold = insert_run_scaffold(&pool, run_id);

        let intent = make_intent(&scaffold);
        let intent_id = intent.id;
        EffectStore::propose_intent(&pool, intent)
            .await
            .expect("propose");

        let attempt_id = EffectAttemptId::new();
        let worker = WorkerId::new();
        let payload = serde_json::json!({"strategy": "direct"});

        EffectStore::record_attempt_start(&pool, attempt_id, intent_id, worker, payload)
            .await
            .expect("record_attempt_start");

        // Verify the attempt is in the database.
        let writer = pool.writer();
        let count: i64 = writer
            .query_row(
                "SELECT COUNT(*) FROM effect_attempts WHERE id = ?1",
                [attempt_id.to_string()],
                |r| r.get(0),
            )
            .expect("count");
        assert_eq!(count, 1);
    }

    // ------------------------------------------------------------------
    // record_outcome
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn record_outcome_transitions_to_resolved() {
        let pool = test_pool();
        let run_id = RunId::new();
        let scaffold = insert_run_scaffold(&pool, run_id);

        let intent = make_intent(&scaffold);
        let intent_id = intent.id;
        EffectStore::propose_intent(&pool, intent)
            .await
            .expect("propose");

        let attempt_id = EffectAttemptId::new();
        let worker = WorkerId::new();
        EffectStore::record_attempt_start(
            &pool,
            attempt_id,
            intent_id,
            worker,
            serde_json::json!({}),
        )
        .await
        .expect("record_attempt_start");

        let outcome = StoredOutcome {
            id: EffectOutcomeId::new(),
            intent_id,
            attempt_id,
            run_id,
            consumed: false,
            payload: serde_json::json!({"status": "success", "data": 42}),
            observed_at: chrono::Utc::now(),
        };

        EffectStore::record_outcome(&pool, outcome)
            .await
            .expect("record_outcome");

        // Intent should now be resolved.
        let fetched = EffectStore::get_intent(&pool, intent_id)
            .await
            .expect("get_intent");
        assert_eq!(fetched.state, "resolved");
    }

    #[tokio::test]
    async fn record_outcome_duplicate_returns_conflict() {
        let pool = test_pool();
        let run_id = RunId::new();
        let scaffold = insert_run_scaffold(&pool, run_id);

        let intent = make_intent(&scaffold);
        let intent_id = intent.id;
        EffectStore::propose_intent(&pool, intent)
            .await
            .expect("propose");

        let attempt_id = EffectAttemptId::new();
        let worker = WorkerId::new();
        EffectStore::record_attempt_start(
            &pool,
            attempt_id,
            intent_id,
            worker,
            serde_json::json!({}),
        )
        .await
        .expect("record_attempt_start");

        let outcome = StoredOutcome {
            id: EffectOutcomeId::new(),
            intent_id,
            attempt_id,
            run_id,
            consumed: false,
            payload: serde_json::json!({"status": "success"}),
            observed_at: chrono::Utc::now(),
        };

        EffectStore::record_outcome(&pool, outcome.clone())
            .await
            .expect("first outcome");

        // Second outcome for the same intent should conflict (UNIQUE on intent_id).
        let outcome2 = StoredOutcome {
            id: EffectOutcomeId::new(),
            ..outcome
        };
        let err = EffectStore::record_outcome(&pool, outcome2)
            .await
            .expect_err("duplicate outcome should fail");

        assert!(
            matches!(err, TraitStoreError::Conflict { .. }),
            "expected Conflict, got: {err:?}"
        );
    }

    // ------------------------------------------------------------------
    // unconsumed_outcomes / mark_outcomes_consumed
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn unconsumed_outcomes_and_mark_consumed() {
        let pool = test_pool();
        let run_id = RunId::new();
        let scaffold = insert_run_scaffold(&pool, run_id);

        let intent = make_intent(&scaffold);
        let intent_id = intent.id;
        EffectStore::propose_intent(&pool, intent)
            .await
            .expect("propose");

        let attempt_id = EffectAttemptId::new();
        let worker = WorkerId::new();
        EffectStore::record_attempt_start(
            &pool,
            attempt_id,
            intent_id,
            worker,
            serde_json::json!({}),
        )
        .await
        .expect("record_attempt_start");

        let outcome_id = EffectOutcomeId::new();
        let outcome = StoredOutcome {
            id: outcome_id,
            intent_id,
            attempt_id,
            run_id,
            consumed: false,
            payload: serde_json::json!({"status": "success", "value": "hello"}),
            observed_at: chrono::Utc::now(),
        };

        EffectStore::record_outcome(&pool, outcome)
            .await
            .expect("record_outcome");

        // Should appear as unconsumed.
        let unconsumed = EffectStore::unconsumed_outcomes(&pool, run_id)
            .await
            .expect("unconsumed_outcomes");
        assert_eq!(unconsumed.len(), 1);
        assert_eq!(unconsumed[0].id, outcome_id);
        assert!(!unconsumed[0].consumed);

        // Mark consumed.
        EffectStore::mark_outcomes_consumed(&pool, &[outcome_id])
            .await
            .expect("mark_outcomes_consumed");

        // Should now be empty.
        let unconsumed = EffectStore::unconsumed_outcomes(&pool, run_id)
            .await
            .expect("unconsumed after mark");
        assert!(unconsumed.is_empty());
    }

    // ------------------------------------------------------------------
    // get_intent not found
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn get_intent_not_found() {
        let pool = test_pool();
        let missing = EffectId::new();

        let err = EffectStore::get_intent(&pool, missing)
            .await
            .expect_err("should not find");

        assert!(
            matches!(err, TraitStoreError::NotFound { .. }),
            "expected NotFound, got: {err:?}"
        );
    }

    // ------------------------------------------------------------------
    // Retry class round-trip
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn retry_class_round_trips() {
        let pool = test_pool();
        let run_id = RunId::new();
        let scaffold = insert_run_scaffold(&pool, run_id);

        for rc in [
            StoreRetryClass::Idempotent,
            StoreRetryClass::CheckBeforeRetry,
            StoreRetryClass::NoAutoRetry,
        ] {
            let mut intent = make_intent(&scaffold);
            intent.retry_class = rc;

            let id = intent.id;
            EffectStore::propose_intent(&pool, intent)
                .await
                .expect("propose");

            let fetched = EffectStore::get_intent(&pool, id)
                .await
                .expect("get");
            assert_eq!(fetched.retry_class, rc);
        }
    }
}
