//! Migration status reporting.
//!
//! Lists applied and pending migrations with their current state.

use rusqlite::Connection;

use crate::error::MigrationResult;
use crate::migration::{compute_checksum, Migration, MigrationState};
use crate::runner::MigrationRunner;

/// Status entry for a single migration.
#[derive(Debug, Clone)]
pub struct MigrationStatusEntry {
    /// The migration version number.
    pub version: u32,
    /// Human-readable name / description.
    pub name: String,
    /// Whether the migration is pending, applied, or tampered.
    pub state: MigrationState,
    /// When the migration was applied (if ever).
    pub applied_at: Option<String>,
    /// Whether rollback is available.
    pub reversible: bool,
}

/// Report on the status of all known migrations against the database.
pub struct MigrationStatus;

impl MigrationStatus {
    /// Gather status for every defined migration.
    ///
    /// Returns one [`MigrationStatusEntry`] per migration, in version order.
    /// Migrations that exist in the database but not in the provided list are
    /// skipped (they may be from a newer version of the application).
    pub fn gather(
        conn: &Connection,
        migrations: &[Migration],
    ) -> MigrationResult<Vec<MigrationStatusEntry>> {
        let applied = MigrationRunner::applied_migrations(conn)?;

        let mut entries = Vec::with_capacity(migrations.len());

        for m in migrations {
            let recorded = applied.iter().find(|a| a.version == m.version);

            let (state, applied_at) = match recorded {
                Some(rec) => {
                    let current_checksum = compute_checksum(&m.sql);
                    if rec.checksum == current_checksum {
                        (MigrationState::Applied, Some(rec.applied_at.clone()))
                    } else {
                        (MigrationState::Tampered, Some(rec.applied_at.clone()))
                    }
                }
                None => (MigrationState::Pending, None),
            };

            entries.push(MigrationStatusEntry {
                version: m.version,
                name: m.name.clone(),
                state,
                applied_at,
                reversible: m.is_reversible(),
            });
        }

        Ok(entries)
    }

    /// Format the status entries as a human-readable table.
    pub fn format_table(entries: &[MigrationStatusEntry]) -> String {
        if entries.is_empty() {
            return String::from("No migrations defined.");
        }

        // Column headers.
        let mut lines = vec![format!(
            "{:<8} {:<40} {:<10} {:<28} {}",
            "VERSION", "NAME", "STATUS", "APPLIED AT", "REVERSIBLE"
        )];
        lines.push("-".repeat(96));

        for entry in entries {
            let applied = entry.applied_at.as_deref().unwrap_or("-");
            let reversible = if entry.reversible { "yes" } else { "no" };
            lines.push(format!(
                "{:<8} {:<40} {:<10} {:<28} {}",
                entry.version, entry.name, entry.state, applied, reversible
            ));
        }

        lines.join("\n")
    }

    /// Count migrations by state.
    pub fn summary(entries: &[MigrationStatusEntry]) -> StatusSummary {
        let mut summary = StatusSummary::default();
        for entry in entries {
            match entry.state {
                MigrationState::Pending => summary.pending += 1,
                MigrationState::Applied => summary.applied += 1,
                MigrationState::Tampered => summary.tampered += 1,
            }
        }
        summary.total = entries.len();
        summary
    }
}

/// Summary counts for a migration status report.
#[derive(Debug, Default, Clone)]
pub struct StatusSummary {
    pub total: usize,
    pub applied: usize,
    pub pending: usize,
    pub tampered: usize,
}

impl std::fmt::Display for StatusSummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Total: {} | Applied: {} | Pending: {} | Tampered: {}",
            self.total, self.applied, self.pending, self.tampered
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::MigrationRunner;
    use rusqlite::Connection;

    fn open_mem() -> Connection {
        Connection::open_in_memory().expect("in-memory db")
    }

    fn sample_migrations() -> Vec<Migration> {
        vec![
            Migration::new(
                1,
                "create users",
                "CREATE TABLE users (id INTEGER PRIMARY KEY);",
            ),
            Migration::new(2, "add email", "ALTER TABLE users ADD COLUMN email TEXT;"),
            Migration::new_reversible(
                3,
                "create posts",
                "CREATE TABLE posts (id INTEGER PRIMARY KEY, body TEXT);",
                "DROP TABLE posts;",
            ),
        ]
    }

    #[test]
    fn status_all_pending_on_fresh_db() {
        let conn = open_mem();
        let migrations = sample_migrations();
        let entries = MigrationStatus::gather(&conn, &migrations).expect("gather");

        assert_eq!(entries.len(), 3);
        for entry in &entries {
            assert_eq!(entry.state, MigrationState::Pending);
            assert!(entry.applied_at.is_none());
        }
    }

    #[test]
    fn status_all_applied_after_run() {
        let conn = open_mem();
        let runner = MigrationRunner::new(None);
        let migrations = sample_migrations();

        runner
            .apply_pending(&conn, &migrations, false)
            .expect("apply");
        let entries = MigrationStatus::gather(&conn, &migrations).expect("gather");

        assert_eq!(entries.len(), 3);
        for entry in &entries {
            assert_eq!(entry.state, MigrationState::Applied);
            assert!(entry.applied_at.is_some());
        }
    }

    #[test]
    fn status_detects_tampered_migration() {
        let conn = open_mem();
        let runner = MigrationRunner::new(None);
        let original = sample_migrations();

        runner
            .apply_pending(&conn, &original, false)
            .expect("apply");

        // Modify the SQL of migration 1 to simulate tampering.
        let mut tampered = sample_migrations();
        tampered[0] = Migration::new(1, "create users", "CREATE TABLE users (id INT, name TEXT);");

        let entries = MigrationStatus::gather(&conn, &tampered).expect("gather");
        assert_eq!(entries[0].state, MigrationState::Tampered);
        assert_eq!(entries[1].state, MigrationState::Applied);
    }

    #[test]
    fn status_shows_reversible_flag() {
        let conn = open_mem();
        let migrations = sample_migrations();
        let entries = MigrationStatus::gather(&conn, &migrations).expect("gather");

        assert!(!entries[0].reversible);
        assert!(!entries[1].reversible);
        assert!(entries[2].reversible);
    }

    #[test]
    fn summary_counts_correct() {
        let conn = open_mem();
        let runner = MigrationRunner::new(None);
        let migrations = sample_migrations();

        // Apply only the first migration.
        runner
            .apply_pending(&conn, &migrations[..1], false)
            .expect("apply");
        let entries = MigrationStatus::gather(&conn, &migrations).expect("gather");
        let summary = MigrationStatus::summary(&entries);

        assert_eq!(summary.total, 3);
        assert_eq!(summary.applied, 1);
        assert_eq!(summary.pending, 2);
        assert_eq!(summary.tampered, 0);
    }

    #[test]
    fn format_table_empty() {
        let table = MigrationStatus::format_table(&[]);
        assert_eq!(table, "No migrations defined.");
    }

    #[test]
    fn format_table_includes_headers() {
        let conn = open_mem();
        let migrations = sample_migrations();
        let entries = MigrationStatus::gather(&conn, &migrations).expect("gather");
        let table = MigrationStatus::format_table(&entries);

        assert!(table.contains("VERSION"));
        assert!(table.contains("NAME"));
        assert!(table.contains("STATUS"));
        assert!(table.contains("pending"));
    }

    #[test]
    fn summary_display_format() {
        let summary = StatusSummary {
            total: 5,
            applied: 3,
            pending: 1,
            tampered: 1,
        };
        let display = summary.to_string();
        assert!(display.contains("Total: 5"));
        assert!(display.contains("Applied: 3"));
        assert!(display.contains("Pending: 1"));
        assert!(display.contains("Tampered: 1"));
    }
}
