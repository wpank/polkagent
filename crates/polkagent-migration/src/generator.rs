//! Migration file generator.
//!
//! Generates new migration SQL files with incrementing version numbers
//! and optional down.sql files for rollback support.

use std::path::{Path, PathBuf};

use chrono::Utc;

use crate::error::{MigrationError, MigrationResult};

/// Result of generating a new migration file.
#[derive(Debug, Clone)]
pub struct GenerateResult {
    /// The version number assigned to the new migration.
    pub version: u32,
    /// The sanitized migration name.
    pub name: String,
    /// Path to the generated up.sql file.
    pub up_path: PathBuf,
    /// Path to the generated down.sql file (if `reversible` was true).
    pub down_path: Option<PathBuf>,
}

/// Generate a new migration file.
///
/// Creates a directory under `migrations_dir` with the structure:
/// ```text
/// migrations_dir/
///   V{version}__{name}/
///     up.sql
///     down.sql   (if reversible)
/// ```
///
/// The `next_version` is typically `current_max_version + 1`.
pub fn generate(
    migrations_dir: &Path,
    next_version: u32,
    name: &str,
    reversible: bool,
) -> MigrationResult<GenerateResult> {
    let sanitized = sanitize_name(name);
    let dir_name = format!("V{next_version}__{sanitized}");
    let dir_path = migrations_dir.join(&dir_name);

    std::fs::create_dir_all(&dir_path).map_err(|e| MigrationError::Io {
        path: dir_path.clone(),
        source: e,
    })?;

    let up_path = dir_path.join("up.sql");
    let up_content = generate_up_template(next_version, &sanitized);

    std::fs::write(&up_path, up_content).map_err(|e| MigrationError::Io {
        path: up_path.clone(),
        source: e,
    })?;

    let down_path = if reversible {
        let path = dir_path.join("down.sql");
        let down_content = generate_down_template(next_version, &sanitized);
        std::fs::write(&path, down_content).map_err(|e| MigrationError::Io {
            path: path.clone(),
            source: e,
        })?;
        Some(path)
    } else {
        None
    };

    Ok(GenerateResult {
        version: next_version,
        name: sanitized,
        up_path,
        down_path,
    })
}

/// Determine the next version number by scanning existing migration directories.
///
/// Looks for directories matching `V{n}__*` and returns `max(n) + 1`, or `1`
/// if no migrations exist yet.
pub fn next_version_from_dir(migrations_dir: &Path) -> MigrationResult<u32> {
    if !migrations_dir.exists() {
        return Ok(1);
    }

    let mut max_version = 0u32;

    let entries = std::fs::read_dir(migrations_dir).map_err(|e| MigrationError::Io {
        path: migrations_dir.to_path_buf(),
        source: e,
    })?;

    for entry in entries {
        let entry = entry.map_err(|e| MigrationError::Io {
            path: migrations_dir.to_path_buf(),
            source: e,
        })?;

        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        if let Some(version) = parse_version_from_dirname(&name_str) {
            if version > max_version {
                max_version = version;
            }
        }
    }

    Ok(max_version + 1)
}

/// Parse the version number from a directory name like `V3__add_users`.
pub fn parse_version_from_dirname(dirname: &str) -> Option<u32> {
    let stripped = dirname.strip_prefix('V')?;
    let underscore_pos = stripped.find("__")?;
    stripped[..underscore_pos].parse().ok()
}

/// Sanitize a migration name for use as a directory component.
///
/// Replaces spaces and non-alphanumeric characters with underscores,
/// converts to lowercase, and trims leading/trailing underscores.
pub fn sanitize_name(name: &str) -> String {
    let sanitized: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();

    sanitized.trim_matches('_').to_string()
}

fn generate_up_template(version: u32, name: &str) -> String {
    let now = Utc::now().format("%Y-%m-%d");
    format!(
        "-- Migration V{version}: {name}\n\
         -- Created: {now}\n\
         --\n\
         -- Write your forward (up) SQL here.\n\
         -- Each statement must end with a semicolon.\n\
         \n"
    )
}

fn generate_down_template(version: u32, name: &str) -> String {
    let now = Utc::now().format("%Y-%m-%d");
    format!(
        "-- Rollback for V{version}: {name}\n\
         -- Created: {now}\n\
         --\n\
         -- Write your reverse (down) SQL here.\n\
         -- This should undo everything in up.sql.\n\
         \n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_name_lowercases() {
        assert_eq!(sanitize_name("AddUsers"), "addusers");
    }

    #[test]
    fn sanitize_name_replaces_spaces() {
        assert_eq!(sanitize_name("add users table"), "add_users_table");
    }

    #[test]
    fn sanitize_name_replaces_special_chars() {
        assert_eq!(sanitize_name("add-users.table!"), "add_users_table");
    }

    #[test]
    fn sanitize_name_trims_underscores() {
        assert_eq!(sanitize_name("__add_users__"), "add_users");
    }

    #[test]
    fn parse_version_valid() {
        assert_eq!(parse_version_from_dirname("V1__create_users"), Some(1));
        assert_eq!(parse_version_from_dirname("V42__add_email"), Some(42));
        assert_eq!(parse_version_from_dirname("V100__big_change"), Some(100));
    }

    #[test]
    fn parse_version_invalid() {
        assert_eq!(parse_version_from_dirname("not_a_migration"), None);
        assert_eq!(parse_version_from_dirname("V__no_number"), None);
        assert_eq!(parse_version_from_dirname("Vabc__bad"), None);
    }

    #[test]
    fn generate_creates_up_sql() {
        let dir = tempfile::tempdir().expect("tempdir");
        let result = generate(dir.path(), 1, "create users", false).expect("generate");

        assert_eq!(result.version, 1);
        assert_eq!(result.name, "create_users");
        assert!(result.up_path.exists());
        assert!(result.down_path.is_none());

        let content = std::fs::read_to_string(&result.up_path).expect("read");
        assert!(content.contains("Migration V1"));
        assert!(content.contains("create_users"));
    }

    #[test]
    fn generate_reversible_creates_both_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let result = generate(dir.path(), 2, "add posts", true).expect("generate");

        assert!(result.up_path.exists());
        assert!(result.down_path.is_some());
        let down = result.down_path.expect("down path");
        assert!(down.exists());

        let down_content = std::fs::read_to_string(down).expect("read down");
        assert!(down_content.contains("Rollback for V2"));
    }

    #[test]
    fn next_version_empty_dir() {
        let dir = tempfile::tempdir().expect("tempdir");
        let next = next_version_from_dir(dir.path()).expect("next");
        assert_eq!(next, 1);
    }

    #[test]
    fn next_version_with_existing_migrations() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(dir.path().join("V1__first")).expect("mkdir");
        std::fs::create_dir(dir.path().join("V3__third")).expect("mkdir");
        std::fs::create_dir(dir.path().join("V2__second")).expect("mkdir");

        let next = next_version_from_dir(dir.path()).expect("next");
        assert_eq!(next, 4);
    }

    #[test]
    fn next_version_nonexistent_dir() {
        let path = std::path::Path::new("/tmp/nonexistent_migration_dir_test_12345");
        let next = next_version_from_dir(path).expect("next");
        assert_eq!(next, 1);
    }

    #[test]
    fn generate_directory_structure() {
        let dir = tempfile::tempdir().expect("tempdir");
        let result = generate(dir.path(), 5, "big change", false).expect("generate");

        let expected_dir = dir.path().join("V5__big_change");
        assert!(expected_dir.is_dir());
        assert_eq!(result.up_path, expected_dir.join("up.sql"));
    }
}
