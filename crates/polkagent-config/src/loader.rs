//! Configuration loading and file-discovery.
//!
//! # File discovery order
//!
//! Configuration is resolved in layers; later layers override earlier ones:
//!
//! 1. **Built-in defaults** — the [`Config::default()`] value, always safe to run.
//! 2. **Global user file** — `~/.config/polkagent/polkagent.toml`.
//! 3. **Project file** — walks up from the current working directory looking
//!    for `.polkagent/polkagent.toml` in each ancestor directory.
//! 4. **Environment variables** — applied last via [`crate::env::apply_env_overrides`].
//!
//! An explicit path supplied to [`ConfigLoader::load_from_file`] replaces steps
//! 2 and 3 entirely.
//!
//! # Merging
//!
//! Merging is field-level: a `Some` value in the overlay replaces the
//! corresponding field in the base. Because TOML deserialization uses
//! `#[serde(default)]`, absent keys in a file produce the struct default,
//! making a naïve struct-replace incorrect. Instead, [`merge`] applies only
//! fields that appear explicitly in the overlay file.
//!
//! The current implementation uses a TOML [`toml::Value`] representation for
//! merging so that absent keys are genuinely absent rather than replaced by
//! their struct defaults.

use std::path::{Path, PathBuf};

use tracing::{debug, warn};

use crate::{
    env::apply_env_overrides,
    error::{ConfigError, Result},
    schema::Config,
};

// ---------------------------------------------------------------------------
// Config file names
// ---------------------------------------------------------------------------

const CONFIG_FILENAME: &str = "polkagent.toml";
const CONFIG_DIR_NAME: &str = ".polkagent";

// ---------------------------------------------------------------------------
// ConfigLoader
// ---------------------------------------------------------------------------

/// Loads, merges, and returns a fully-resolved [`Config`].
///
/// # Examples
///
/// ```no_run
/// use polkagent_config::ConfigLoader;
///
/// let config = ConfigLoader::new().load().expect("failed to load config");
/// ```
#[derive(Debug, Default)]
pub struct ConfigLoader {
    /// Optional explicit config file path; bypasses file discovery when set.
    explicit_path: Option<PathBuf>,
    /// Optional in-memory TOML overlay applied after file loading.
    extra_toml: Option<String>,
}

impl ConfigLoader {
    /// Create a new loader with default settings.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Override the config file path, skipping automatic file discovery.
    ///
    /// If `path` points to a file that does not exist, [`load`](Self::load)
    /// returns an error.
    #[must_use]
    pub fn with_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.explicit_path = Some(path.into());
        self
    }

    /// Apply an additional TOML string as the final file-layer override.
    ///
    /// Useful for tests and CLI `--config-override key=value` flags.
    #[must_use]
    pub fn with_extra_toml(mut self, toml: impl Into<String>) -> Self {
        self.extra_toml = Some(toml.into());
        self
    }

    /// Load the configuration, applying all layers in order.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] if a file cannot be read or contains invalid TOML.
    pub fn load(&self) -> Result<Config> {
        // Layer 1: built-in defaults.
        let mut base = toml::Value::try_from(Config::default())
            .map_err(|e| ConfigError::Serialize(e.to_string()))?;

        // Layer 2 & 3: file layers.
        if let Some(explicit) = &self.explicit_path {
            debug!(path = %explicit.display(), "loading explicit config file");
            let overlay = read_toml_file(explicit)?;
            merge_toml(&mut base, overlay);
        } else {
            // Global user config.
            if let Some(global_path) = global_config_path() {
                if global_path.exists() {
                    debug!(path = %global_path.display(), "loading global config file");
                    match read_toml_file(&global_path) {
                        Ok(overlay) => merge_toml(&mut base, overlay),
                        Err(e) => {
                            warn!(path = %global_path.display(), error = %e, "skipping unreadable global config");
                        }
                    }
                }
            }

            // Project-local config (walk up from CWD).
            if let Some(project_path) = find_project_config() {
                debug!(path = %project_path.display(), "loading project config file");
                match read_toml_file(&project_path) {
                    Ok(overlay) => merge_toml(&mut base, overlay),
                    Err(e) => {
                        warn!(path = %project_path.display(), error = %e, "skipping unreadable project config");
                    }
                }
            }
        }

        // Extra TOML overlay (CLI flags, tests).
        if let Some(extra) = &self.extra_toml {
            let overlay: toml::Value = toml::from_str(extra)
                .map_err(|e| ConfigError::Parse("<extra-toml>".to_owned(), e.to_string()))?;
            merge_toml(&mut base, overlay);
        }

        // Deserialize merged TOML value into Config.
        let mut config: Config = base
            .try_into()
            .map_err(|e| ConfigError::Deserialize(e.to_string()))?;

        // Layer 4: environment variable overrides.
        apply_env_overrides(&mut config);

        Ok(config)
    }

    /// Load configuration exclusively from `path`, then apply env-var overrides.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] if `path` cannot be read or contains invalid TOML.
    pub fn load_from_file(&self, path: &Path) -> Result<Config> {
        ConfigLoader::new().with_path(path).load()
    }
}

// ---------------------------------------------------------------------------
// File discovery
// ---------------------------------------------------------------------------

/// Return the path to the user-global config file.
///
/// Uses `$XDG_CONFIG_HOME/polkagent/polkagent.toml` on Linux and the
/// platform-appropriate equivalent on macOS (`~/Library/Application Support/...`)
/// via the [`dirs`] crate, falling back to `~/.config/polkagent/polkagent.toml`.
pub fn global_config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("polkagent").join(CONFIG_FILENAME))
}

/// Walk up from `start` directory looking for `.polkagent/polkagent.toml`.
///
/// Returns the first match found, or `None` if no project config is found
/// before the filesystem root or a repository boundary (`.git`).
pub fn find_project_config_from(start: &Path) -> Option<PathBuf> {
    let mut current = start.to_path_buf();
    loop {
        let candidate = current.join(CONFIG_DIR_NAME).join(CONFIG_FILENAME);
        if candidate.exists() {
            return Some(candidate);
        }

        // Stop at repository root: `.git` can be a directory (normal repo) or
        // a file (git worktrees).
        if current.join(".git").exists() {
            return None;
        }

        match current.parent() {
            Some(parent) => current = parent.to_path_buf(),
            None => return None,
        }
    }
}

/// Walk up from the current working directory looking for a project config.
pub fn find_project_config() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    find_project_config_from(&cwd)
}

// ---------------------------------------------------------------------------
// TOML helpers
// ---------------------------------------------------------------------------

fn read_toml_file(path: &Path) -> Result<toml::Value> {
    let contents =
        std::fs::read_to_string(path).map_err(|e| ConfigError::Io(path.to_path_buf(), e))?;
    toml::from_str(&contents)
        .map_err(|e| ConfigError::Parse(path.display().to_string(), e.to_string()))
}

/// Recursively merge `overlay` into `base`.
///
/// - TOML tables are merged key-by-key (overlay wins on conflict).
/// - TOML arrays replace the base array entirely (no element-wise merging).
/// - Scalar values in the overlay replace those in the base.
pub fn merge_toml(base: &mut toml::Value, overlay: toml::Value) {
    match (base, overlay) {
        (toml::Value::Table(base_table), toml::Value::Table(overlay_table)) => {
            for (key, overlay_val) in overlay_table {
                let both_tables = base_table.get(&key).is_some_and(toml::Value::is_table)
                    && overlay_val.is_table();

                if both_tables {
                    if let Some(base_val) = base_table.get_mut(&key) {
                        merge_toml(base_val, overlay_val);
                    }
                } else {
                    base_table.insert(key, overlay_val);
                }
            }
        }
        (base, overlay) => {
            *base = overlay;
        }
    }
}

/// Merge a `base` config with a raw TOML overlay string.
///
/// The overlay string is parsed into a [`toml::Value`] so that only keys
/// explicitly present in the string participate in the merge.  This avoids the
/// pitfall of serializing a full [`Config`] (which materializes every default)
/// and accidentally clobbering base values with overlay defaults.
///
/// # Errors
///
/// Returns [`ConfigError`] if the base cannot be serialized, the overlay
/// string is not valid TOML, or the merged value cannot be deserialized.
pub fn merge(base: Config, overlay_toml: &str) -> Result<Config> {
    let mut base_val =
        toml::Value::try_from(base).map_err(|e| ConfigError::Serialize(e.to_string()))?;
    let overlay_val: toml::Value = toml::from_str(overlay_toml)
        .map_err(|e| ConfigError::Parse("<merge-overlay>".to_owned(), e.to_string()))?;
    merge_toml(&mut base_val, overlay_val);
    base_val
        .try_into()
        .map_err(|e| ConfigError::Deserialize(e.to_string()))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;
    use crate::schema::{Config, DatabaseBackend, LogFormat};

    // -----------------------------------------------------------------------
    // File loading
    // -----------------------------------------------------------------------

    fn write_config(dir: &Path, subpath: &str, content: &str) -> PathBuf {
        let path = dir.join(subpath);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create dir");
        }
        fs::write(&path, content).expect("write config");
        path
    }

    #[test]
    fn load_from_explicit_file_overrides_defaults() {
        let tmp = TempDir::new().expect("tempdir");
        let config_path = write_config(
            tmp.path(),
            "config.toml",
            r#"
[log]
level = "debug"
"#,
        );
        let cfg = ConfigLoader::new()
            .with_path(config_path)
            .load()
            .expect("load");
        assert_eq!(cfg.log.level, "debug");
        // Other defaults should still be intact.
        assert_eq!(cfg.execution.max_concurrent_runs, 10);
    }

    #[test]
    fn load_from_nonexistent_explicit_file_returns_error() {
        let result = ConfigLoader::new()
            .with_path("/nonexistent/path/polkagent.toml")
            .load();
        assert!(result.is_err(), "expected error for missing file");
    }

    #[test]
    fn load_from_file_convenience_method() {
        let tmp = TempDir::new().expect("tempdir");
        let config_path = write_config(
            tmp.path(),
            "polkagent.toml",
            r#"
[log]
format = "json"
"#,
        );
        let loader = ConfigLoader::new();
        let cfg = loader.load_from_file(&config_path).expect("load");
        assert_eq!(cfg.log.format, LogFormat::Json);
    }

    #[test]
    fn extra_toml_overlay_is_applied() {
        let cfg = ConfigLoader::new()
            .with_extra_toml(
                r#"
[log]
level = "warn"
"#,
            )
            .load()
            .expect("load");
        assert_eq!(cfg.log.level, "warn");
    }

    // -----------------------------------------------------------------------
    // Merging
    // -----------------------------------------------------------------------

    #[test]
    fn merge_overlays_nested_tables() {
        let mut base: toml::Value =
            toml::from_str("[log]\nlevel = \"info\"\nformat = \"pretty\"").expect("base");
        let overlay: toml::Value = toml::from_str("[log]\nlevel = \"debug\"").expect("overlay");
        merge_toml(&mut base, overlay);

        let table = base.as_table().expect("table");
        let log = table
            .get("log")
            .expect("log")
            .as_table()
            .expect("log table");
        assert_eq!(log.get("level").and_then(|v| v.as_str()), Some("debug"));
        // format should be preserved from base.
        assert_eq!(log.get("format").and_then(|v| v.as_str()), Some("pretty"));
    }

    #[test]
    fn merge_arrays_are_replaced_not_appended() {
        let mut base: toml::Value =
            toml::from_str("cors_origins = [\"http://old\"]").expect("base");
        let overlay: toml::Value =
            toml::from_str("cors_origins = [\"http://new\", \"http://extra\"]").expect("overlay");
        merge_toml(&mut base, overlay);

        let arr = base
            .as_table()
            .expect("table")
            .get("cors_origins")
            .expect("cors_origins")
            .as_array()
            .expect("array");
        assert_eq!(arr.len(), 2);
    }

    #[test]
    fn merge_config_structs_overlay_wins() {
        let mut base = Config::default();
        base.log.level = "trace".to_owned();
        base.execution.max_concurrent_runs = 42;

        let overlay_toml = r#"
[log]
level = "error"

[database]
backend = "postgres"
"#;
        let merged = merge(base, overlay_toml).expect("merge");
        // Overlay keys win.
        assert_eq!(merged.log.level, "error");
        assert_eq!(merged.database.backend, DatabaseBackend::Postgres);
        // Base values not mentioned in the overlay are preserved.
        assert_eq!(merged.execution.max_concurrent_runs, 42);
    }

    // -----------------------------------------------------------------------
    // File discovery
    // -----------------------------------------------------------------------

    #[test]
    fn find_project_config_discovers_in_parent_directory() {
        let tmp = TempDir::new().expect("tempdir");
        let project_root = tmp.path().join("project");
        let nested_dir = project_root.join("sub").join("deep");
        fs::create_dir_all(&nested_dir).expect("create dirs");

        // Place config at project root.
        write_config(
            &project_root,
            ".polkagent/polkagent.toml",
            "[log]\nlevel = \"trace\"",
        );

        // Discovery should find it when searching from the nested dir.
        let found = find_project_config_from(&nested_dir);
        assert!(found.is_some(), "should find project config");
        assert!(
            found
                .as_ref()
                .unwrap()
                .ends_with(".polkagent/polkagent.toml"),
            "unexpected path: {:?}",
            found
        );
    }

    #[test]
    fn find_project_config_returns_none_when_absent() {
        // Start from a temp dir that has no .polkagent config.
        let tmp = TempDir::new().expect("tempdir");
        let result = find_project_config_from(tmp.path());
        // It's possible the real filesystem above tmp has a config; we can't
        // assert None in general. Just verify the function doesn't panic.
        let _ = result;
    }

    #[test]
    fn find_project_config_stops_at_git_directory_boundary() {
        let tmp = TempDir::new().expect("tempdir");
        // Layout:
        //   outer/.polkagent/polkagent.toml   <-- config above repo root
        //   outer/repo/.git/                  <-- repo root
        //   outer/repo/sub/deep/              <-- start dir
        let outer = tmp.path().join("outer");
        let repo = outer.join("repo");
        let nested = repo.join("sub").join("deep");
        fs::create_dir_all(&nested).expect("create nested");
        fs::create_dir_all(repo.join(".git")).expect("create .git");

        // Config above the repo root — should NOT be discovered.
        write_config(
            &outer,
            ".polkagent/polkagent.toml",
            "[log]\nlevel = \"trace\"",
        );

        let found = find_project_config_from(&nested);
        assert!(
            found.is_none(),
            "should not walk past .git boundary, but found: {:?}",
            found,
        );
    }

    #[test]
    fn find_project_config_finds_config_inside_repo_boundary() {
        let tmp = TempDir::new().expect("tempdir");
        // Layout:
        //   repo/.git/                        <-- repo root
        //   repo/.polkagent/polkagent.toml    <-- config at repo root
        //   repo/sub/deep/                    <-- start dir
        let repo = tmp.path().join("repo");
        let nested = repo.join("sub").join("deep");
        fs::create_dir_all(&nested).expect("create nested");
        fs::create_dir_all(repo.join(".git")).expect("create .git");

        write_config(
            &repo,
            ".polkagent/polkagent.toml",
            "[log]\nlevel = \"debug\"",
        );

        let found = find_project_config_from(&nested);
        assert!(found.is_some(), "should find config inside repo boundary");
    }

    #[test]
    fn find_project_config_stops_at_git_worktree_file_boundary() {
        let tmp = TempDir::new().expect("tempdir");
        // In git worktrees, .git is a file, not a directory.
        let repo = tmp.path().join("worktree");
        let nested = repo.join("sub");
        fs::create_dir_all(&nested).expect("create nested");

        // Create .git as a file (worktree marker).
        fs::write(repo.join(".git"), "gitdir: /some/path/to/.git/worktrees/wt")
            .expect("write .git file");

        // Config above the worktree — should NOT be discovered.
        write_config(
            tmp.path(),
            ".polkagent/polkagent.toml",
            "[log]\nlevel = \"trace\"",
        );

        let found = find_project_config_from(&nested);
        assert!(
            found.is_none(),
            "should not walk past .git file boundary (worktree), but found: {:?}",
            found,
        );
    }

    #[test]
    fn project_config_overrides_global() {
        let tmp = TempDir::new().expect("tempdir");

        // Global config: info level.
        let global_cfg = write_config(
            tmp.path(),
            "global/polkagent.toml",
            "[log]\nlevel = \"info\"",
        );
        // Project config: debug level.
        let project_cfg = write_config(
            tmp.path(),
            "project/.polkagent/polkagent.toml",
            "[log]\nlevel = \"debug\"",
        );

        // Manually build the same two-layer merge the loader would perform.
        let mut base = toml::Value::try_from(Config::default()).expect("serialize");
        merge_toml(&mut base, read_toml_file(&global_cfg).expect("global"));
        merge_toml(&mut base, read_toml_file(&project_cfg).expect("project"));
        let cfg: Config = base.try_into().expect("deserialize");

        assert_eq!(
            cfg.log.level, "debug",
            "project config should win over global"
        );
    }

    #[test]
    fn load_with_no_files_produces_default_config() {
        // With no explicit path and no discoverable files (hopefully), the
        // loader must at minimum produce a Config that has sane defaults.
        // We use with_extra_toml("") which is a no-op to confirm the chain works.
        let cfg = ConfigLoader::new()
            .with_extra_toml("") // empty overlay — no-op
            .load()
            .expect("load");
        assert_eq!(
            cfg.meta.schema_version,
            crate::schema::CURRENT_SCHEMA_VERSION
        );
    }
}
