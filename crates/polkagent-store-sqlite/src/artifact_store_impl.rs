//! [`ArtifactStore`] trait implementation for [`SqlitePool`].
//!
//! Wraps synchronous `rusqlite` calls in [`tokio::task::spawn_blocking`] to
//! satisfy the async trait interface.  The writer connection is protected by a
//! `parking_lot::Mutex` inside `SqlitePool`, so each method acquires it briefly
//! within the blocking closure.

use std::collections::{HashSet, VecDeque};

use polkagent_artifact::store::{ArtifactStore, StoreError};
use polkagent_core::artifact::{Artifact, ArtifactKind, BlobRef};
use polkagent_core::config::DataClassification;
use polkagent_core::ids::{ArtifactId, RunId};

use crate::pool::SqlitePool;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Map a `rusqlite::Error` to the artifact `StoreError::Backend`.
fn map_backend(e: rusqlite::Error) -> StoreError {
    StoreError::Backend(Box::new(e))
}

/// Map a `serde_json::Error` to `StoreError::Backend`.
fn map_json(e: serde_json::Error) -> StoreError {
    StoreError::Backend(Box::new(e))
}

/// Parse a `&str` into an `ArtifactId`.
fn parse_artifact_id(s: &str) -> Result<ArtifactId, StoreError> {
    s.parse::<ArtifactId>()
        .map_err(|e| StoreError::Backend(Box::new(e)))
}

/// Parse a `&str` into a `RunId`.
fn parse_run_id(s: &str) -> Result<RunId, StoreError> {
    s.parse::<RunId>()
        .map_err(|e| StoreError::Backend(Box::new(e)))
}

/// Parse an ISO-8601 timestamp string into `chrono::DateTime<Utc>`.
fn parse_ts(s: &str) -> Result<chrono::DateTime<chrono::Utc>, StoreError> {
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .map_err(|e| StoreError::Backend(Box::new(e)))
}

/// Reconstruct an [`Artifact`] from raw column values.
fn row_to_artifact(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawArtifactRow> {
    Ok(RawArtifactRow {
        id: row.get(0)?,
        run_id: row.get(1)?,
        kind: row.get(2)?,
        digest_hex: row.get(3)?,
        size_bytes: row.get::<_, i64>(4)?,
        metadata_json: row.get(5)?,
        created_at: row.get(6)?,
    })
}

/// Intermediate type holding raw string values from a row before parsing.
struct RawArtifactRow {
    id: String,
    run_id: Option<String>,
    kind: String,
    digest_hex: String,
    size_bytes: i64,
    metadata_json: String,
    created_at: String,
}

impl RawArtifactRow {
    fn into_artifact(self) -> Result<Artifact, StoreError> {
        let id = parse_artifact_id(&self.id)?;
        let run_id = self.run_id.as_deref().map(parse_run_id).transpose()?;
        let kind: ArtifactKind = serde_json::from_str(&self.kind).map_err(map_json)?;
        let created_at = parse_ts(&self.created_at)?;
        let metadata: std::collections::HashMap<String, String> =
            serde_json::from_str(&self.metadata_json).map_err(map_json)?;

        Ok(Artifact {
            id,
            run_id,
            step_id: None,
            kind,
            blob_ref: BlobRef {
                blake3_hex: self.digest_hex,
                sha256_hex: None,
                size_bytes: self.size_bytes as u64,
            },
            classification: DataClassification::default(),
            parents: Vec::new(),
            created_at,
            metadata,
        })
    }
}

// ---------------------------------------------------------------------------
// ArtifactStore implementation
// ---------------------------------------------------------------------------

impl ArtifactStore for SqlitePool {
    async fn store(&self, artifact: &Artifact, body: &[u8]) -> Result<(), StoreError> {
        let pool = self.clone();
        let id_str = artifact.id.to_string();
        let run_id_str = artifact.run_id.map(|r| r.to_string());
        let kind_json = serde_json::to_string(&artifact.kind).map_err(map_json)?;
        let digest_hex = artifact.blob_ref.blake3_hex.clone();
        let size_bytes = artifact.blob_ref.size_bytes as i64;
        let metadata_json =
            serde_json::to_string(&artifact.metadata).map_err(map_json)?;
        let created_at = artifact.created_at.to_rfc3339();
        let body = body.to_vec();

        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();

            // Use a transaction for atomicity.
            writer.execute_batch("BEGIN IMMEDIATE")?;

            let result = (|| -> Result<(), rusqlite::Error> {
                // Insert artifact metadata.
                writer.execute(
                    "INSERT INTO artifacts (id, run_id, kind, digest_hex, size_bytes, metadata_json, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    rusqlite::params![
                        id_str,
                        run_id_str,
                        kind_json,
                        digest_hex,
                        size_bytes,
                        metadata_json,
                        created_at,
                    ],
                )?;

                // Insert body (content-addressed, deduped).
                writer.execute(
                    "INSERT OR IGNORE INTO artifact_bodies (digest_hex, body) VALUES (?1, ?2)",
                    rusqlite::params![digest_hex, body],
                )?;

                Ok(())
            })();

            match result {
                Ok(()) => {
                    writer.execute_batch("COMMIT")?;
                    Ok(())
                }
                Err(e) => {
                    let _ = writer.execute_batch("ROLLBACK");
                    Err(e)
                }
            }
        })
        .await
        .map_err(|e| StoreError::Backend(Box::new(e)))?
        .map_err(map_backend)
    }

    async fn get(&self, id: ArtifactId) -> Result<Artifact, StoreError> {
        let pool = self.clone();

        tokio::task::spawn_blocking(move || {
            let id_str = id.to_string();
            let writer = pool.writer();
            let raw = writer
                .query_row(
                    "SELECT id, run_id, kind, digest_hex, size_bytes, metadata_json, created_at
                     FROM artifacts WHERE id = ?1",
                    [&id_str],
                    row_to_artifact,
                )
                .map_err(|e| match e {
                    rusqlite::Error::QueryReturnedNoRows => StoreError::NotFound(id),
                    other => map_backend(other),
                })?;
            raw.into_artifact()
        })
        .await
        .map_err(|e| StoreError::Backend(Box::new(e)))?
    }

    async fn get_body(&self, id: ArtifactId) -> Result<Vec<u8>, StoreError> {
        let pool = self.clone();

        tokio::task::spawn_blocking(move || {
            let id_str = id.to_string();
            let writer = pool.writer();

            // Fetch the digest for this artifact.
            let digest_hex: String = writer
                .query_row(
                    "SELECT digest_hex FROM artifacts WHERE id = ?1",
                    [&id_str],
                    |r| r.get(0),
                )
                .map_err(|e| match e {
                    rusqlite::Error::QueryReturnedNoRows => StoreError::NotFound(id),
                    other => map_backend(other),
                })?;

            // Fetch the body by digest.
            let body: Vec<u8> = writer
                .query_row(
                    "SELECT body FROM artifact_bodies WHERE digest_hex = ?1",
                    [&digest_hex],
                    |r| r.get(0),
                )
                .map_err(|e| match e {
                    rusqlite::Error::QueryReturnedNoRows => StoreError::NotFound(id),
                    other => map_backend(other),
                })?;

            // Verify BLAKE3 digest before returning.
            let actual_hash = blake3::hash(&body);
            if actual_hash.to_hex().as_str() != digest_hex {
                return Err(StoreError::DigestMismatch(id));
            }

            Ok(body)
        })
        .await
        .map_err(|e| StoreError::Backend(Box::new(e)))?
    }

    async fn verify(&self, id: ArtifactId) -> Result<bool, StoreError> {
        let pool = self.clone();

        tokio::task::spawn_blocking(move || {
            let id_str = id.to_string();
            let writer = pool.writer();

            // Fetch the digest for this artifact.
            let digest_hex: String = writer
                .query_row(
                    "SELECT digest_hex FROM artifacts WHERE id = ?1",
                    [&id_str],
                    |r| r.get(0),
                )
                .map_err(|e| match e {
                    rusqlite::Error::QueryReturnedNoRows => StoreError::NotFound(id),
                    other => map_backend(other),
                })?;

            // Attempt to fetch the body.
            let body_result: Result<Vec<u8>, _> = writer.query_row(
                "SELECT body FROM artifact_bodies WHERE digest_hex = ?1",
                [&digest_hex],
                |r| r.get(0),
            );

            match body_result {
                Ok(body) => {
                    let actual_hash = blake3::hash(&body);
                    Ok(actual_hash.to_hex().as_str() == digest_hex)
                }
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(false),
                Err(e) => Err(map_backend(e)),
            }
        })
        .await
        .map_err(|e| StoreError::Backend(Box::new(e)))?
    }

    async fn list_for_run(&self, run_id: RunId) -> Result<Vec<Artifact>, StoreError> {
        let pool = self.clone();

        tokio::task::spawn_blocking(move || {
            let run_id_str = run_id.to_string();
            let writer = pool.writer();
            let mut stmt = writer
                .prepare(
                    "SELECT id, run_id, kind, digest_hex, size_bytes, metadata_json, created_at
                     FROM artifacts WHERE run_id = ?1 ORDER BY created_at ASC",
                )
                .map_err(map_backend)?;

            let raw_rows = stmt
                .query_map([&run_id_str], row_to_artifact)
                .map_err(map_backend)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(map_backend)?;

            raw_rows
                .into_iter()
                .map(RawArtifactRow::into_artifact)
                .collect()
        })
        .await
        .map_err(|e| StoreError::Backend(Box::new(e)))?
    }

    async fn add_lineage(
        &self,
        child_id: ArtifactId,
        parent_id: ArtifactId,
    ) -> Result<(), StoreError> {
        let pool = self.clone();

        tokio::task::spawn_blocking(move || {
            let child_str = child_id.to_string();
            let parent_str = parent_id.to_string();
            let writer = pool.writer();
            writer
                .execute(
                    "INSERT OR IGNORE INTO artifact_lineage (child_id, parent_id) VALUES (?1, ?2)",
                    rusqlite::params![child_str, parent_str],
                )
                .map_err(map_backend)?;
            Ok(())
        })
        .await
        .map_err(|e| StoreError::Backend(Box::new(e)))?
    }

    async fn get_lineage(&self, id: ArtifactId) -> Result<Vec<ArtifactId>, StoreError> {
        let pool = self.clone();

        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();

            // BFS through ancestor chain.
            let mut ancestors = Vec::new();
            let mut visited = HashSet::new();
            let mut queue = VecDeque::new();
            queue.push_back(id.to_string());
            visited.insert(id.to_string());

            while let Some(current) = queue.pop_front() {
                let mut stmt = writer
                    .prepare(
                        "SELECT parent_id FROM artifact_lineage WHERE child_id = ?1",
                    )
                    .map_err(map_backend)?;

                let parents: Vec<String> = stmt
                    .query_map([&current], |r| r.get(0))
                    .map_err(map_backend)?
                    .collect::<Result<Vec<String>, _>>()
                    .map_err(map_backend)?;

                for parent_str in parents {
                    if visited.insert(parent_str.clone()) {
                        let parent_id = parse_artifact_id(&parent_str)?;
                        ancestors.push(parent_id);
                        queue.push_back(parent_str);
                    }
                }
            }

            Ok(ancestors)
        })
        .await
        .map_err(|e| StoreError::Backend(Box::new(e)))?
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migrations;
    use polkagent_core::artifact::ArtifactKind;

    const TEST_AGENT_ID: &str = "test-agent";

    /// Create an in-memory pool with the schema applied and a test agent
    /// pre-inserted (required to satisfy the FK on `runs.agent_id`).
    fn test_pool() -> SqlitePool {
        let pool = SqlitePool::open_in_memory().expect("open in-memory pool");
        {
            let writer = pool.writer();
            migrations::migrate(&writer).expect("migrate");
            writer
                .execute(
                    "INSERT INTO agents (id, name, state, spec_json, created_at, updated_at)
                     VALUES (?1, 'Test Agent', 'active', '{}', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z')",
                    [TEST_AGENT_ID],
                )
                .expect("insert test agent");
        }
        pool
    }

    /// Insert a run record in the database and return its `RunId`.
    /// Required for tests that set `artifact.run_id` (FK constraint).
    fn insert_test_run(pool: &SqlitePool, run_id: RunId) {
        let writer = pool.writer();
        let now = chrono::Utc::now().to_rfc3339();
        writer
            .execute(
                "INSERT INTO runs (id, agent_id, state, params_json, created_at, updated_at)
                 VALUES (?1, ?2, 'created', '{}', ?3, ?4)",
                rusqlite::params![run_id.to_string(), TEST_AGENT_ID, now, now],
            )
            .expect("insert test run");
    }

    /// Build a simple test artifact from bytes.
    fn make_artifact(body: &[u8]) -> Artifact {
        Artifact::from_bytes(
            ArtifactId::new(),
            ArtifactKind::Custom {
                type_uri: "test://v1".into(),
            },
            body,
        )
    }

    #[tokio::test]
    async fn store_and_get_round_trip() {
        let pool = test_pool();
        let body = b"hello artifact";
        let artifact = make_artifact(body);
        let id = artifact.id;

        ArtifactStore::store(&pool, &artifact, body)
            .await
            .expect("store");

        let fetched = ArtifactStore::get(&pool, id).await.expect("get");
        assert_eq!(fetched.id, id);
        assert_eq!(fetched.blob_ref.blake3_hex, artifact.blob_ref.blake3_hex);
        assert_eq!(fetched.blob_ref.size_bytes, body.len() as u64);
    }

    #[tokio::test]
    async fn get_body_returns_correct_bytes() {
        let pool = test_pool();
        let body = b"body content for retrieval";
        let artifact = make_artifact(body);
        let id = artifact.id;

        ArtifactStore::store(&pool, &artifact, body)
            .await
            .expect("store");

        let fetched_body = ArtifactStore::get_body(&pool, id)
            .await
            .expect("get_body");
        assert_eq!(fetched_body, body);
    }

    #[tokio::test]
    async fn get_body_detects_digest_mismatch() {
        let pool = test_pool();
        let body = b"original content";
        let artifact = make_artifact(body);
        let id = artifact.id;

        // Store the artifact normally.
        ArtifactStore::store(&pool, &artifact, body)
            .await
            .expect("store");

        // Tamper with the body in the database directly.
        {
            let writer = pool.writer();
            writer
                .execute(
                    "UPDATE artifact_bodies SET body = ?1 WHERE digest_hex = ?2",
                    rusqlite::params![b"tampered content", artifact.blob_ref.blake3_hex],
                )
                .expect("tamper body");
        }

        let err = ArtifactStore::get_body(&pool, id)
            .await
            .expect_err("should detect mismatch");

        assert!(
            matches!(err, StoreError::DigestMismatch(_)),
            "expected DigestMismatch, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn get_not_found() {
        let pool = test_pool();
        let id = ArtifactId::new();

        let err = ArtifactStore::get(&pool, id)
            .await
            .expect_err("should be not found");

        assert!(
            matches!(err, StoreError::NotFound(_)),
            "expected NotFound, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn get_body_not_found() {
        let pool = test_pool();
        let id = ArtifactId::new();

        let err = ArtifactStore::get_body(&pool, id)
            .await
            .expect_err("should be not found");

        assert!(
            matches!(err, StoreError::NotFound(_)),
            "expected NotFound, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn verify_returns_true_for_valid_body() {
        let pool = test_pool();
        let body = b"verified content";
        let artifact = make_artifact(body);
        let id = artifact.id;

        ArtifactStore::store(&pool, &artifact, body)
            .await
            .expect("store");

        let valid = ArtifactStore::verify(&pool, id).await.expect("verify");
        assert!(valid);
    }

    #[tokio::test]
    async fn verify_returns_false_for_tampered_body() {
        let pool = test_pool();
        let body = b"original verified content";
        let artifact = make_artifact(body);
        let id = artifact.id;

        ArtifactStore::store(&pool, &artifact, body)
            .await
            .expect("store");

        // Tamper with the body.
        {
            let writer = pool.writer();
            writer
                .execute(
                    "UPDATE artifact_bodies SET body = ?1 WHERE digest_hex = ?2",
                    rusqlite::params![b"tampered!", artifact.blob_ref.blake3_hex],
                )
                .expect("tamper");
        }

        let valid = ArtifactStore::verify(&pool, id).await.expect("verify");
        assert!(!valid);
    }

    #[tokio::test]
    async fn verify_returns_false_when_body_missing() {
        let pool = test_pool();
        let body = b"will be deleted";
        let artifact = make_artifact(body);
        let id = artifact.id;

        ArtifactStore::store(&pool, &artifact, body)
            .await
            .expect("store");

        // Delete the body.
        {
            let writer = pool.writer();
            writer
                .execute(
                    "DELETE FROM artifact_bodies WHERE digest_hex = ?1",
                    [&artifact.blob_ref.blake3_hex],
                )
                .expect("delete body");
        }

        let valid = ArtifactStore::verify(&pool, id).await.expect("verify");
        assert!(!valid);
    }

    #[tokio::test]
    async fn list_for_run_returns_artifacts_in_order() {
        let pool = test_pool();
        let run_id = RunId::new();
        insert_test_run(&pool, run_id);

        let body1 = b"first artifact body";
        let mut art1 = make_artifact(body1);
        art1.run_id = Some(run_id);

        let body2 = b"second artifact body";
        let mut art2 = make_artifact(body2);
        art2.run_id = Some(run_id);

        ArtifactStore::store(&pool, &art1, body1)
            .await
            .expect("store 1");
        ArtifactStore::store(&pool, &art2, body2)
            .await
            .expect("store 2");

        let list = ArtifactStore::list_for_run(&pool, run_id)
            .await
            .expect("list");

        assert_eq!(list.len(), 2);
        assert_eq!(list[0].id, art1.id);
        assert_eq!(list[1].id, art2.id);
    }

    #[tokio::test]
    async fn list_for_run_excludes_other_runs() {
        let pool = test_pool();
        let run_a = RunId::new();
        let run_b = RunId::new();
        insert_test_run(&pool, run_a);
        insert_test_run(&pool, run_b);

        let body_a = b"run a artifact";
        let mut art_a = make_artifact(body_a);
        art_a.run_id = Some(run_a);

        let body_b = b"run b artifact";
        let mut art_b = make_artifact(body_b);
        art_b.run_id = Some(run_b);

        ArtifactStore::store(&pool, &art_a, body_a)
            .await
            .expect("store a");
        ArtifactStore::store(&pool, &art_b, body_b)
            .await
            .expect("store b");

        let list = ArtifactStore::list_for_run(&pool, run_a)
            .await
            .expect("list");
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, art_a.id);
    }

    #[tokio::test]
    async fn list_for_run_empty() {
        let pool = test_pool();
        let run_id = RunId::new();

        let list = ArtifactStore::list_for_run(&pool, run_id)
            .await
            .expect("list");
        assert!(list.is_empty());
    }

    #[tokio::test]
    async fn add_lineage_and_get_lineage() {
        let pool = test_pool();

        // Create three artifacts: grandparent -> parent -> child
        let body_gp = b"grandparent";
        let art_gp = make_artifact(body_gp);
        ArtifactStore::store(&pool, &art_gp, body_gp)
            .await
            .expect("store gp");

        let body_p = b"parent";
        let art_p = make_artifact(body_p);
        ArtifactStore::store(&pool, &art_p, body_p)
            .await
            .expect("store parent");

        let body_c = b"child";
        let art_c = make_artifact(body_c);
        ArtifactStore::store(&pool, &art_c, body_c)
            .await
            .expect("store child");

        // Record lineage.
        ArtifactStore::add_lineage(&pool, art_c.id, art_p.id)
            .await
            .expect("link child->parent");
        ArtifactStore::add_lineage(&pool, art_p.id, art_gp.id)
            .await
            .expect("link parent->gp");

        // Get lineage for child: should return [parent, grandparent] in BFS order.
        let lineage = ArtifactStore::get_lineage(&pool, art_c.id)
            .await
            .expect("get lineage");

        assert_eq!(lineage.len(), 2);
        assert_eq!(lineage[0], art_p.id);
        assert_eq!(lineage[1], art_gp.id);
    }

    #[tokio::test]
    async fn add_lineage_duplicate_is_idempotent() {
        let pool = test_pool();

        let body_p = b"parent art";
        let art_p = make_artifact(body_p);
        ArtifactStore::store(&pool, &art_p, body_p)
            .await
            .expect("store parent");

        let body_c = b"child art";
        let art_c = make_artifact(body_c);
        ArtifactStore::store(&pool, &art_c, body_c)
            .await
            .expect("store child");

        // Add the same edge twice.
        ArtifactStore::add_lineage(&pool, art_c.id, art_p.id)
            .await
            .expect("first add");
        ArtifactStore::add_lineage(&pool, art_c.id, art_p.id)
            .await
            .expect("duplicate add should succeed");

        let lineage = ArtifactStore::get_lineage(&pool, art_c.id)
            .await
            .expect("get lineage");
        assert_eq!(lineage.len(), 1);
    }

    #[tokio::test]
    async fn get_lineage_no_parents() {
        let pool = test_pool();

        let body = b"standalone";
        let art = make_artifact(body);
        ArtifactStore::store(&pool, &art, body)
            .await
            .expect("store");

        let lineage = ArtifactStore::get_lineage(&pool, art.id)
            .await
            .expect("get lineage");
        assert!(lineage.is_empty());
    }

    #[tokio::test]
    async fn get_lineage_diamond_dag() {
        let pool = test_pool();

        // Diamond: root -> left, root -> right, left -> leaf, right -> leaf
        let body_root = b"root";
        let art_root = make_artifact(body_root);
        ArtifactStore::store(&pool, &art_root, body_root)
            .await
            .expect("store root");

        let body_left = b"left";
        let art_left = make_artifact(body_left);
        ArtifactStore::store(&pool, &art_left, body_left)
            .await
            .expect("store left");

        let body_right = b"right";
        let art_right = make_artifact(body_right);
        ArtifactStore::store(&pool, &art_right, body_right)
            .await
            .expect("store right");

        let body_leaf = b"leaf";
        let art_leaf = make_artifact(body_leaf);
        ArtifactStore::store(&pool, &art_leaf, body_leaf)
            .await
            .expect("store leaf");

        // Record edges.
        ArtifactStore::add_lineage(&pool, art_left.id, art_root.id)
            .await
            .expect("left->root");
        ArtifactStore::add_lineage(&pool, art_right.id, art_root.id)
            .await
            .expect("right->root");
        ArtifactStore::add_lineage(&pool, art_leaf.id, art_left.id)
            .await
            .expect("leaf->left");
        ArtifactStore::add_lineage(&pool, art_leaf.id, art_right.id)
            .await
            .expect("leaf->right");

        let lineage = ArtifactStore::get_lineage(&pool, art_leaf.id)
            .await
            .expect("get lineage");

        // BFS from leaf: left and right (level 1), then root (level 2).
        // Root should appear exactly once despite two paths.
        assert_eq!(lineage.len(), 3);

        // The first two entries are the direct parents (left, right) in some order.
        let first_two: HashSet<ArtifactId> =
            lineage[..2].iter().copied().collect();
        assert!(first_two.contains(&art_left.id));
        assert!(first_two.contains(&art_right.id));

        // The third entry is the root.
        assert_eq!(lineage[2], art_root.id);
    }

    #[tokio::test]
    async fn store_deduplicates_body_by_digest() {
        let pool = test_pool();
        let body = b"shared content";

        // Two artifacts with the same body content.
        let art1 = make_artifact(body);
        let art2 = make_artifact(body);

        ArtifactStore::store(&pool, &art1, body)
            .await
            .expect("store 1");
        ArtifactStore::store(&pool, &art2, body)
            .await
            .expect("store 2");

        // Both should retrieve the same body.
        let body1 = ArtifactStore::get_body(&pool, art1.id)
            .await
            .expect("get body 1");
        let body2 = ArtifactStore::get_body(&pool, art2.id)
            .await
            .expect("get body 2");
        assert_eq!(body1, body2);
        assert_eq!(body1, body);

        // Only one row should exist in artifact_bodies.
        let count: i64 = {
            let writer = pool.writer();
            writer
                .query_row("SELECT COUNT(*) FROM artifact_bodies", [], |r| r.get(0))
                .expect("count")
        };
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn store_preserves_metadata() {
        let pool = test_pool();
        let body = b"metadata test";
        let mut artifact = make_artifact(body);
        artifact
            .metadata
            .insert("tool".into(), "write_file".into());
        artifact
            .metadata
            .insert("commit".into(), "abc123".into());
        let id = artifact.id;

        ArtifactStore::store(&pool, &artifact, body)
            .await
            .expect("store");

        let fetched = ArtifactStore::get(&pool, id).await.expect("get");
        assert_eq!(fetched.metadata.get("tool").map(String::as_str), Some("write_file"));
        assert_eq!(fetched.metadata.get("commit").map(String::as_str), Some("abc123"));
    }

    #[tokio::test]
    async fn store_preserves_artifact_kind() {
        let pool = test_pool();
        let body = b"file content";
        let run_id = RunId::new();
        insert_test_run(&pool, run_id);
        let mut artifact = Artifact::from_bytes(
            ArtifactId::new(),
            ArtifactKind::File {
                path: "src/main.rs".into(),
                mime_type: Some("text/x-rust".into()),
            },
            body,
        );
        artifact.run_id = Some(run_id);
        let id = artifact.id;

        ArtifactStore::store(&pool, &artifact, body)
            .await
            .expect("store");

        let fetched = ArtifactStore::get(&pool, id).await.expect("get");
        assert_eq!(fetched.kind, artifact.kind);
    }

    /// Verify that `SqlitePool` satisfies the `ArtifactStore` trait bounds.
    #[allow(dead_code)]
    fn _pool_is_artifact_store() {
        fn assert_artifact_store<T: ArtifactStore>() {}
        assert_artifact_store::<SqlitePool>();
    }
}
