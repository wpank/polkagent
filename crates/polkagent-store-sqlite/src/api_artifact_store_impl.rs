//! Strict adapter for the public [`polkagent_store_trait::ArtifactStore`] port.
//!
//! Polkagent's richer artifact domain store and the public persistence port
//! intentionally expose different record shapes. This adapter projects the
//! same `SQLite` rows without defaulting or discarding algorithm,
//! classification, integrity, or lineage information.

use std::collections::{HashMap, HashSet, VecDeque};

use async_trait::async_trait;
use chrono::DateTime;
use polkagent_core::artifact::ArtifactKind;
use polkagent_core::{ArtifactId, RunId};
use polkagent_store_trait::{ArtifactStore, ArtifactSummary, StoreError};
use rusqlite::OptionalExtension;

use crate::SqlitePool;

/// API artifact adapter over an existing migrated [`SqlitePool`].
#[derive(Debug, Clone)]
pub struct SqliteApiArtifactStore {
    pool: SqlitePool,
}

impl SqliteApiArtifactStore {
    /// Construct an adapter over `pool`.
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[derive(Debug, Clone)]
struct RawArtifact {
    id: String,
    run_id: Option<String>,
    kind: String,
    algorithm: String,
    digest_hex: String,
    classification: String,
    size_bytes: i64,
    metadata_json: String,
    created_at: String,
}

impl RawArtifact {
    fn into_summary(self) -> Result<ArtifactSummary, StoreError> {
        let id = parse_artifact_id(&self.id)?;
        let run_id = self.run_id.as_deref().map(parse_run_id).transpose()?;
        validate_kind(&self.kind, id)?;
        validate_algorithm(&self.algorithm)?;
        validate_digest(&self.digest_hex, id)?;
        validate_stored_classification(&self.classification, id)?;
        if self.size_bytes < 0 {
            return Err(invalid_projection(format!(
                "artifact {id} has negative size {}",
                self.size_bytes
            )));
        }
        serde_json::from_str::<HashMap<String, String>>(&self.metadata_json).map_err(|error| {
            invalid_projection(format!("artifact {id} metadata is invalid: {error}"))
        })?;
        let created_at = DateTime::parse_from_rfc3339(&self.created_at)
            .map_err(|error| {
                invalid_projection(format!("artifact {id} timestamp is invalid: {error}"))
            })?
            .to_utc();

        Ok(ArtifactSummary {
            id,
            kind: self.kind,
            algorithm: self.algorithm,
            digest_hex: self.digest_hex,
            classification: self.classification,
            run_id,
            created_at,
        })
    }
}

#[async_trait]
impl ArtifactStore for SqliteApiArtifactStore {
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
        validate_kind(kind, artifact_id)?;
        validate_algorithm(algorithm)?;
        validate_digest(digest_hex, artifact_id)?;
        validate_input_classification(classification)?;
        if blake3::hash(body).to_hex().as_str() != digest_hex {
            return Err(StoreError::IntegrityError {
                resource_type: "Artifact",
                id: artifact_id.to_string(),
            });
        }
        let size_bytes = i64::try_from(body.len()).map_err(|_| StoreError::Internal {
            message: "artifact body length exceeds SQLite INTEGER capacity".to_owned(),
        })?;

        let pool = self.pool.clone();
        let id_text = artifact_id.to_string();
        let run_id_text = run_id.map(|id| id.to_string());
        let kind = kind.to_owned();
        let algorithm = algorithm.to_owned();
        let digest_hex = digest_hex.to_owned();
        let classification = classification.to_owned();
        let body = body.to_vec();
        tokio::task::spawn_blocking(move || {
            let mut writer = pool.writer();
            let transaction = writer
                .transaction()
                .map_err(|error| map_sqlite(&error, "begin artifact transaction"))?;
            let existing = load_raw_optional(&transaction, &id_text)?;
            if let Some(existing) = existing {
                existing.clone().into_summary()?;
                let matches = existing.run_id == run_id_text
                    && existing.kind == kind
                    && existing.algorithm == algorithm
                    && existing.digest_hex == digest_hex
                    && existing.classification == classification
                    && existing.size_bytes == size_bytes;
                if !matches {
                    return Err(StoreError::Conflict {
                        resource_type: "Artifact",
                        id: id_text,
                    });
                }
            } else {
                transaction
                    .execute(
                        "INSERT INTO artifacts
                            (id, run_id, kind, algorithm, digest_hex,
                             classification, size_bytes, metadata_json, created_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, '{}', ?8)",
                        rusqlite::params![
                            id_text,
                            run_id_text,
                            kind,
                            algorithm,
                            digest_hex,
                            classification,
                            size_bytes,
                            chrono::Utc::now().to_rfc3339(),
                        ],
                    )
                    .map_err(|error| map_artifact_insert(&error, &id_text))?;
            }

            let existing_body: Option<Vec<u8>> = transaction
                .query_row(
                    "SELECT body FROM artifact_bodies WHERE digest_hex = ?1",
                    [&digest_hex],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|error| map_sqlite(&error, "read content-addressed artifact body"))?;
            if let Some(existing_body) = existing_body {
                if blake3::hash(&existing_body).to_hex().as_str() != digest_hex {
                    return Err(StoreError::IntegrityError {
                        resource_type: "Artifact",
                        id: artifact_id.to_string(),
                    });
                }
            } else {
                transaction
                    .execute(
                        "INSERT INTO artifact_bodies (digest_hex, body) VALUES (?1, ?2)",
                        rusqlite::params![digest_hex, body],
                    )
                    .map_err(|error| map_sqlite(&error, "insert artifact body"))?;
            }
            transaction
                .commit()
                .map_err(|error| map_sqlite(&error, "commit artifact transaction"))
        })
        .await
        .map_err(|error| StoreError::Internal {
            message: format!("artifact write task failed: {error}"),
        })?
    }

    async fn get(&self, id: ArtifactId) -> Result<ArtifactSummary, StoreError> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || load_raw(&pool.writer(), id)?.into_summary())
            .await
            .map_err(|error| StoreError::Internal {
                message: format!("artifact read task failed: {error}"),
            })?
    }

    async fn get_body(&self, id: ArtifactId) -> Result<Vec<u8>, StoreError> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();
            let raw = load_raw(&writer, id)?;
            let expected_size = usize::try_from(raw.size_bytes).map_err(|_| {
                invalid_projection(format!("artifact {id} has invalid size {}", raw.size_bytes))
            })?;
            let summary = raw.into_summary()?;
            let body: Vec<u8> = writer
                .query_row(
                    "SELECT body FROM artifact_bodies WHERE digest_hex = ?1",
                    [&summary.digest_hex],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|error| map_sqlite(&error, "read artifact body"))?
                .ok_or_else(|| StoreError::NotFound {
                    resource_type: "ArtifactBody",
                    id: id.to_string(),
                })?;
            if body.len() != expected_size
                || blake3::hash(&body).to_hex().as_str() != summary.digest_hex
            {
                return Err(StoreError::IntegrityError {
                    resource_type: "Artifact",
                    id: id.to_string(),
                });
            }
            Ok(body)
        })
        .await
        .map_err(|error| StoreError::Internal {
            message: format!("artifact body read task failed: {error}"),
        })?
    }

    async fn verify(&self, id: ArtifactId) -> Result<bool, StoreError> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();
            let Some(raw) = load_raw_optional(&writer, &id.to_string())? else {
                return Ok(false);
            };
            let expected_size = usize::try_from(raw.size_bytes).map_err(|_| {
                invalid_projection(format!("artifact {id} has invalid size {}", raw.size_bytes))
            })?;
            let summary = raw.into_summary()?;
            let body: Option<Vec<u8>> = writer
                .query_row(
                    "SELECT body FROM artifact_bodies WHERE digest_hex = ?1",
                    [&summary.digest_hex],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|error| map_sqlite(&error, "verify artifact body"))?;
            Ok(body.is_some_and(|body| {
                body.len() == expected_size
                    && blake3::hash(&body).to_hex().as_str() == summary.digest_hex
            }))
        })
        .await
        .map_err(|error| StoreError::Internal {
            message: format!("artifact verify task failed: {error}"),
        })?
    }

    async fn list_for_run(&self, run_id: RunId) -> Result<Vec<ArtifactSummary>, StoreError> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();
            let mut statement = writer
                .prepare(
                    "SELECT id, run_id, kind, algorithm, digest_hex,
                            classification, size_bytes, metadata_json, created_at
                     FROM artifacts WHERE run_id = ?1
                     ORDER BY created_at ASC, id ASC",
                )
                .map_err(|error| map_sqlite(&error, "prepare artifact list"))?;
            let rows = statement
                .query_map([run_id.to_string()], row_to_raw)
                .map_err(|error| map_sqlite(&error, "query artifact list"))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| map_sqlite(&error, "project artifact list"))?;
            rows.into_iter().map(RawArtifact::into_summary).collect()
        })
        .await
        .map_err(|error| StoreError::Internal {
            message: format!("artifact list task failed: {error}"),
        })?
    }

    async fn add_lineage(
        &self,
        child_id: ArtifactId,
        parent_id: ArtifactId,
    ) -> Result<(), StoreError> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();
            load_raw(&writer, child_id)?.into_summary()?;
            load_raw(&writer, parent_id)?.into_summary()?;
            writer
                .execute(
                    "INSERT OR IGNORE INTO artifact_lineage (child_id, parent_id)
                     VALUES (?1, ?2)",
                    rusqlite::params![child_id.to_string(), parent_id.to_string()],
                )
                .map_err(|error| map_sqlite(&error, "insert artifact lineage"))?;
            Ok(())
        })
        .await
        .map_err(|error| StoreError::Internal {
            message: format!("artifact lineage write task failed: {error}"),
        })?
    }

    async fn get_lineage(&self, id: ArtifactId) -> Result<Vec<ArtifactId>, StoreError> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();
            load_raw(&writer, id)?.into_summary()?;

            let mut ancestors = Vec::new();
            let mut visited = HashSet::from([id]);
            let mut queue = VecDeque::from([id]);
            while let Some(current) = queue.pop_front() {
                let mut statement = writer
                    .prepare(
                        "SELECT parent_id FROM artifact_lineage
                         WHERE child_id = ?1 ORDER BY parent_id ASC",
                    )
                    .map_err(|error| map_sqlite(&error, "prepare artifact lineage query"))?;
                let parents = statement
                    .query_map([current.to_string()], |row| row.get::<_, String>(0))
                    .map_err(|error| map_sqlite(&error, "query artifact lineage"))?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|error| map_sqlite(&error, "project artifact lineage"))?;
                for parent_text in parents {
                    let parent = parse_artifact_id(&parent_text)?;
                    if visited.insert(parent) {
                        load_raw(&writer, parent)?.into_summary()?;
                        ancestors.push(parent);
                        queue.push_back(parent);
                    }
                }
            }
            Ok(ancestors)
        })
        .await
        .map_err(|error| StoreError::Internal {
            message: format!("artifact lineage read task failed: {error}"),
        })?
    }
}

fn row_to_raw(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawArtifact> {
    Ok(RawArtifact {
        id: row.get(0)?,
        run_id: row.get(1)?,
        kind: row.get(2)?,
        algorithm: row.get(3)?,
        digest_hex: row.get(4)?,
        classification: row.get(5)?,
        size_bytes: row.get(6)?,
        metadata_json: row.get(7)?,
        created_at: row.get(8)?,
    })
}

fn load_raw(connection: &rusqlite::Connection, id: ArtifactId) -> Result<RawArtifact, StoreError> {
    load_raw_optional(connection, &id.to_string())?.ok_or_else(|| StoreError::NotFound {
        resource_type: "Artifact",
        id: id.to_string(),
    })
}

fn load_raw_optional(
    connection: &rusqlite::Connection,
    id: &str,
) -> Result<Option<RawArtifact>, StoreError> {
    connection
        .query_row(
            "SELECT id, run_id, kind, algorithm, digest_hex,
                    classification, size_bytes, metadata_json, created_at
             FROM artifacts WHERE id = ?1",
            [id],
            row_to_raw,
        )
        .optional()
        .map_err(|error| map_sqlite(&error, "read artifact metadata"))
}

fn parse_artifact_id(value: &str) -> Result<ArtifactId, StoreError> {
    value.parse().map_err(|error| {
        invalid_projection(format!("invalid stored artifact id '{value}': {error}"))
    })
}

fn parse_run_id(value: &str) -> Result<RunId, StoreError> {
    value
        .parse()
        .map_err(|error| invalid_projection(format!("invalid stored run id '{value}': {error}")))
}

fn validate_kind(kind: &str, id: ArtifactId) -> Result<(), StoreError> {
    if kind.is_empty() {
        return Err(invalid_projection(format!(
            "artifact {id} has an empty kind"
        )));
    }
    if kind.trim_start().starts_with('{') {
        serde_json::from_str::<ArtifactKind>(kind).map_err(|error| {
            invalid_projection(format!("artifact {id} kind is invalid: {error}"))
        })?;
    }
    Ok(())
}

fn validate_algorithm(algorithm: &str) -> Result<(), StoreError> {
    if algorithm == "blake3" {
        Ok(())
    } else {
        Err(StoreError::Serialisation {
            message: format!("unsupported artifact digest algorithm '{algorithm}'"),
        })
    }
}

fn validate_digest(digest_hex: &str, id: ArtifactId) -> Result<(), StoreError> {
    blake3::Hash::from_hex(digest_hex)
        .map(|_| ())
        .map_err(|error| invalid_projection(format!("artifact {id} digest is invalid: {error}")))
}

fn validate_input_classification(classification: &str) -> Result<(), StoreError> {
    validate_classification(classification)?;
    if classification == "secret_forbidden" {
        return Err(StoreError::InvalidTransition {
            message: "secret_forbidden artifacts require a secret-specific store".to_owned(),
        });
    }
    Ok(())
}

fn validate_stored_classification(classification: &str, id: ArtifactId) -> Result<(), StoreError> {
    validate_classification(classification).map_err(|_| {
        invalid_projection(format!(
            "artifact {id} has invalid classification '{classification}'"
        ))
    })?;
    if classification == "secret_forbidden" {
        return Err(invalid_projection(format!(
            "artifact {id} is secret_forbidden and cannot be projected"
        )));
    }
    Ok(())
}

fn validate_classification(classification: &str) -> Result<(), StoreError> {
    match classification {
        "public" | "internal" | "private" | "sensitive" | "secret_forbidden" => Ok(()),
        _ => Err(StoreError::Serialisation {
            message: format!("invalid artifact classification '{classification}'"),
        }),
    }
}

fn invalid_projection(message: impl Into<String>) -> StoreError {
    StoreError::Serialisation {
        message: message.into(),
    }
}

fn map_artifact_insert(error: &rusqlite::Error, id: &str) -> StoreError {
    if error.sqlite_error_code() == Some(rusqlite::ffi::ErrorCode::ConstraintViolation) {
        StoreError::Conflict {
            resource_type: "Artifact",
            id: id.to_owned(),
        }
    } else {
        map_sqlite(error, "insert artifact metadata")
    }
}

fn map_sqlite(error: &rusqlite::Error, operation: &str) -> StoreError {
    use rusqlite::ffi::ErrorCode;

    match error.sqlite_error_code() {
        Some(
            ErrorCode::DatabaseBusy
            | ErrorCode::DatabaseLocked
            | ErrorCode::SystemIoFailure
            | ErrorCode::CannotOpen
            | ErrorCode::DiskFull,
        ) => StoreError::ConnectionError {
            message: format!("{operation}: {error}"),
        },
        _ => StoreError::Internal {
            message: format!("{operation}: {error}"),
        },
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "adapter contract fixtures fail immediately at controlled persistence boundaries"
)]
mod tests {
    use super::*;
    use polkagent_artifact::store::ArtifactStore as DomainArtifactStore;
    use polkagent_core::artifact::{Artifact, ArtifactKind, BlobRef};
    use polkagent_core::config::DataClassification;
    use polkagent_store_trait::ArtifactStore as ApiArtifactStore;

    fn test_pool() -> SqlitePool {
        let pool = SqlitePool::open_in_memory().expect("open in-memory SQLite");
        crate::migrations::migrate(&pool.writer()).expect("migrate SQLite");
        pool
    }

    fn insert_run(pool: &SqlitePool, run_id: RunId) {
        let agent_id = uuid::Uuid::now_v7().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        let writer = pool.writer();
        writer
            .execute(
                "INSERT INTO agents
                    (id, name, state, spec_json, created_at, updated_at)
                 VALUES (?1, ?2, 'active', '{}', ?3, ?3)",
                rusqlite::params![agent_id, format!("agent-{agent_id}"), now],
            )
            .expect("insert agent");
        writer
            .execute(
                "INSERT INTO runs
                    (id, agent_id, state, params_json, created_at, updated_at)
                 VALUES (?1, ?2, 'completed', '{}', ?3, ?3)",
                rusqlite::params![run_id.to_string(), agent_id, now],
            )
            .expect("insert run");
    }

    async fn store_api_artifact(
        store: &SqliteApiArtifactStore,
        id: ArtifactId,
        run_id: Option<RunId>,
        kind: &str,
        classification: &str,
        body: &[u8],
    ) {
        let digest = blake3::hash(body).to_hex().to_string();
        ApiArtifactStore::store(
            store,
            id,
            run_id,
            kind,
            "blake3",
            &digest,
            classification,
            body,
        )
        .await
        .expect("store API artifact");
    }

    #[tokio::test]
    async fn api_rows_roundtrip_without_loss_and_are_domain_readable() {
        let pool = test_pool();
        let store = SqliteApiArtifactStore::new(pool.clone());
        let id = ArtifactId::new();
        let body = b"private adapter artifact";
        store_api_artifact(&store, id, None, "urn:polkagent:test", "private", body).await;

        let summary = ApiArtifactStore::get(&store, id)
            .await
            .expect("read API summary");
        assert_eq!(summary.kind, "urn:polkagent:test");
        assert_eq!(summary.algorithm, "blake3");
        assert_eq!(summary.classification, "private");
        assert_eq!(
            ApiArtifactStore::get_body(&store, id)
                .await
                .expect("read API body"),
            body
        );

        let domain = DomainArtifactStore::get(&pool, id)
            .await
            .expect("read shared row through domain port");
        assert_eq!(
            domain.kind,
            ArtifactKind::Custom {
                type_uri: "urn:polkagent:test".to_owned()
            }
        );
        assert_eq!(domain.classification, DataClassification::Private);

        // An exact duplicate is idempotent; a conflicting projection is not.
        store_api_artifact(&store, id, None, "urn:polkagent:test", "private", body).await;
        let digest = blake3::hash(body).to_hex().to_string();
        let conflict = ApiArtifactStore::store(
            &store,
            id,
            None,
            "urn:polkagent:different",
            "blake3",
            &digest,
            "private",
            body,
        )
        .await
        .expect_err("different metadata for one ID must conflict");
        assert!(matches!(conflict, StoreError::Conflict { .. }));
    }

    #[tokio::test]
    async fn domain_rows_preserve_kind_and_classification_in_api_projection() {
        let pool = test_pool();
        let store = SqliteApiArtifactStore::new(pool.clone());
        let body = b"domain artifact";
        let kind = ArtifactKind::TestResult {
            suite: "runtime".to_owned(),
            passed: true,
        };
        let artifact = Artifact {
            id: ArtifactId::new(),
            run_id: None,
            step_id: None,
            kind: kind.clone(),
            blob_ref: BlobRef::from_bytes(body),
            classification: DataClassification::Sensitive,
            parents: Vec::new(),
            created_at: chrono::Utc::now(),
            metadata: HashMap::from([("source".to_owned(), "domain".to_owned())]),
        };
        DomainArtifactStore::store(&pool, &artifact, body)
            .await
            .expect("write domain artifact");

        let summary = ApiArtifactStore::get(&store, artifact.id)
            .await
            .expect("project domain artifact");
        assert_eq!(
            summary.kind,
            serde_json::to_string(&kind).expect("kind JSON")
        );
        assert_eq!(summary.classification, "sensitive");
        assert_eq!(summary.digest_hex, artifact.blob_ref.blake3_hex);
    }

    #[tokio::test]
    async fn validation_rejects_unsupported_or_unsafe_writes() {
        let store = SqliteApiArtifactStore::new(test_pool());
        let id = ArtifactId::new();
        let body = b"strict artifact";
        let digest = blake3::hash(body).to_hex().to_string();

        let algorithm =
            ApiArtifactStore::store(&store, id, None, "test", "sha256", &digest, "public", body)
                .await
                .expect_err("unsupported digest algorithm must fail");
        assert!(matches!(algorithm, StoreError::Serialisation { .. }));

        let bad_digest = "00".repeat(32);
        let integrity = ApiArtifactStore::store(
            &store,
            id,
            None,
            "test",
            "blake3",
            &bad_digest,
            "public",
            body,
        )
        .await
        .expect_err("mismatched body digest must fail");
        assert!(matches!(integrity, StoreError::IntegrityError { .. }));

        let secret = ApiArtifactStore::store(
            &store,
            id,
            None,
            "test",
            "blake3",
            &digest,
            "secret_forbidden",
            body,
        )
        .await
        .expect_err("secret artifact requires a different store");
        assert!(matches!(secret, StoreError::InvalidTransition { .. }));
    }

    #[tokio::test]
    async fn missing_corrupt_and_backend_failures_remain_distinct() {
        let pool = test_pool();
        let store = SqliteApiArtifactStore::new(pool.clone());
        let missing = ArtifactId::new();
        assert!(matches!(
            ApiArtifactStore::get(&store, missing)
                .await
                .expect_err("missing artifact"),
            StoreError::NotFound { .. }
        ));
        assert!(!ApiArtifactStore::verify(&store, missing)
            .await
            .expect("missing verify is false"));

        let corrupt = ArtifactId::new();
        pool.writer()
            .execute(
                "INSERT INTO artifacts
                    (id, kind, algorithm, digest_hex, classification, size_bytes,
                     metadata_json, created_at)
                 VALUES (?1, 'test', 'blake3', ?2, 'unknown', 0, '{}', ?3)",
                rusqlite::params![
                    corrupt.to_string(),
                    blake3::hash(b"").to_hex().to_string(),
                    chrono::Utc::now().to_rfc3339()
                ],
            )
            .expect("insert corrupt projection");
        assert!(matches!(
            ApiArtifactStore::get(&store, corrupt)
                .await
                .expect_err("corrupt projection"),
            StoreError::Serialisation { .. }
        ));

        pool.writer()
            .execute_batch("ALTER TABLE artifacts RENAME TO unavailable_artifacts")
            .expect("make backend unavailable");
        assert!(matches!(
            ApiArtifactStore::get(&store, ArtifactId::new())
                .await
                .expect_err("backend failure"),
            StoreError::Internal { .. }
        ));
    }

    #[tokio::test]
    async fn content_lineage_and_run_order_are_durable_and_strict() {
        let temp = tempfile::tempdir().expect("temporary artifact database");
        let path = temp.path().join("artifacts.db");
        let pool = SqlitePool::open(&path).expect("open artifact database");
        crate::migrations::migrate(&pool.writer()).expect("migrate artifact database");
        let run_id = RunId::new();
        insert_run(&pool, run_id);
        let store = SqliteApiArtifactStore::new(pool.clone());

        let root = ArtifactId::new();
        let parent = ArtifactId::new();
        let child = ArtifactId::new();
        store_api_artifact(&store, root, Some(run_id), "root", "public", b"root").await;
        store_api_artifact(
            &store,
            parent,
            Some(run_id),
            "parent",
            "internal",
            b"parent",
        )
        .await;
        store_api_artifact(&store, child, Some(run_id), "child", "private", b"child").await;
        ApiArtifactStore::add_lineage(&store, child, parent)
            .await
            .expect("child to parent lineage");
        ApiArtifactStore::add_lineage(&store, parent, root)
            .await
            .expect("parent to root lineage");

        drop(store);
        drop(pool);
        let reopened = SqlitePool::open(&path).expect("reopen artifact database");
        crate::migrations::migrate(&reopened.writer()).expect("recheck migrations");
        let restarted = SqliteApiArtifactStore::new(reopened);
        assert_eq!(
            ApiArtifactStore::get_lineage(&restarted, child)
                .await
                .expect("durable lineage"),
            vec![parent, root]
        );
        let listed = ApiArtifactStore::list_for_run(&restarted, run_id)
            .await
            .expect("durable run artifacts");
        assert_eq!(listed.len(), 3);
        assert_eq!(
            ApiArtifactStore::get_body(&restarted, child)
                .await
                .expect("durable child body"),
            b"child"
        );

        let digest = blake3::hash(b"child").to_hex().to_string();
        restarted
            .pool
            .writer()
            .execute(
                "UPDATE artifact_bodies SET body = ?1 WHERE digest_hex = ?2",
                rusqlite::params![b"tampered".as_slice(), digest],
            )
            .expect("tamper durable body");
        assert!(matches!(
            ApiArtifactStore::get_body(&restarted, child)
                .await
                .expect_err("tampered body"),
            StoreError::IntegrityError { .. }
        ));
    }
}
