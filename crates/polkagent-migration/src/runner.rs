//! Migration runner: applies pending migrations, supports rollback and dry-run.
//!
//! Each migration runs in its own transaction so that a failure leaves the
//! database in a consistent state at the last successfully applied version.

use std::path::{Path, PathBuf};

use chrono::Utc;
use rusqlite::Connection;
use tracing::{debug, info, warn};

use crate::error::{MigrationError, MigrationResult};
use crate::migration::{compute_checksum, AppliedMigration, Migration};

/// Result of applying a single migration.
#[derive(Debug, Clone)]
pub struct ApplyResult {
    pub version: u32,
    pub name: String,
    pub dry_run: bool,
}

/// Result of rolling back a migration.
#[derive(Debug, Clone)]
pub struct RollbackResult {
    pub version: u32,
    pub name: String,
    pub dry_run: bool,
}

/// Manages applying and rolling back migrations against a `SQLite` database.
pub struct MigrationRunner {
    lock_path: Option<PathBuf>,
}

impl MigrationRunner {
    /// Create a new runner.
    ///
    /// If `lock_dir` is provided, a lock file will be created in that directory
    /// to prevent concurrent migration processes.
    pub fn new(lock_dir: Option<&Path>) -> Self {
        Self {
            lock_path: lock_dir.map(|d| d.join(".migration.lock")),
        }
    }

    /// Acquire the lock file, returning an error if it already exists.
    fn acquire_lock(&self) -> MigrationResult<()> {
        if let Some(ref path) = self.lock_path {
            if path.exists() {
                return Err(MigrationError::Locked { path: path.clone() });
            }
            std::fs::write(path, format!("pid={}", std::process::id())).map_err(|e| {
                MigrationError::Io {
                    path: path.clone(),
                    source: e,
                }
            })?;
        }
        Ok(())
    }

    /// Release the lock file.
    fn release_lock(&self) {
        if let Some(ref path) = self.lock_path {
            let _ = std::fs::remove_file(path);
        }
    }

    /// Ensure the `schema_migrations` table exists.
    pub fn ensure_schema_table(conn: &Connection) -> MigrationResult<()> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version     INTEGER PRIMARY KEY,
                description TEXT    NOT NULL,
                applied_at  TEXT    NOT NULL,
                checksum    TEXT    NOT NULL
            );",
        )
        .map_err(|e| MigrationError::SchemaTable(format!("bootstrap: {e}")))?;
        Ok(())
    }

    /// Query all applied migrations from the database.
    pub fn applied_migrations(conn: &Connection) -> MigrationResult<Vec<AppliedMigration>> {
        Self::ensure_schema_table(conn)?;

        let mut stmt = conn
            .prepare(
                "SELECT version, description, applied_at, checksum
                 FROM schema_migrations
                 ORDER BY version ASC",
            )
            .map_err(|e| MigrationError::SchemaTable(format!("prepare: {e}")))?;

        let rows = stmt
            .query_map([], |row| {
                Ok(AppliedMigration {
                    version: row.get(0)?,
                    description: row.get(1)?,
                    applied_at: row.get(2)?,
                    checksum: row.get(3)?,
                })
            })
            .map_err(|e| MigrationError::SchemaTable(format!("query: {e}")))?;

        let mut result = Vec::new();
        for row in rows {
            result.push(row.map_err(MigrationError::Sqlite)?);
        }
        Ok(result)
    }

    /// Return the highest applied version, or 0 if none.
    pub fn current_version(conn: &Connection) -> MigrationResult<u32> {
        Self::ensure_schema_table(conn)?;

        let version: u32 = conn
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
                [],
                |row| row.get(0),
            )
            .map_err(|e| MigrationError::SchemaTable(format!("query max version: {e}")))?;

        Ok(version)
    }

    /// Apply all pending migrations.
    ///
    /// Migrations whose version is greater than the current database version
    /// are applied in order. Each migration runs in its own transaction.
    ///
    /// If `dry_run` is `true`, the SQL is not actually executed and no
    /// transactions are committed.
    pub fn apply_pending(
        &self,
        conn: &Connection,
        migrations: &[Migration],
        dry_run: bool,
    ) -> MigrationResult<Vec<ApplyResult>> {
        self.acquire_lock()?;
        let result = Self::apply_pending_inner(conn, migrations, dry_run);
        self.release_lock();
        result
    }

    fn apply_pending_inner(
        conn: &Connection,
        migrations: &[Migration],
        dry_run: bool,
    ) -> MigrationResult<Vec<ApplyResult>> {
        Self::ensure_schema_table(conn)?;
        Self::validate_ordering(migrations)?;

        let current = Self::current_version(conn)?;
        debug!(current_version = current, "starting migration run");

        let mut results = Vec::new();

        for migration in migrations {
            if migration.version <= current {
                debug!(version = migration.version, "already applied, skipping");
                continue;
            }

            if dry_run {
                info!(
                    version = migration.version,
                    name = %migration.name,
                    "DRY RUN: would apply migration"
                );
                results.push(ApplyResult {
                    version: migration.version,
                    name: migration.name.clone(),
                    dry_run: true,
                });
                continue;
            }

            info!(
                version = migration.version,
                name = %migration.name,
                "applying migration"
            );

            // Run each migration in its own transaction.
            let tx = conn
                .unchecked_transaction()
                .map_err(|e| MigrationError::Apply {
                    version: migration.version,
                    name: migration.name.clone(),
                    reason: format!("begin transaction: {e}"),
                })?;

            tx.execute_batch(&migration.sql)
                .map_err(|e| MigrationError::Apply {
                    version: migration.version,
                    name: migration.name.clone(),
                    reason: e.to_string(),
                })?;

            let checksum = compute_checksum(&migration.sql);
            let applied_at = Utc::now().to_rfc3339();

            tx.execute(
                "INSERT INTO schema_migrations (version, description, applied_at, checksum)
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![migration.version, migration.name, applied_at, checksum],
            )
            .map_err(|e| MigrationError::Apply {
                version: migration.version,
                name: migration.name.clone(),
                reason: format!("record migration: {e}"),
            })?;

            tx.commit().map_err(|e| MigrationError::Apply {
                version: migration.version,
                name: migration.name.clone(),
                reason: format!("commit: {e}"),
            })?;

            info!(
                version = migration.version,
                "migration applied successfully"
            );
            results.push(ApplyResult {
                version: migration.version,
                name: migration.name.clone(),
                dry_run: false,
            });
        }

        Ok(results)
    }

    /// Rollback the last applied migration, if it is reversible.
    ///
    /// If `dry_run` is `true`, the rollback SQL is not actually executed.
    pub fn rollback_last(
        &self,
        conn: &Connection,
        migrations: &[Migration],
        dry_run: bool,
    ) -> MigrationResult<RollbackResult> {
        self.acquire_lock()?;
        let result = Self::rollback_last_inner(conn, migrations, dry_run);
        self.release_lock();
        result
    }

    fn rollback_last_inner(
        conn: &Connection,
        migrations: &[Migration],
        dry_run: bool,
    ) -> MigrationResult<RollbackResult> {
        Self::ensure_schema_table(conn)?;

        let current = Self::current_version(conn)?;
        if current == 0 {
            return Err(MigrationError::NothingToRollback);
        }

        let migration = migrations
            .iter()
            .find(|m| m.version == current)
            .ok_or_else(|| MigrationError::RollbackUnavailable {
                version: current,
                name: String::from("unknown"),
            })?;

        let down_sql =
            migration
                .down_sql
                .as_deref()
                .ok_or_else(|| MigrationError::RollbackUnavailable {
                    version: migration.version,
                    name: migration.name.clone(),
                })?;

        if dry_run {
            info!(
                version = migration.version,
                name = %migration.name,
                "DRY RUN: would roll back migration"
            );
            return Ok(RollbackResult {
                version: migration.version,
                name: migration.name.clone(),
                dry_run: true,
            });
        }

        warn!(
            version = migration.version,
            name = %migration.name,
            "rolling back migration"
        );

        let tx = conn
            .unchecked_transaction()
            .map_err(|e| MigrationError::Apply {
                version: migration.version,
                name: migration.name.clone(),
                reason: format!("begin rollback transaction: {e}"),
            })?;

        tx.execute_batch(down_sql)
            .map_err(|e| MigrationError::Apply {
                version: migration.version,
                name: migration.name.clone(),
                reason: format!("rollback SQL: {e}"),
            })?;

        tx.execute(
            "DELETE FROM schema_migrations WHERE version = ?1",
            rusqlite::params![migration.version],
        )
        .map_err(|e| MigrationError::Apply {
            version: migration.version,
            name: migration.name.clone(),
            reason: format!("remove migration record: {e}"),
        })?;

        tx.commit().map_err(|e| MigrationError::Apply {
            version: migration.version,
            name: migration.name.clone(),
            reason: format!("commit rollback: {e}"),
        })?;

        info!(
            version = migration.version,
            "migration rolled back successfully"
        );

        Ok(RollbackResult {
            version: migration.version,
            name: migration.name.clone(),
            dry_run: false,
        })
    }

    /// Validate that migrations are in strictly ascending version order
    /// and have no zero-valued versions.
    fn validate_ordering(migrations: &[Migration]) -> MigrationResult<()> {
        let mut prev_version = 0u32;
        for m in migrations {
            if m.version == 0 {
                return Err(MigrationError::InvalidVersion(0));
            }
            if m.version <= prev_version {
                return Err(MigrationError::VersionOrdering {
                    version: m.version,
                    previous: prev_version,
                });
            }
            prev_version = m.version;
        }
        Ok(())
    }
}

impl Drop for MigrationRunner {
    fn drop(&mut self) {
        self.release_lock();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn open_mem() -> Connection {
        Connection::open_in_memory().expect("in-memory db")
    }

    fn sample_migrations() -> Vec<Migration> {
        vec![
            Migration::new(
                1,
                "create users",
                "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT NOT NULL);",
            ),
            Migration::new(2, "add email", "ALTER TABLE users ADD COLUMN email TEXT;"),
            Migration::new(
                3,
                "create posts",
                "CREATE TABLE posts (id INTEGER PRIMARY KEY, user_id INTEGER, body TEXT);",
            ),
        ]
    }

    fn reversible_migrations() -> Vec<Migration> {
        vec![
            Migration::new_reversible(
                1,
                "create users",
                "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT NOT NULL);",
                "DROP TABLE users;",
            ),
            Migration::new_reversible(
                2,
                "create posts",
                "CREATE TABLE posts (id INTEGER PRIMARY KEY, body TEXT);",
                "DROP TABLE posts;",
            ),
        ]
    }

    #[test]
    fn apply_all_pending_migrations() {
        let conn = open_mem();
        let runner = MigrationRunner::new(None);
        let migrations = sample_migrations();

        let results = runner
            .apply_pending(&conn, &migrations, false)
            .expect("apply");
        assert_eq!(results.len(), 3);
        assert_eq!(MigrationRunner::current_version(&conn).expect("v"), 3);
    }

    #[test]
    fn apply_is_idempotent() {
        let conn = open_mem();
        let runner = MigrationRunner::new(None);
        let migrations = sample_migrations();

        runner
            .apply_pending(&conn, &migrations, false)
            .expect("first apply");
        let results = runner
            .apply_pending(&conn, &migrations, false)
            .expect("second apply");
        assert!(results.is_empty(), "no new migrations should be applied");
        assert_eq!(MigrationRunner::current_version(&conn).expect("v"), 3);
    }

    #[test]
    fn dry_run_does_not_apply() {
        let conn = open_mem();
        let runner = MigrationRunner::new(None);
        let migrations = sample_migrations();

        let results = runner
            .apply_pending(&conn, &migrations, true)
            .expect("dry run");
        assert_eq!(results.len(), 3);
        assert!(results[0].dry_run);
        // Version should still be 0 — nothing was actually applied.
        assert_eq!(MigrationRunner::current_version(&conn).expect("v"), 0);
    }

    #[test]
    fn partial_apply_then_resume() {
        let conn = open_mem();
        let runner = MigrationRunner::new(None);

        // Apply only the first migration.
        let first = &sample_migrations()[..1];
        runner
            .apply_pending(&conn, first, false)
            .expect("apply first");
        assert_eq!(MigrationRunner::current_version(&conn).expect("v"), 1);

        // Now apply all — only 2 and 3 should run.
        let all = sample_migrations();
        let results = runner
            .apply_pending(&conn, &all, false)
            .expect("apply rest");
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].version, 2);
        assert_eq!(results[1].version, 3);
    }

    #[test]
    fn rollback_reversible_migration() {
        let conn = open_mem();
        let runner = MigrationRunner::new(None);
        let migrations = reversible_migrations();

        runner
            .apply_pending(&conn, &migrations, false)
            .expect("apply");
        assert_eq!(MigrationRunner::current_version(&conn).expect("v"), 2);

        let result = runner
            .rollback_last(&conn, &migrations, false)
            .expect("rollback");
        assert_eq!(result.version, 2);
        assert!(!result.dry_run);
        assert_eq!(MigrationRunner::current_version(&conn).expect("v"), 1);
    }

    #[test]
    fn rollback_dry_run_does_not_change_db() {
        let conn = open_mem();
        let runner = MigrationRunner::new(None);
        let migrations = reversible_migrations();

        runner
            .apply_pending(&conn, &migrations, false)
            .expect("apply");
        let result = runner
            .rollback_last(&conn, &migrations, true)
            .expect("dry rollback");
        assert!(result.dry_run);
        assert_eq!(MigrationRunner::current_version(&conn).expect("v"), 2);
    }

    #[test]
    fn rollback_forward_only_fails() {
        let conn = open_mem();
        let runner = MigrationRunner::new(None);
        let migrations = sample_migrations();

        runner
            .apply_pending(&conn, &migrations, false)
            .expect("apply");
        let err = runner
            .rollback_last(&conn, &migrations, false)
            .expect_err("should fail");
        assert!(matches!(err, MigrationError::RollbackUnavailable { .. }));
    }

    #[test]
    fn rollback_on_empty_db_fails() {
        let conn = open_mem();
        let runner = MigrationRunner::new(None);
        let migrations = sample_migrations();

        let err = runner
            .rollback_last(&conn, &migrations, false)
            .expect_err("should fail");
        assert!(matches!(err, MigrationError::NothingToRollback));
    }

    #[test]
    fn version_ordering_violation_detected() {
        let conn = open_mem();
        let runner = MigrationRunner::new(None);

        let bad = vec![
            Migration::new(2, "second", "SELECT 1;"),
            Migration::new(1, "first", "SELECT 2;"),
        ];
        let err = runner
            .apply_pending(&conn, &bad, false)
            .expect_err("ordering");
        assert!(matches!(err, MigrationError::VersionOrdering { .. }));
    }

    #[test]
    fn zero_version_rejected() {
        let conn = open_mem();
        let runner = MigrationRunner::new(None);

        let bad = vec![Migration::new(0, "bad", "SELECT 1;")];
        let err = runner
            .apply_pending(&conn, &bad, false)
            .expect_err("zero version");
        assert!(matches!(err, MigrationError::InvalidVersion(0)));
    }

    #[test]
    fn lock_file_prevents_concurrent_run() {
        let dir = tempfile::tempdir().expect("tempdir");
        let lock_path = dir.path().join(".migration.lock");
        std::fs::write(&lock_path, "pid=99999").expect("write lock");

        let conn = open_mem();
        let runner = MigrationRunner::new(Some(dir.path()));
        let migrations = sample_migrations();

        let err = runner
            .apply_pending(&conn, &migrations, false)
            .expect_err("locked");
        assert!(matches!(err, MigrationError::Locked { .. }));
    }

    #[test]
    fn lock_file_cleaned_up_after_run() {
        let dir = tempfile::tempdir().expect("tempdir");
        let lock_path = dir.path().join(".migration.lock");

        let conn = open_mem();
        let runner = MigrationRunner::new(Some(dir.path()));
        let migrations = sample_migrations();

        runner
            .apply_pending(&conn, &migrations, false)
            .expect("apply");
        assert!(!lock_path.exists(), "lock file should be removed after run");
    }

    #[test]
    fn applied_migrations_returns_recorded_entries() {
        let conn = open_mem();
        let runner = MigrationRunner::new(None);
        let migrations = sample_migrations();

        runner
            .apply_pending(&conn, &migrations, false)
            .expect("apply");
        let applied = MigrationRunner::applied_migrations(&conn).expect("list");
        assert_eq!(applied.len(), 3);
        assert_eq!(applied[0].version, 1);
        assert_eq!(applied[0].description, "create users");
        assert!(!applied[0].checksum.is_empty());
    }

    #[test]
    fn bad_sql_does_not_corrupt_version() {
        let conn = open_mem();
        let runner = MigrationRunner::new(None);

        let migrations = vec![
            Migration::new(1, "good", "CREATE TABLE t1 (id INT);"),
            Migration::new(2, "bad", "THIS IS NOT VALID SQL!!!"),
        ];

        let err = runner
            .apply_pending(&conn, &migrations, false)
            .expect_err("bad sql");
        assert!(matches!(err, MigrationError::Apply { version: 2, .. }));
        // Version should be 1 — the good migration should have stuck.
        assert_eq!(MigrationRunner::current_version(&conn).expect("v"), 1);
    }

    #[test]
    fn current_version_on_empty_db() {
        let conn = open_mem();
        let version = MigrationRunner::current_version(&conn).expect("version");
        assert_eq!(version, 0);
    }
}
