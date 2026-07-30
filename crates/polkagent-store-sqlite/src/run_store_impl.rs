//! [`RunStore`] trait implementation for [`SqlitePool`].
//!
//! Wraps synchronous `rusqlite` calls in [`tokio::task::spawn_blocking`] to
//! satisfy the async trait interface. The writer connection is protected by a
//! `parking_lot::Mutex` inside `SqlitePool`, so each method acquires it briefly
//! within the blocking closure.

use async_trait::async_trait;
use chrono::DateTime;
use polkagent_core::RunId;
use polkagent_store_trait::{RunStatus, RunStore, RunSummary, StoreError};

use crate::pool::SqlitePool;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Parse an ISO-8601 timestamp string into a `chrono::DateTime<Utc>`.
fn parse_ts(s: &str) -> Result<polkagent_core::Timestamp, StoreError> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .map_err(|e| StoreError::Internal {
            message: format!("invalid timestamp '{s}': {e}"),
        })
}

/// Parse a UUID string into a `RunId`.
fn parse_run_id(s: &str) -> Result<RunId, StoreError> {
    s.parse::<RunId>().map_err(|e| StoreError::Internal {
        message: format!("invalid run id '{s}': {e}"),
    })
}

/// Map a `rusqlite::Error` to the appropriate `StoreError` variant.
fn map_sqlite_err(e: rusqlite::Error) -> StoreError {
    match &e {
        rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error {
                code: rusqlite::ffi::ErrorCode::ConstraintViolation,
                ..
            },
            _,
        ) => StoreError::Conflict {
            resource_type: "Run",
            id: String::new(),
        },
        _ => StoreError::Internal {
            message: format!("sqlite error: {e}"),
        },
    }
}

/// Map a `rusqlite::Error` to `StoreError`, with a richer Conflict message.
fn map_sqlite_err_with_id(e: rusqlite::Error, id: &str) -> StoreError {
    match &e {
        rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error {
                code: rusqlite::ffi::ErrorCode::ConstraintViolation,
                ..
            },
            _,
        ) => StoreError::Conflict {
            resource_type: "Run",
            id: id.to_string(),
        },
        _ => StoreError::Internal {
            message: format!("sqlite error: {e}"),
        },
    }
}

/// Extract a `RunSummary` from a `rusqlite::Row`.
///
/// Expected column order: id, agent_id, state, created_at, completed_at
fn row_to_summary(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawRunSummary> {
    Ok(RawRunSummary {
        id: row.get(0)?,
        agent_id: row.get(1)?,
        state: row.get(2)?,
        created_at: row.get(3)?,
        completed_at: row.get(4)?,
    })
}

/// Intermediate type holding raw string values from a row before parsing.
struct RawRunSummary {
    id: String,
    agent_id: String,
    state: String,
    created_at: String,
    completed_at: Option<String>,
}

impl RawRunSummary {
    fn into_run_summary(self) -> Result<RunSummary, StoreError> {
        Ok(RunSummary {
            id: parse_run_id(&self.id)?,
            agent_id: self.agent_id,
            status: RunStatus::new(self.state),
            created_at: parse_ts(&self.created_at)?,
            completed_at: self
                .completed_at
                .as_deref()
                .map(parse_ts)
                .transpose()?,
        })
    }
}

// ---------------------------------------------------------------------------
// RunStore implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl RunStore for SqlitePool {
    async fn create(
        &self,
        run_id: RunId,
        agent_id: &str,
        status: RunStatus,
    ) -> Result<(), StoreError> {
        let pool = self.clone();
        let agent_id = agent_id.to_string();
        let status_str = status.0;

        tokio::task::spawn_blocking(move || {
            let now = chrono::Utc::now().to_rfc3339();
            let id_str = run_id.to_string();
            let writer = pool.writer();
            writer
                .execute(
                    "INSERT INTO runs (id, agent_id, state, params_json, created_at, updated_at)
                     VALUES (?1, ?2, ?3, '{}', ?4, ?5)",
                    rusqlite::params![id_str, agent_id, status_str, now, now],
                )
                .map_err(|e| map_sqlite_err_with_id(e, &id_str))?;
            Ok(())
        })
        .await
        .map_err(|e| StoreError::Internal {
            message: format!("blocking task panicked: {e}"),
        })?
    }

    async fn get(&self, run_id: RunId) -> Result<RunSummary, StoreError> {
        let pool = self.clone();

        tokio::task::spawn_blocking(move || {
            let id_str = run_id.to_string();
            let writer = pool.writer();
            let raw = writer
                .query_row(
                    "SELECT id, agent_id, state, created_at, completed_at
                     FROM runs WHERE id = ?1",
                    [&id_str],
                    row_to_summary,
                )
                .map_err(|e| match e {
                    rusqlite::Error::QueryReturnedNoRows => StoreError::NotFound {
                        resource_type: "Run",
                        id: id_str.clone(),
                    },
                    other => map_sqlite_err(other),
                })?;
            raw.into_run_summary()
        })
        .await
        .map_err(|e| StoreError::Internal {
            message: format!("blocking task panicked: {e}"),
        })?
    }

    async fn update_state(
        &self,
        run_id: RunId,
        new_status: RunStatus,
    ) -> Result<(), StoreError> {
        let pool = self.clone();
        let status_str = new_status.0;

        tokio::task::spawn_blocking(move || {
            let id_str = run_id.to_string();
            let now = chrono::Utc::now().to_rfc3339();
            let writer = pool.writer();

            // If the status looks terminal, also set completed_at.
            let is_terminal = matches!(
                status_str.as_str(),
                "completed" | "failed" | "cancelled" | "timed_out"
            );
            let completed_at: Option<String> = if is_terminal { Some(now.clone()) } else { None };

            let n = writer
                .execute(
                    "UPDATE runs SET state = ?1, updated_at = ?2, completed_at = COALESCE(?3, completed_at)
                     WHERE id = ?4",
                    rusqlite::params![status_str, now, completed_at, id_str],
                )
                .map_err(|e| map_sqlite_err(e))?;

            if n == 0 {
                return Err(StoreError::NotFound {
                    resource_type: "Run",
                    id: id_str,
                });
            }
            Ok(())
        })
        .await
        .map_err(|e| StoreError::Internal {
            message: format!("blocking task panicked: {e}"),
        })?
    }

    async fn list_by_agent(
        &self,
        agent_id: &str,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<RunSummary>, StoreError> {
        let pool = self.clone();
        let agent_id = agent_id.to_string();

        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();
            let mut stmt = writer
                .prepare(
                    "SELECT id, agent_id, state, created_at, completed_at
                     FROM runs
                     WHERE agent_id = ?1
                     ORDER BY created_at DESC
                     LIMIT ?2 OFFSET ?3",
                )
                .map_err(map_sqlite_err)?;

            let raw_rows = stmt
                .query_map(rusqlite::params![agent_id, limit, offset], row_to_summary)
                .map_err(map_sqlite_err)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(map_sqlite_err)?;

            raw_rows
                .into_iter()
                .map(RawRunSummary::into_run_summary)
                .collect()
        })
        .await
        .map_err(|e| StoreError::Internal {
            message: format!("blocking task panicked: {e}"),
        })?
    }

    async fn list_by_state(
        &self,
        status: RunStatus,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<RunSummary>, StoreError> {
        let pool = self.clone();
        let status_str = status.0;

        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();
            let mut stmt = writer
                .prepare(
                    "SELECT id, agent_id, state, created_at, completed_at
                     FROM runs
                     WHERE state = ?1
                     ORDER BY created_at DESC
                     LIMIT ?2 OFFSET ?3",
                )
                .map_err(map_sqlite_err)?;

            let raw_rows = stmt
                .query_map(rusqlite::params![status_str, limit, offset], row_to_summary)
                .map_err(map_sqlite_err)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(map_sqlite_err)?;

            raw_rows
                .into_iter()
                .map(RawRunSummary::into_run_summary)
                .collect()
        })
        .await
        .map_err(|e| StoreError::Internal {
            message: format!("blocking task panicked: {e}"),
        })?
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migrations;

    /// Create an in-memory pool with the schema applied.
    fn test_pool() -> SqlitePool {
        let pool = SqlitePool::open_in_memory().expect("open in-memory pool");
        {
            let writer = pool.writer();
            migrations::migrate(&writer).expect("migrate");
            // Insert a dummy agent so FK on runs.agent_id is satisfied.
            writer
                .execute(
                    "INSERT INTO agents (id, name, state, spec_json, created_at, updated_at)
                     VALUES ('test-agent', 'Test Agent', 'active', '{}', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z')",
                    [],
                )
                .expect("insert test agent");
        }
        pool
    }

    #[tokio::test]
    async fn create_and_get() {
        let pool = test_pool();
        let run_id = RunId::new();

        RunStore::create(&pool, run_id, "test-agent", RunStatus::new("created"))
            .await
            .expect("create run");

        let summary = RunStore::get(&pool, run_id).await.expect("get run");
        assert_eq!(summary.id, run_id);
        assert_eq!(summary.agent_id, "test-agent");
        assert_eq!(summary.status.as_str(), "created");
        assert!(summary.completed_at.is_none());
    }

    #[tokio::test]
    async fn create_duplicate_returns_conflict() {
        let pool = test_pool();
        let run_id = RunId::new();

        RunStore::create(&pool, run_id, "test-agent", RunStatus::new("created"))
            .await
            .expect("first create");

        let err = RunStore::create(&pool, run_id, "test-agent", RunStatus::new("created"))
            .await
            .expect_err("duplicate should fail");

        assert!(
            matches!(err, StoreError::Conflict { .. }),
            "expected Conflict, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn get_not_found() {
        let pool = test_pool();
        let run_id = RunId::new();

        let err = RunStore::get(&pool, run_id)
            .await
            .expect_err("should not find non-existent run");

        assert!(
            matches!(err, StoreError::NotFound { .. }),
            "expected NotFound, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn update_state() {
        let pool = test_pool();
        let run_id = RunId::new();

        RunStore::create(&pool, run_id, "test-agent", RunStatus::new("created"))
            .await
            .expect("create");

        RunStore::update_state(&pool, run_id, RunStatus::new("running"))
            .await
            .expect("update to running");

        let summary = RunStore::get(&pool, run_id).await.expect("get");
        assert_eq!(summary.status.as_str(), "running");
        assert!(summary.completed_at.is_none());
    }

    #[tokio::test]
    async fn update_state_to_terminal_sets_completed_at() {
        let pool = test_pool();
        let run_id = RunId::new();

        RunStore::create(&pool, run_id, "test-agent", RunStatus::new("created"))
            .await
            .expect("create");

        RunStore::update_state(&pool, run_id, RunStatus::new("completed"))
            .await
            .expect("update to completed");

        let summary = RunStore::get(&pool, run_id).await.expect("get");
        assert_eq!(summary.status.as_str(), "completed");
        assert!(summary.completed_at.is_some());
    }

    #[tokio::test]
    async fn update_state_not_found() {
        let pool = test_pool();
        let run_id = RunId::new();

        let err = RunStore::update_state(&pool, run_id, RunStatus::new("running"))
            .await
            .expect_err("should fail for non-existent run");

        assert!(
            matches!(err, StoreError::NotFound { .. }),
            "expected NotFound, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn list_by_agent_empty() {
        let pool = test_pool();

        let runs = RunStore::list_by_agent(&pool, "test-agent", 10, 0)
            .await
            .expect("list");

        assert!(runs.is_empty());
    }

    #[tokio::test]
    async fn list_by_agent_returns_descending_order() {
        let pool = test_pool();
        let id1 = RunId::new();
        let id2 = RunId::new();

        RunStore::create(&pool, id1, "test-agent", RunStatus::new("created"))
            .await
            .expect("create 1");
        RunStore::create(&pool, id2, "test-agent", RunStatus::new("running"))
            .await
            .expect("create 2");

        let runs = RunStore::list_by_agent(&pool, "test-agent", 10, 0)
            .await
            .expect("list");

        assert_eq!(runs.len(), 2);
        // Descending by created_at: id2 should come first (created later).
        assert_eq!(runs[0].id, id2);
        assert_eq!(runs[1].id, id1);
    }

    #[tokio::test]
    async fn list_by_agent_pagination() {
        let pool = test_pool();

        // Create 3 runs.
        let mut ids = Vec::new();
        for _ in 0..3 {
            let id = RunId::new();
            RunStore::create(&pool, id, "test-agent", RunStatus::new("created"))
                .await
                .expect("create");
            ids.push(id);
        }

        // Page 1: limit=2, offset=0
        let page1 = RunStore::list_by_agent(&pool, "test-agent", 2, 0)
            .await
            .expect("page 1");
        assert_eq!(page1.len(), 2);

        // Page 2: limit=2, offset=2
        let page2 = RunStore::list_by_agent(&pool, "test-agent", 2, 2)
            .await
            .expect("page 2");
        assert_eq!(page2.len(), 1);

        // Page 3: limit=2, offset=4
        let page3 = RunStore::list_by_agent(&pool, "test-agent", 2, 4)
            .await
            .expect("page 3");
        assert!(page3.is_empty());
    }

    #[tokio::test]
    async fn list_by_agent_filters_by_agent() {
        let pool = test_pool();

        // Insert a second agent.
        {
            let writer = pool.writer();
            writer
                .execute(
                    "INSERT INTO agents (id, name, state, spec_json, created_at, updated_at)
                     VALUES ('other-agent', 'Other Agent', 'active', '{}', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z')",
                    [],
                )
                .expect("insert other agent");
        }

        let id1 = RunId::new();
        let id2 = RunId::new();
        RunStore::create(&pool, id1, "test-agent", RunStatus::new("created"))
            .await
            .expect("create for test-agent");
        RunStore::create(&pool, id2, "other-agent", RunStatus::new("created"))
            .await
            .expect("create for other-agent");

        let runs = RunStore::list_by_agent(&pool, "test-agent", 10, 0)
            .await
            .expect("list");
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].id, id1);
    }

    #[tokio::test]
    async fn list_by_state_empty() {
        let pool = test_pool();

        let runs = RunStore::list_by_state(&pool, RunStatus::new("running"), 10, 0)
            .await
            .expect("list");

        assert!(runs.is_empty());
    }

    #[tokio::test]
    async fn list_by_state_filters_correctly() {
        let pool = test_pool();
        let id1 = RunId::new();
        let id2 = RunId::new();

        RunStore::create(&pool, id1, "test-agent", RunStatus::new("created"))
            .await
            .expect("create 1");
        RunStore::create(&pool, id2, "test-agent", RunStatus::new("running"))
            .await
            .expect("create 2");

        let created_runs =
            RunStore::list_by_state(&pool, RunStatus::new("created"), 10, 0)
                .await
                .expect("list created");
        assert_eq!(created_runs.len(), 1);
        assert_eq!(created_runs[0].id, id1);

        let running_runs =
            RunStore::list_by_state(&pool, RunStatus::new("running"), 10, 0)
                .await
                .expect("list running");
        assert_eq!(running_runs.len(), 1);
        assert_eq!(running_runs[0].id, id2);
    }

    #[tokio::test]
    async fn list_by_state_pagination() {
        let pool = test_pool();

        for _ in 0..5 {
            let id = RunId::new();
            RunStore::create(&pool, id, "test-agent", RunStatus::new("created"))
                .await
                .expect("create");
        }

        let page1 = RunStore::list_by_state(&pool, RunStatus::new("created"), 3, 0)
            .await
            .expect("page 1");
        assert_eq!(page1.len(), 3);

        let page2 = RunStore::list_by_state(&pool, RunStatus::new("created"), 3, 3)
            .await
            .expect("page 2");
        assert_eq!(page2.len(), 2);
    }

    /// Verify that `SqlitePool` satisfies the `dyn RunStore` object-safety requirement.
    #[allow(dead_code)]
    fn _pool_is_run_store_object_safe(_s: &dyn RunStore) {}

    #[allow(dead_code)]
    fn _pool_is_run_store() {
        fn assert_run_store<T: RunStore>() {}
        assert_run_store::<SqlitePool>();
    }
}
