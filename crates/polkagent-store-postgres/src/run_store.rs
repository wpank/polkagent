use async_trait::async_trait;
use chrono::Utc;
use polkagent_core::{RunId, Timestamp, TurnId};
use polkagent_store_trait::{RunStatus, RunStore, RunSummary, StoreError};
use sqlx::Row;

use crate::pool::PgPool;

fn parse_ts(dt: chrono::DateTime<Utc>) -> Timestamp {
    dt
}

fn map_pg_err(e: sqlx::Error) -> StoreError {
    match &e {
        sqlx::Error::Database(db_err) => {
            if db_err.is_unique_violation() {
                StoreError::Conflict {
                    resource_type: "Run",
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

fn map_pg_err_with_id(e: sqlx::Error, id: &str) -> StoreError {
    match &e {
        sqlx::Error::Database(db_err) => {
            if db_err.is_unique_violation() {
                StoreError::Conflict {
                    resource_type: "Run",
                    id: id.to_string(),
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

#[async_trait]
impl RunStore for PgPool {
    async fn create(
        &self,
        run_id: RunId,
        agent_id: &str,
        status: RunStatus,
    ) -> Result<(), StoreError> {
        let id_str = run_id.to_string();
        let now = Utc::now();
        let tenant = self.tenant_id().to_string();

        let mut tx = self.pool().begin().await.map_err(|e| StoreError::ConnectionError {
            message: format!("begin transaction: {e}"),
        })?;
        self.set_tenant(&mut *tx).await.map_err(|e| StoreError::Internal {
            message: format!("set tenant: {e}"),
        })?;

        sqlx::query(
            "INSERT INTO runs (id, tenant_id, agent_id, state, params_json, created_at, updated_at)
             VALUES ($1, $2, $3, $4, '{}', $5, $6)",
        )
        .bind(&id_str)
        .bind(&tenant)
        .bind(agent_id)
        .bind(status.as_str())
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(|e| map_pg_err_with_id(e, &id_str))?;

        tx.commit().await.map_err(|e| StoreError::Internal {
            message: format!("commit: {e}"),
        })?;
        Ok(())
    }

    async fn get(&self, run_id: RunId) -> Result<RunSummary, StoreError> {
        let id_str = run_id.to_string();

        let mut tx = self.pool().begin().await.map_err(|e| StoreError::ConnectionError {
            message: format!("begin transaction: {e}"),
        })?;
        self.set_tenant(&mut *tx).await.map_err(|e| StoreError::Internal {
            message: format!("set tenant: {e}"),
        })?;

        let row = sqlx::query(
            "SELECT id, agent_id, state, created_at, started_at, completed_at
             FROM runs WHERE id = $1",
        )
        .bind(&id_str)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| map_pg_err(e))?
        .ok_or_else(|| StoreError::NotFound {
            resource_type: "Run",
            id: id_str.clone(),
        })?;

        let id: String = row.get("id");
        let run_id = id.parse::<RunId>().map_err(|e| StoreError::Internal {
            message: format!("invalid run id: {e}"),
        })?;

        Ok(RunSummary {
            id: run_id,
            agent_id: row.get("agent_id"),
            status: RunStatus::new(row.get::<String, _>("state")),
            created_at: parse_ts(row.get("created_at")),
            started_at: row.get::<Option<chrono::DateTime<Utc>>, _>("started_at").map(parse_ts),
            completed_at: row.get::<Option<chrono::DateTime<Utc>>, _>("completed_at").map(parse_ts),
        })
    }

    async fn update_state(
        &self,
        run_id: RunId,
        new_status: RunStatus,
    ) -> Result<(), StoreError> {
        let id_str = run_id.to_string();
        let status_str = new_status.as_str().to_string();
        let now = Utc::now();

        let is_terminal = matches!(
            status_str.as_str(),
            "completed" | "failed" | "cancelled" | "timed_out"
        );
        let completed_at: Option<chrono::DateTime<Utc>> = if is_terminal { Some(now) } else { None };
        let started_at: Option<chrono::DateTime<Utc>> = if status_str == "running" { Some(now) } else { None };

        let mut tx = self.pool().begin().await.map_err(|e| StoreError::ConnectionError {
            message: format!("begin transaction: {e}"),
        })?;
        self.set_tenant(&mut *tx).await.map_err(|e| StoreError::Internal {
            message: format!("set tenant: {e}"),
        })?;

        let result = sqlx::query(
            "UPDATE runs SET state = $1, updated_at = $2,
                    started_at = COALESCE(started_at, $3),
                    completed_at = COALESCE($4, completed_at)
             WHERE id = $5",
        )
        .bind(&status_str)
        .bind(now)
        .bind(started_at)
        .bind(completed_at)
        .bind(&id_str)
        .execute(&mut *tx)
        .await
        .map_err(|e| map_pg_err(e))?;

        if result.rows_affected() == 0 {
            return Err(StoreError::NotFound {
                resource_type: "Run",
                id: id_str,
            });
        }

        tx.commit().await.map_err(|e| StoreError::Internal {
            message: format!("commit: {e}"),
        })?;
        Ok(())
    }

    async fn list_by_agent(
        &self,
        agent_id: &str,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<RunSummary>, StoreError> {
        let mut tx = self.pool().begin().await.map_err(|e| StoreError::ConnectionError {
            message: format!("begin transaction: {e}"),
        })?;
        self.set_tenant(&mut *tx).await.map_err(|e| StoreError::Internal {
            message: format!("set tenant: {e}"),
        })?;

        let rows = sqlx::query(
            "SELECT id, agent_id, state, created_at, started_at, completed_at
             FROM runs
             WHERE agent_id = $1
             ORDER BY created_at DESC
             LIMIT $2 OFFSET $3",
        )
        .bind(agent_id)
        .bind(limit as i64)
        .bind(offset as i64)
        .fetch_all(&mut *tx)
        .await
        .map_err(|e| map_pg_err(e))?;

        rows.iter()
            .map(|row| {
                let id: String = row.get("id");
                let run_id = id.parse::<RunId>().map_err(|e| StoreError::Internal {
                    message: format!("invalid run id: {e}"),
                })?;
                Ok(RunSummary {
                    id: run_id,
                    agent_id: row.get("agent_id"),
                    status: RunStatus::new(row.get::<String, _>("state")),
                    created_at: parse_ts(row.get("created_at")),
                    started_at: row.get::<Option<chrono::DateTime<Utc>>, _>("started_at").map(parse_ts),
                    completed_at: row.get::<Option<chrono::DateTime<Utc>>, _>("completed_at").map(parse_ts),
                })
            })
            .collect()
    }

    async fn list_by_state(
        &self,
        status: RunStatus,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<RunSummary>, StoreError> {
        let mut tx = self.pool().begin().await.map_err(|e| StoreError::ConnectionError {
            message: format!("begin transaction: {e}"),
        })?;
        self.set_tenant(&mut *tx).await.map_err(|e| StoreError::Internal {
            message: format!("set tenant: {e}"),
        })?;

        let rows = sqlx::query(
            "SELECT id, agent_id, state, created_at, started_at, completed_at
             FROM runs
             WHERE state = $1
             ORDER BY created_at DESC
             LIMIT $2 OFFSET $3",
        )
        .bind(status.as_str())
        .bind(limit as i64)
        .bind(offset as i64)
        .fetch_all(&mut *tx)
        .await
        .map_err(|e| map_pg_err(e))?;

        rows.iter()
            .map(|row| {
                let id: String = row.get("id");
                let run_id = id.parse::<RunId>().map_err(|e| StoreError::Internal {
                    message: format!("invalid run id: {e}"),
                })?;
                Ok(RunSummary {
                    id: run_id,
                    agent_id: row.get("agent_id"),
                    status: RunStatus::new(row.get::<String, _>("state")),
                    created_at: parse_ts(row.get("created_at")),
                    started_at: row.get::<Option<chrono::DateTime<Utc>>, _>("started_at").map(parse_ts),
                    completed_at: row.get::<Option<chrono::DateTime<Utc>>, _>("completed_at").map(parse_ts),
                })
            })
            .collect()
    }

    async fn insert_turn(
        &self,
        turn_id: TurnId,
        run_id: RunId,
        sequence: u32,
        role: &str,
        started_at: &str,
        completed_at: Option<&str>,
        input_tokens: u32,
        output_tokens: u32,
    ) -> Result<(), StoreError> {
        let turn_id_str = turn_id.to_string();
        let run_id_str = run_id.to_string();
        let tenant = self.tenant_id().to_string();

        let started_at_dt = chrono::DateTime::parse_from_rfc3339(started_at)
            .map(|dt| dt.with_timezone(&Utc))
            .map_err(|e| StoreError::Internal {
                message: format!("invalid started_at timestamp: {e}"),
            })?;
        let completed_at_dt = completed_at
            .map(|s| {
                chrono::DateTime::parse_from_rfc3339(s)
                    .map(|dt| dt.with_timezone(&Utc))
                    .map_err(|e| StoreError::Internal {
                        message: format!("invalid completed_at timestamp: {e}"),
                    })
            })
            .transpose()?;

        let mut tx = self.pool().begin().await.map_err(|e| StoreError::ConnectionError {
            message: format!("begin transaction: {e}"),
        })?;
        self.set_tenant(&mut *tx).await.map_err(|e| StoreError::Internal {
            message: format!("set tenant: {e}"),
        })?;

        sqlx::query(
            "INSERT INTO turns (id, tenant_id, run_id, sequence, role, started_at, completed_at, input_tokens, output_tokens)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(&turn_id_str)
        .bind(&tenant)
        .bind(&run_id_str)
        .bind(sequence as i32)
        .bind(role)
        .bind(started_at_dt)
        .bind(completed_at_dt)
        .bind(input_tokens as i32)
        .bind(output_tokens as i32)
        .execute(&mut *tx)
        .await
        .map_err(|e| map_pg_err_with_id(e, &turn_id_str))?;

        tx.commit().await.map_err(|e| StoreError::Internal {
            message: format!("commit: {e}"),
        })?;
        Ok(())
    }
}
