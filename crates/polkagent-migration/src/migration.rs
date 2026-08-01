//! Core migration data types.
//!
//! A [`Migration`] represents a single schema change: a numbered SQL batch
//! with an optional reverse (down) SQL for rollback support.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A database migration definition.
///
/// Migrations are ordered by [`version`](Self::version) (1-based, monotonic).
/// Each migration carries a BLAKE3 checksum of its SQL so that tampering can
/// be detected after initial application.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Migration {
    /// 1-based monotonically increasing version number.
    pub version: u32,

    /// Human-readable description of the migration.
    pub name: String,

    /// Forward (up) SQL to apply the migration.
    pub sql: String,

    /// Optional reverse (down) SQL for rollback support.
    /// If `None`, the migration is forward-only and cannot be rolled back.
    pub down_sql: Option<String>,

    /// When this migration was applied to the database, or `None` if it is
    /// still pending.
    pub applied_at: Option<DateTime<Utc>>,

    /// BLAKE3 hex-encoded checksum of the forward SQL.
    pub checksum: String,
}

impl Migration {
    /// Create a new migration definition (not yet applied).
    pub fn new(version: u32, name: impl Into<String>, sql: impl Into<String>) -> Self {
        let sql = sql.into();
        let checksum = compute_checksum(&sql);
        Self {
            version,
            name: name.into(),
            sql,
            down_sql: None,
            applied_at: None,
            checksum,
        }
    }

    /// Create a new reversible migration definition.
    pub fn new_reversible(
        version: u32,
        name: impl Into<String>,
        sql: impl Into<String>,
        down_sql: impl Into<String>,
    ) -> Self {
        let sql = sql.into();
        let checksum = compute_checksum(&sql);
        Self {
            version,
            name: name.into(),
            sql,
            down_sql: Some(down_sql.into()),
            applied_at: None,
            checksum,
        }
    }

    /// Returns `true` if this migration has been applied.
    pub fn is_applied(&self) -> bool {
        self.applied_at.is_some()
    }

    /// Returns `true` if this migration supports rollback.
    pub fn is_reversible(&self) -> bool {
        self.down_sql.is_some()
    }

    /// Recompute the checksum of the forward SQL and return it.
    pub fn recompute_checksum(&self) -> String {
        compute_checksum(&self.sql)
    }
}

/// Compute a BLAKE3 hex-encoded checksum of the given SQL string.
pub fn compute_checksum(sql: &str) -> String {
    blake3::hash(sql.as_bytes()).to_hex().to_string()
}

/// Status of a migration: pending, applied, or tampered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MigrationState {
    /// The migration has not yet been applied.
    Pending,
    /// The migration was applied and its checksum matches.
    Applied,
    /// The migration was applied but its checksum no longer matches
    /// the current SQL content.
    Tampered,
}

impl std::fmt::Display for MigrationState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pending => write!(f, "pending"),
            Self::Applied => write!(f, "applied"),
            Self::Tampered => write!(f, "TAMPERED"),
        }
    }
}

/// A row from the `schema_migrations` table, as recorded in the database.
#[derive(Debug, Clone)]
pub struct AppliedMigration {
    pub version: u32,
    pub description: String,
    pub applied_at: String,
    pub checksum: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_migration_computes_checksum() {
        let m = Migration::new(1, "test", "CREATE TABLE t (id INT);");
        assert!(!m.checksum.is_empty());
        assert_eq!(m.checksum.len(), 64); // BLAKE3 hex is 64 chars
    }

    #[test]
    fn checksum_is_deterministic() {
        let sql = "CREATE TABLE foo (bar TEXT);";
        let c1 = compute_checksum(sql);
        let c2 = compute_checksum(sql);
        assert_eq!(c1, c2);
    }

    #[test]
    fn different_sql_different_checksum() {
        let c1 = compute_checksum("CREATE TABLE a (id INT);");
        let c2 = compute_checksum("CREATE TABLE b (id INT);");
        assert_ne!(c1, c2);
    }

    #[test]
    fn recompute_checksum_matches_stored() {
        let m = Migration::new(1, "test", "SELECT 1;");
        assert_eq!(m.checksum, m.recompute_checksum());
    }

    #[test]
    fn new_migration_is_not_applied() {
        let m = Migration::new(1, "test", "SELECT 1;");
        assert!(!m.is_applied());
    }

    #[test]
    fn forward_only_is_not_reversible() {
        let m = Migration::new(1, "test", "SELECT 1;");
        assert!(!m.is_reversible());
    }

    #[test]
    fn reversible_migration_has_down_sql() {
        let m = Migration::new_reversible(1, "test", "CREATE TABLE t (id INT);", "DROP TABLE t;");
        assert!(m.is_reversible());
        assert_eq!(m.down_sql.as_deref(), Some("DROP TABLE t;"));
    }

    #[test]
    fn migration_state_display() {
        assert_eq!(MigrationState::Pending.to_string(), "pending");
        assert_eq!(MigrationState::Applied.to_string(), "applied");
        assert_eq!(MigrationState::Tampered.to_string(), "TAMPERED");
    }

    #[test]
    fn migration_serialization_roundtrip() {
        let m = Migration::new(1, "test migration", "CREATE TABLE t (id INT);");
        let json = serde_json::to_string(&m).expect("serialize");
        let m2: Migration = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(m.version, m2.version);
        assert_eq!(m.name, m2.name);
        assert_eq!(m.sql, m2.sql);
        assert_eq!(m.checksum, m2.checksum);
    }
}
