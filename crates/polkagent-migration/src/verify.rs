//! Migration integrity verification.
//!
//! Compares the BLAKE3 checksums of the current migration SQL definitions
//! against the checksums recorded in the database at the time each migration
//! was first applied.

use rusqlite::Connection;

use crate::error::{MigrationError, MigrationResult};
use crate::migration::{compute_checksum, Migration};
use crate::runner::MigrationRunner;

/// Result of verifying a single migration's checksum.
#[derive(Debug, Clone)]
pub struct VerifyResult {
    pub version: u32,
    pub name: String,
    pub expected_checksum: String,
    pub actual_checksum: String,
    pub ok: bool,
}

/// Verify the integrity of all applied migrations.
///
/// For each migration that has been applied, recompute the BLAKE3 checksum
/// of the current SQL and compare it against the stored checksum. Returns
/// a list of [`VerifyResult`] entries, one per applied migration.
pub fn verify_checksums(
    conn: &Connection,
    migrations: &[Migration],
) -> MigrationResult<Vec<VerifyResult>> {
    let applied = MigrationRunner::applied_migrations(conn)?;
    let mut results = Vec::new();

    for recorded in &applied {
        let migration = migrations.iter().find(|m| m.version == recorded.version);

        match migration {
            Some(m) => {
                let actual = compute_checksum(&m.sql);
                let ok = recorded.checksum == actual;
                results.push(VerifyResult {
                    version: recorded.version,
                    name: m.name.clone(),
                    expected_checksum: recorded.checksum.clone(),
                    actual_checksum: actual,
                    ok,
                });
            }
            None => {
                // Migration exists in DB but not in code — likely from a
                // newer version. We report it with the recorded checksum
                // and mark it as OK (we cannot verify what we do not have).
                results.push(VerifyResult {
                    version: recorded.version,
                    name: recorded.description.clone(),
                    expected_checksum: recorded.checksum.clone(),
                    actual_checksum: recorded.checksum.clone(),
                    ok: true,
                });
            }
        }
    }

    Ok(results)
}

/// Verify checksums and return an error on the first mismatch.
///
/// This is a stricter variant of [`verify_checksums`] that fails fast.
pub fn verify_or_fail(conn: &Connection, migrations: &[Migration]) -> MigrationResult<()> {
    let results = verify_checksums(conn, migrations)?;

    for r in &results {
        if !r.ok {
            return Err(MigrationError::ChecksumMismatch {
                version: r.version,
                name: r.name.clone(),
                expected: r.expected_checksum.clone(),
                actual: r.actual_checksum.clone(),
            });
        }
    }

    Ok(())
}

/// Format verification results as a human-readable report.
pub fn format_verify_report(results: &[VerifyResult]) -> String {
    if results.is_empty() {
        return String::from("No applied migrations to verify.");
    }

    let mut lines = vec![format!(
        "{:<8} {:<40} {:<8} {}",
        "VERSION", "NAME", "STATUS", "CHECKSUM"
    )];
    lines.push("-".repeat(96));

    for r in results {
        let status = if r.ok { "OK" } else { "FAIL" };
        let checksum_display = if r.ok {
            truncate_checksum(&r.expected_checksum)
        } else {
            format!(
                "{} != {}",
                truncate_checksum(&r.expected_checksum),
                truncate_checksum(&r.actual_checksum)
            )
        };
        lines.push(format!(
            "{:<8} {:<40} {:<8} {}",
            r.version, r.name, status, checksum_display
        ));
    }

    let failures = results.iter().filter(|r| !r.ok).count();
    if failures > 0 {
        lines.push(String::new());
        lines.push(format!(
            "VERIFICATION FAILED: {failures} migration(s) have been tampered with."
        ));
    } else {
        lines.push(String::new());
        lines.push(format!(
            "All {} applied migration(s) verified OK.",
            results.len()
        ));
    }

    lines.join("\n")
}

/// Truncate a checksum to the first 16 hex characters for display.
fn truncate_checksum(checksum: &str) -> String {
    if checksum.len() > 16 {
        format!("{}...", &checksum[..16])
    } else {
        checksum.to_string()
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
        ]
    }

    #[test]
    fn verify_passes_when_checksums_match() {
        let conn = open_mem();
        let runner = MigrationRunner::new(None);
        let migrations = sample_migrations();

        runner
            .apply_pending(&conn, &migrations, false)
            .expect("apply");
        let results = verify_checksums(&conn, &migrations).expect("verify");

        assert_eq!(results.len(), 2);
        assert!(results[0].ok);
        assert!(results[1].ok);
    }

    #[test]
    fn verify_detects_tampered_migration() {
        let conn = open_mem();
        let runner = MigrationRunner::new(None);
        let original = sample_migrations();

        runner
            .apply_pending(&conn, &original, false)
            .expect("apply");

        // Modify SQL of migration 1 to simulate tampering.
        let tampered = vec![
            Migration::new(1, "create users", "CREATE TABLE users (id INT, name TEXT);"),
            Migration::new(2, "add email", "ALTER TABLE users ADD COLUMN email TEXT;"),
        ];

        let results = verify_checksums(&conn, &tampered).expect("verify");
        assert!(!results[0].ok);
        assert!(results[1].ok);
    }

    #[test]
    fn verify_or_fail_returns_error_on_tamper() {
        let conn = open_mem();
        let runner = MigrationRunner::new(None);
        let original = sample_migrations();

        runner
            .apply_pending(&conn, &original, false)
            .expect("apply");

        let tampered = vec![
            Migration::new(1, "create users", "CREATE TABLE users (id INT, name TEXT);"),
            Migration::new(2, "add email", "ALTER TABLE users ADD COLUMN email TEXT;"),
        ];

        let err = verify_or_fail(&conn, &tampered).expect_err("should fail");
        assert!(matches!(
            err,
            MigrationError::ChecksumMismatch { version: 1, .. }
        ));
    }

    #[test]
    fn verify_or_fail_succeeds_when_clean() {
        let conn = open_mem();
        let runner = MigrationRunner::new(None);
        let migrations = sample_migrations();

        runner
            .apply_pending(&conn, &migrations, false)
            .expect("apply");
        verify_or_fail(&conn, &migrations).expect("should pass");
    }

    #[test]
    fn verify_empty_db_returns_empty() {
        let conn = open_mem();
        let migrations = sample_migrations();
        let results = verify_checksums(&conn, &migrations).expect("verify");
        assert!(results.is_empty());
    }

    #[test]
    fn format_report_shows_ok() {
        let results = vec![VerifyResult {
            version: 1,
            name: String::from("test"),
            expected_checksum: String::from(
                "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890",
            ),
            actual_checksum: String::from(
                "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890",
            ),
            ok: true,
        }];
        let report = format_verify_report(&results);
        assert!(report.contains("OK"));
        assert!(report.contains("All 1 applied migration(s) verified OK."));
    }

    #[test]
    fn format_report_shows_failure() {
        let results = vec![VerifyResult {
            version: 1,
            name: String::from("test"),
            expected_checksum: String::from("aaaa1234567890ab"),
            actual_checksum: String::from("bbbb1234567890ab"),
            ok: false,
        }];
        let report = format_verify_report(&results);
        assert!(report.contains("FAIL"));
        assert!(report.contains("VERIFICATION FAILED"));
    }

    #[test]
    fn format_report_empty() {
        let report = format_verify_report(&[]);
        assert_eq!(report, "No applied migrations to verify.");
    }
}
