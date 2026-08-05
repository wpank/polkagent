use async_trait::async_trait;
use chrono::Utc;
use polkagent_core::{
    EffectAttemptId, EffectId, EffectOutcomeId, RunId, StepId, Timestamp, WorkerId,
};
use polkagent_store_trait::{
    EffectStore, StoreError, StoreRetryClass, StoredIntent, StoredOutcome,
};
use sqlx::Row;
use std::time::Duration;
use uuid::Uuid;

use crate::pool::PgPool;

fn map_pg_err(e: &sqlx::Error) -> StoreError {
    match e {
        sqlx::Error::Database(db_err) => {
            if db_err.is_unique_violation() {
                StoreError::Conflict {
                    resource_type: "EffectIntent",
                    id: String::new(),
                }
            } else {
                StoreError::Internal {
                    message: format!("postgres error: {e}"),
                }
            }
        }
        _ => StoreError::Internal {
            message: format!("postgres error: {e}"),
        },
    }
}

fn parse_ts(dt: chrono::DateTime<Utc>) -> Timestamp {
    dt
}

fn parse_id<T: From<Uuid>>(s: &str, resource_type: &'static str) -> Result<T, StoreError> {
    Uuid::parse_str(s)
        .map(T::from)
        .map_err(|e| StoreError::Internal {
            message: format!("invalid {resource_type} UUID '{s}': {e}"),
        })
}

fn derive_state(claimed_by: Option<&str>) -> &str {
    match claimed_by {
        None => "pending",
        Some("resolved") => "resolved",
        Some("failed") => "failed",
        Some("permanently_failed") => "permanently_failed",
        Some(_) => "claimed",
    }
}

fn priority_from_payload(payload: &serde_json::Value) -> Result<i32, StoreError> {
    let Some(value) = payload.get("priority") else {
        return Ok(1);
    };

    match value {
        serde_json::Value::Number(number) => {
            let value = number.as_i64().ok_or_else(|| StoreError::Serialisation {
                message: format!("intent priority is not a signed integer: {number}"),
            })?;
            i32::try_from(value).map_err(|_| StoreError::Serialisation {
                message: format!("intent priority {value} exceeds PostgreSQL INTEGER capacity"),
            })
        }
        serde_json::Value::String(value) => Ok(match value.as_str() {
            "low" => 0,
            "high" => 2,
            "critical" => 3,
            _ => 1,
        }),
        _ => Ok(1),
    }
}

fn row_to_intent(row: &sqlx::postgres::PgRow) -> Result<StoredIntent, StoreError> {
    let id: String = row.get("id");
    let run_id: String = row.get("run_id");
    let step_id: Option<String> = row.get("step_id");
    let claimed_by: Option<String> = row.get("claimed_by");
    let claimed_until: Option<chrono::DateTime<Utc>> = row.get("claimed_until");
    let kind: String = row.get("kind");
    let params_json: String = row.get("params_json");
    let idempotency_key: String = row.get("idempotency_key");
    let created_at: chrono::DateTime<Utc> = row.get("created_at");

    let state = derive_state(claimed_by.as_deref()).to_string();

    let lease_owner = claimed_by
        .as_deref()
        .filter(|s| Uuid::parse_str(s).is_ok())
        .map(|s| parse_id::<WorkerId>(s, "WorkerId"))
        .transpose()?;

    let lease_expires = claimed_until.map(parse_ts);

    let step = step_id
        .as_deref()
        .map(|s| parse_id::<StepId>(s, "StepId"))
        .transpose()?
        .unwrap_or_else(StepId::new);

    let params: serde_json::Value =
        serde_json::from_str(&params_json).map_err(|e| StoreError::Serialisation {
            message: format!("intent params_json: {e}"),
        })?;

    let payload = serde_json::json!({
        "kind": kind,
        "params": params,
    });

    Ok(StoredIntent {
        id: parse_id(&id, "EffectId")?,
        run_id: parse_id(&run_id, "RunId")?,
        step_id: step,
        state,
        lease_owner,
        lease_expires,
        retry_class: StoreRetryClass::Idempotent,
        payload,
        idempotency_key,
        created_at: parse_ts(created_at),
    })
}

const INTENT_COLS: &str =
    "id, run_id, step_id, claimed_by, claimed_until, kind, params_json, idempotency_key, created_at";

#[async_trait]
impl EffectStore for PgPool {
    async fn propose_intent(&self, intent: StoredIntent) -> Result<(), StoreError> {
        let id_str = intent.id.to_string();
        let run_id_str = intent.run_id.to_string();
        let step_id_str = intent.step_id.to_string();
        let tenant = self.tenant_id().to_string();
        let created_at = intent.created_at;

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
        let params_json =
            serde_json::to_string(&params).map_err(|e| StoreError::Serialisation {
                message: format!("params: {e}"),
            })?;

        let priority = priority_from_payload(&intent.payload)?;

        let mut tx = self
            .pool()
            .begin()
            .await
            .map_err(|e| StoreError::ConnectionError {
                message: format!("begin transaction: {e}"),
            })?;
        self.set_tenant(&mut tx)
            .await
            .map_err(|e| StoreError::Internal {
                message: format!("set tenant: {e}"),
            })?;

        sqlx::query(
            "INSERT INTO effect_intents
             (id, tenant_id, run_id, step_id, kind, params_json, idempotency_key, created_at, priority)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(&id_str)
        .bind(&tenant)
        .bind(&run_id_str)
        .bind(&step_id_str)
        .bind(&kind)
        .bind(&params_json)
        .bind(&intent.idempotency_key)
        .bind(created_at)
        .bind(priority)
        .execute(&mut *tx)
        .await
        .map_err(|e| {
            if matches!(&e, sqlx::Error::Database(db_err) if db_err.is_unique_violation()) {
                StoreError::Conflict {
                    resource_type: "EffectIntent",
                    id: id_str.clone(),
                }
            } else {
                map_pg_err(&e)
            }
        })?;

        tx.commit().await.map_err(|e| StoreError::Internal {
            message: format!("commit: {e}"),
        })?;
        Ok(())
    }

    async fn claim_intent(
        &self,
        worker_id: WorkerId,
        lease_duration: Duration,
    ) -> Result<Option<StoredIntent>, StoreError> {
        let worker_str = worker_id.to_string();
        let lease_until = Utc::now()
            + chrono::Duration::from_std(lease_duration).map_err(|e| StoreError::Internal {
                message: format!("duration conversion: {e}"),
            })?;

        let mut tx = self
            .pool()
            .begin()
            .await
            .map_err(|e| StoreError::ConnectionError {
                message: format!("begin transaction: {e}"),
            })?;
        self.set_tenant(&mut tx)
            .await
            .map_err(|e| StoreError::Internal {
                message: format!("set tenant: {e}"),
            })?;

        let maybe_row = sqlx::query(&format!(
            "UPDATE effect_intents
             SET claimed_by = $1, claimed_until = $2
             WHERE id = (
                 SELECT id FROM effect_intents
                 WHERE claimed_by IS NULL
                 ORDER BY priority DESC, created_at ASC
                 LIMIT 1
                 FOR UPDATE SKIP LOCKED
             )
             RETURNING {INTENT_COLS}"
        ))
        .bind(&worker_str)
        .bind(lease_until)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| map_pg_err(&e))?;

        let result = match maybe_row {
            Some(row) => Some(row_to_intent(&row)?),
            None => None,
        };

        tx.commit().await.map_err(|e| StoreError::Internal {
            message: format!("commit: {e}"),
        })?;
        Ok(result)
    }

    async fn claim_intent_by_id(
        &self,
        intent_id: EffectId,
        worker_id: WorkerId,
        lease_duration: Duration,
    ) -> Result<StoredIntent, StoreError> {
        let id_str = intent_id.to_string();
        let worker_str = worker_id.to_string();
        let now = Utc::now();
        let lease_until = now
            + chrono::Duration::from_std(lease_duration).map_err(|e| StoreError::Internal {
                message: format!("duration conversion: {e}"),
            })?;

        let mut tx = self
            .pool()
            .begin()
            .await
            .map_err(|e| StoreError::ConnectionError {
                message: format!("begin transaction: {e}"),
            })?;
        self.set_tenant(&mut tx)
            .await
            .map_err(|e| StoreError::Internal {
                message: format!("set tenant: {e}"),
            })?;

        let maybe_row = sqlx::query(&format!(
            "UPDATE effect_intents
             SET claimed_by = $1, claimed_until = $2
             WHERE id = $3
               AND (claimed_by IS NULL
                    OR (claimed_by != 'resolved' AND claimed_until < $4))
             RETURNING {INTENT_COLS}"
        ))
        .bind(&worker_str)
        .bind(lease_until)
        .bind(&id_str)
        .bind(now)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| map_pg_err(&e))?;

        let Some(row) = maybe_row else {
            let exists: bool =
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM effect_intents WHERE id = $1)")
                    .bind(&id_str)
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(|e| map_pg_err(&e))?;

            if !exists {
                return Err(StoreError::NotFound {
                    resource_type: "EffectIntent",
                    id: id_str,
                });
            }
            return Err(StoreError::InvalidTransition {
                message: format!("intent {id_str} is not claimable (already claimed or resolved)"),
            });
        };
        let result = row_to_intent(&row)?;

        tx.commit().await.map_err(|e| StoreError::Internal {
            message: format!("commit: {e}"),
        })?;
        Ok(result)
    }

    async fn release_claim(
        &self,
        intent_id: EffectId,
        worker_id: WorkerId,
    ) -> Result<(), StoreError> {
        let id_str = intent_id.to_string();
        let worker_str = worker_id.to_string();

        let mut tx = self
            .pool()
            .begin()
            .await
            .map_err(|e| StoreError::ConnectionError {
                message: format!("begin transaction: {e}"),
            })?;
        self.set_tenant(&mut tx)
            .await
            .map_err(|e| StoreError::Internal {
                message: format!("set tenant: {e}"),
            })?;

        sqlx::query(
            "UPDATE effect_intents
             SET claimed_by = NULL, claimed_until = NULL
             WHERE id = $1 AND claimed_by = $2",
        )
        .bind(&id_str)
        .bind(&worker_str)
        .execute(&mut *tx)
        .await
        .map_err(|e| map_pg_err(&e))?;

        tx.commit().await.map_err(|e| StoreError::Internal {
            message: format!("commit: {e}"),
        })?;
        Ok(())
    }

    async fn get_intent(&self, intent_id: EffectId) -> Result<StoredIntent, StoreError> {
        let id_str = intent_id.to_string();

        let mut tx = self
            .pool()
            .begin()
            .await
            .map_err(|e| StoreError::ConnectionError {
                message: format!("begin transaction: {e}"),
            })?;
        self.set_tenant(&mut tx)
            .await
            .map_err(|e| StoreError::Internal {
                message: format!("set tenant: {e}"),
            })?;

        let row = sqlx::query(&format!(
            "SELECT {INTENT_COLS} FROM effect_intents WHERE id = $1"
        ))
        .bind(&id_str)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| map_pg_err(&e))?
        .ok_or_else(|| StoreError::NotFound {
            resource_type: "EffectIntent",
            id: id_str.clone(),
        })?;

        row_to_intent(&row)
    }

    async fn get_by_run(&self, run_id: RunId) -> Result<Vec<StoredIntent>, StoreError> {
        let run_str = run_id.to_string();

        let mut tx = self
            .pool()
            .begin()
            .await
            .map_err(|e| StoreError::ConnectionError {
                message: format!("begin transaction: {e}"),
            })?;
        self.set_tenant(&mut tx)
            .await
            .map_err(|e| StoreError::Internal {
                message: format!("set tenant: {e}"),
            })?;

        let rows = sqlx::query(&format!(
            "SELECT {INTENT_COLS} FROM effect_intents WHERE run_id = $1 ORDER BY created_at ASC"
        ))
        .bind(&run_str)
        .fetch_all(&mut *tx)
        .await
        .map_err(|e| map_pg_err(&e))?;

        rows.iter().map(row_to_intent).collect()
    }

    async fn get_by_idempotency_key(
        &self,
        key: &str,
        run_id: RunId,
    ) -> Result<Option<StoredIntent>, StoreError> {
        let run_str = run_id.to_string();

        let mut tx = self
            .pool()
            .begin()
            .await
            .map_err(|e| StoreError::ConnectionError {
                message: format!("begin transaction: {e}"),
            })?;
        self.set_tenant(&mut tx)
            .await
            .map_err(|e| StoreError::Internal {
                message: format!("set tenant: {e}"),
            })?;

        let maybe_row = sqlx::query(&format!(
            "SELECT {INTENT_COLS} FROM effect_intents WHERE run_id = $1 AND idempotency_key = $2"
        ))
        .bind(&run_str)
        .bind(key)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| map_pg_err(&e))?;

        match maybe_row {
            Some(row) => Ok(Some(row_to_intent(&row)?)),
            None => Ok(None),
        }
    }

    async fn expired_leases(&self, cutoff: Timestamp) -> Result<Vec<StoredIntent>, StoreError> {
        let cutoff_dt = cutoff;

        let mut tx = self
            .pool()
            .begin()
            .await
            .map_err(|e| StoreError::ConnectionError {
                message: format!("begin transaction: {e}"),
            })?;
        self.set_tenant(&mut tx)
            .await
            .map_err(|e| StoreError::Internal {
                message: format!("set tenant: {e}"),
            })?;

        let rows = sqlx::query(&format!(
            "SELECT {INTENT_COLS} FROM effect_intents
             WHERE claimed_by IS NOT NULL
               AND claimed_by NOT IN ('resolved', 'failed', 'permanently_failed')
               AND claimed_until < $1"
        ))
        .bind(cutoff_dt)
        .fetch_all(&mut *tx)
        .await
        .map_err(|e| map_pg_err(&e))?;

        rows.iter().map(row_to_intent).collect()
    }

    async fn update_intent_state(
        &self,
        intent_id: EffectId,
        new_state: &str,
    ) -> Result<StoredIntent, StoreError> {
        let id_str = intent_id.to_string();
        let claimed_by_value = match new_state {
            "pending" => None,
            "resolved" => Some("resolved".to_string()),
            "failed" => Some("failed".to_string()),
            "permanently_failed" => Some("permanently_failed".to_string()),
            other => Some(other.to_string()),
        };

        let mut tx = self
            .pool()
            .begin()
            .await
            .map_err(|e| StoreError::ConnectionError {
                message: format!("begin transaction: {e}"),
            })?;
        self.set_tenant(&mut tx)
            .await
            .map_err(|e| StoreError::Internal {
                message: format!("set tenant: {e}"),
            })?;

        let result = sqlx::query(
            "UPDATE effect_intents SET claimed_by = $1, claimed_until = NULL WHERE id = $2",
        )
        .bind(&claimed_by_value)
        .bind(&id_str)
        .execute(&mut *tx)
        .await
        .map_err(|e| map_pg_err(&e))?;

        if result.rows_affected() == 0 {
            return Err(StoreError::NotFound {
                resource_type: "EffectIntent",
                id: id_str,
            });
        }

        let row = sqlx::query(&format!(
            "SELECT {INTENT_COLS} FROM effect_intents WHERE id = $1"
        ))
        .bind(&id_str)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| map_pg_err(&e))?;

        let intent = row_to_intent(&row)?;

        tx.commit().await.map_err(|e| StoreError::Internal {
            message: format!("commit: {e}"),
        })?;
        Ok(intent)
    }

    async fn record_attempt_start(
        &self,
        attempt_id: EffectAttemptId,
        intent_id: EffectId,
        worker_id: WorkerId,
        payload: serde_json::Value,
    ) -> Result<(), StoreError> {
        let tenant = self.tenant_id().to_string();

        let mut tx = self
            .pool()
            .begin()
            .await
            .map_err(|e| StoreError::ConnectionError {
                message: format!("begin transaction: {e}"),
            })?;
        self.set_tenant(&mut tx)
            .await
            .map_err(|e| StoreError::Internal {
                message: format!("set tenant: {e}"),
            })?;

        sqlx::query(
            "INSERT INTO effect_attempts (id, tenant_id, intent_id, worker_id, payload)
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(attempt_id.to_string())
        .bind(&tenant)
        .bind(intent_id.to_string())
        .bind(worker_id.to_string())
        .bind(payload)
        .execute(&mut *tx)
        .await
        .map_err(|e| map_pg_err(&e))?;

        tx.commit().await.map_err(|e| StoreError::Internal {
            message: format!("commit: {e}"),
        })?;
        Ok(())
    }

    async fn record_outcome(&self, outcome: StoredOutcome) -> Result<(), StoreError> {
        let tenant = self.tenant_id().to_string();
        let id_str = outcome.id.to_string();
        let intent_id_str = outcome.intent_id.to_string();
        let attempt_id_str = outcome.attempt_id.to_string();
        let run_id_str = outcome.run_id.to_string();

        let mut tx = self
            .pool()
            .begin()
            .await
            .map_err(|e| StoreError::ConnectionError {
                message: format!("begin transaction: {e}"),
            })?;
        self.set_tenant(&mut tx)
            .await
            .map_err(|e| StoreError::Internal {
                message: format!("set tenant: {e}"),
            })?;

        sqlx::query(
            "INSERT INTO effect_outcomes (id, tenant_id, intent_id, attempt_id, run_id, consumed, payload, observed_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(&id_str)
        .bind(&tenant)
        .bind(&intent_id_str)
        .bind(&attempt_id_str)
        .bind(&run_id_str)
        .bind(outcome.consumed)
        .bind(&outcome.payload)
        .bind(outcome.observed_at)
        .execute(&mut *tx)
        .await
        .map_err(|e| {
            if matches!(&e, sqlx::Error::Database(db_err) if db_err.is_unique_violation()) {
                StoreError::Conflict {
                    resource_type: "EffectOutcome",
                    id: id_str.clone(),
                }
            } else {
                map_pg_err(&e)
            }
        })?;

        // Transition intent to resolved.
        sqlx::query(
            "UPDATE effect_intents SET claimed_by = 'resolved', claimed_until = NULL WHERE id = $1",
        )
        .bind(&intent_id_str)
        .execute(&mut *tx)
        .await
        .map_err(|e| map_pg_err(&e))?;

        tx.commit().await.map_err(|e| StoreError::Internal {
            message: format!("commit: {e}"),
        })?;
        Ok(())
    }

    async fn unconsumed_outcomes(&self, run_id: RunId) -> Result<Vec<StoredOutcome>, StoreError> {
        let run_str = run_id.to_string();

        let mut tx = self
            .pool()
            .begin()
            .await
            .map_err(|e| StoreError::ConnectionError {
                message: format!("begin transaction: {e}"),
            })?;
        self.set_tenant(&mut tx)
            .await
            .map_err(|e| StoreError::Internal {
                message: format!("set tenant: {e}"),
            })?;

        let rows = sqlx::query(
            "SELECT id, intent_id, attempt_id, run_id, consumed, payload, observed_at
             FROM effect_outcomes
             WHERE run_id = $1 AND consumed = FALSE
             ORDER BY observed_at ASC",
        )
        .bind(&run_str)
        .fetch_all(&mut *tx)
        .await
        .map_err(|e| map_pg_err(&e))?;

        rows.iter()
            .map(|row| {
                let id: String = row.get("id");
                let intent_id: String = row.get("intent_id");
                let attempt_id: String = row.get("attempt_id");
                let run_id: String = row.get("run_id");
                let observed_at: chrono::DateTime<Utc> = row.get("observed_at");

                Ok(StoredOutcome {
                    id: parse_id(&id, "EffectOutcomeId")?,
                    intent_id: parse_id(&intent_id, "EffectId")?,
                    attempt_id: parse_id(&attempt_id, "EffectAttemptId")?,
                    run_id: parse_id(&run_id, "RunId")?,
                    consumed: row.get("consumed"),
                    payload: row.get("payload"),
                    observed_at: parse_ts(observed_at),
                })
            })
            .collect()
    }

    async fn mark_outcomes_consumed(
        &self,
        outcome_ids: &[EffectOutcomeId],
    ) -> Result<(), StoreError> {
        if outcome_ids.is_empty() {
            return Ok(());
        }

        let mut tx = self
            .pool()
            .begin()
            .await
            .map_err(|e| StoreError::ConnectionError {
                message: format!("begin transaction: {e}"),
            })?;
        self.set_tenant(&mut tx)
            .await
            .map_err(|e| StoreError::Internal {
                message: format!("set tenant: {e}"),
            })?;

        let ids: Vec<String> = outcome_ids.iter().map(ToString::to_string).collect();
        sqlx::query("UPDATE effect_outcomes SET consumed = TRUE WHERE id = ANY($1)")
            .bind(&ids)
            .execute(&mut *tx)
            .await
            .map_err(|e| map_pg_err(&e))?;

        tx.commit().await.map_err(|e| StoreError::Internal {
            message: format!("commit: {e}"),
        })?;
        Ok(())
    }
}
