use async_trait::async_trait;
use chrono::Utc;
use polkagent_core::{ArtifactId, RunId, Timestamp};
use polkagent_store_trait::{ArtifactStore, ArtifactSummary, StoreError};
use sqlx::Row;

use crate::pool::PgPool;

fn map_pg_err(e: sqlx::Error) -> StoreError {
    StoreError::Internal {
        message: format!("postgres error: {e}"),
    }
}

fn parse_ts(dt: chrono::DateTime<Utc>) -> Timestamp {
    dt
}

fn parse_artifact_id(s: &str) -> Result<ArtifactId, StoreError> {
    s.parse::<ArtifactId>().map_err(|e| StoreError::Internal {
        message: format!("invalid artifact id '{s}': {e}"),
    })
}

fn parse_run_id(s: &str) -> Result<RunId, StoreError> {
    s.parse::<RunId>().map_err(|e| StoreError::Internal {
        message: format!("invalid run id '{s}': {e}"),
    })
}

fn row_to_summary(row: &sqlx::postgres::PgRow) -> Result<ArtifactSummary, StoreError> {
    let id: String = row.get("id");
    let run_id: Option<String> = row.get("run_id");
    let created_at: chrono::DateTime<Utc> = row.get("created_at");

    Ok(ArtifactSummary {
        id: parse_artifact_id(&id)?,
        kind: row.get("kind"),
        algorithm: row.get("algorithm"),
        digest_hex: row.get("digest_hex"),
        classification: row.get("classification"),
        run_id: run_id.as_deref().map(parse_run_id).transpose()?,
        created_at: parse_ts(created_at),
    })
}

#[async_trait]
impl ArtifactStore for PgPool {
    async fn store(
        &self,
        artifact_id: ArtifactId,
        run_id: Option<RunId>,
        kind: &str,
        algorithm: &str,
        digest_hex: &str,
        classification: &str,
        body: &[u8],
    ) -> Result<(), StoreError> {
        let id_str = artifact_id.to_string();
        let run_id_str = run_id.map(|r| r.to_string());
        let tenant = self.tenant_id().to_string();
        let now = Utc::now();
        let size_bytes = body.len() as i64;
        let body = body.to_vec();

        let mut tx = self
            .pool()
            .begin()
            .await
            .map_err(|e| StoreError::ConnectionError {
                message: format!("begin transaction: {e}"),
            })?;
        self.set_tenant(&mut *tx)
            .await
            .map_err(|e| StoreError::Internal {
                message: format!("set tenant: {e}"),
            })?;

        // Upsert artifact metadata (idempotent).
        sqlx::query(
            "INSERT INTO artifacts (id, tenant_id, run_id, kind, algorithm, digest_hex, classification, size_bytes, created_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
             ON CONFLICT (id) DO NOTHING",
        )
        .bind(&id_str)
        .bind(&tenant)
        .bind(&run_id_str)
        .bind(kind)
        .bind(algorithm)
        .bind(digest_hex)
        .bind(classification)
        .bind(size_bytes)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(map_pg_err)?;

        // Upsert body (content-addressed, deduped).
        sqlx::query(
            "INSERT INTO artifact_bodies (digest_hex, body)
             VALUES ($1, $2)
             ON CONFLICT (digest_hex) DO NOTHING",
        )
        .bind(digest_hex)
        .bind(&body)
        .execute(&mut *tx)
        .await
        .map_err(map_pg_err)?;

        tx.commit().await.map_err(|e| StoreError::Internal {
            message: format!("commit: {e}"),
        })?;
        Ok(())
    }

    async fn get(&self, id: ArtifactId) -> Result<ArtifactSummary, StoreError> {
        let id_str = id.to_string();

        let mut tx = self
            .pool()
            .begin()
            .await
            .map_err(|e| StoreError::ConnectionError {
                message: format!("begin transaction: {e}"),
            })?;
        self.set_tenant(&mut *tx)
            .await
            .map_err(|e| StoreError::Internal {
                message: format!("set tenant: {e}"),
            })?;

        let row = sqlx::query(
            "SELECT id, run_id, kind, algorithm, digest_hex, classification, created_at
             FROM artifacts WHERE id = $1",
        )
        .bind(&id_str)
        .fetch_optional(&mut *tx)
        .await
        .map_err(map_pg_err)?
        .ok_or_else(|| StoreError::NotFound {
            resource_type: "Artifact",
            id: id_str,
        })?;

        row_to_summary(&row)
    }

    async fn get_body(&self, id: ArtifactId) -> Result<Vec<u8>, StoreError> {
        let id_str = id.to_string();

        let mut tx = self
            .pool()
            .begin()
            .await
            .map_err(|e| StoreError::ConnectionError {
                message: format!("begin transaction: {e}"),
            })?;
        self.set_tenant(&mut *tx)
            .await
            .map_err(|e| StoreError::Internal {
                message: format!("set tenant: {e}"),
            })?;

        // Get the digest for this artifact.
        let digest_hex: String =
            sqlx::query_scalar("SELECT digest_hex FROM artifacts WHERE id = $1")
                .bind(&id_str)
                .fetch_optional(&mut *tx)
                .await
                .map_err(map_pg_err)?
                .ok_or_else(|| StoreError::NotFound {
                    resource_type: "Artifact",
                    id: id_str.clone(),
                })?;

        // Fetch body by digest.
        let body: Vec<u8> =
            sqlx::query_scalar("SELECT body FROM artifact_bodies WHERE digest_hex = $1")
                .bind(&digest_hex)
                .fetch_optional(&mut *tx)
                .await
                .map_err(map_pg_err)?
                .ok_or_else(|| StoreError::NotFound {
                    resource_type: "ArtifactBody",
                    id: id_str.clone(),
                })?;

        // Verify BLAKE3 digest.
        let actual_hash = blake3::hash(&body);
        if actual_hash.to_hex().as_str() != digest_hex {
            return Err(StoreError::IntegrityError {
                resource_type: "Artifact",
                id: id_str,
            });
        }

        Ok(body)
    }

    async fn verify(&self, id: ArtifactId) -> Result<bool, StoreError> {
        let id_str = id.to_string();

        let mut tx = self
            .pool()
            .begin()
            .await
            .map_err(|e| StoreError::ConnectionError {
                message: format!("begin transaction: {e}"),
            })?;
        self.set_tenant(&mut *tx)
            .await
            .map_err(|e| StoreError::Internal {
                message: format!("set tenant: {e}"),
            })?;

        // Get digest.
        let maybe_digest: Option<String> =
            sqlx::query_scalar("SELECT digest_hex FROM artifacts WHERE id = $1")
                .bind(&id_str)
                .fetch_optional(&mut *tx)
                .await
                .map_err(map_pg_err)?;

        let digest_hex = match maybe_digest {
            Some(d) => d,
            None => return Ok(false),
        };

        // Fetch body.
        let maybe_body: Option<Vec<u8>> =
            sqlx::query_scalar("SELECT body FROM artifact_bodies WHERE digest_hex = $1")
                .bind(&digest_hex)
                .fetch_optional(&mut *tx)
                .await
                .map_err(map_pg_err)?;

        match maybe_body {
            Some(body) => {
                let actual_hash = blake3::hash(&body);
                Ok(actual_hash.to_hex().as_str() == digest_hex)
            }
            None => Ok(false),
        }
    }

    async fn list_for_run(&self, run_id: RunId) -> Result<Vec<ArtifactSummary>, StoreError> {
        let run_str = run_id.to_string();

        let mut tx = self
            .pool()
            .begin()
            .await
            .map_err(|e| StoreError::ConnectionError {
                message: format!("begin transaction: {e}"),
            })?;
        self.set_tenant(&mut *tx)
            .await
            .map_err(|e| StoreError::Internal {
                message: format!("set tenant: {e}"),
            })?;

        let rows = sqlx::query(
            "SELECT id, run_id, kind, algorithm, digest_hex, classification, created_at
             FROM artifacts
             WHERE run_id = $1
             ORDER BY created_at ASC",
        )
        .bind(&run_str)
        .fetch_all(&mut *tx)
        .await
        .map_err(map_pg_err)?;

        rows.iter().map(row_to_summary).collect()
    }
}
