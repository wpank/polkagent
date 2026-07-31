//! Skill loader: discovers and loads skill manifests from the filesystem.
//!
//! The loader searches well-known directories for skill packages and parses
//! their `skill.toml` manifests. Search paths follow a precedence order:
//!
//! 1. **Local project skills** `.polkagent/skills/` (relative to CWD)
//! 2. **User-level skills** `~/.polkagent/skills/`
//! 3. **Built-in skills** (compiled into the binary, if any)
//!
//! Each skill is a directory containing at minimum a `skill.toml` file.

use std::path::{Path, PathBuf};

use tracing::{debug, info, warn};

use crate::error::SkillError;
use crate::manifest::SkillManifest;

/// The conventional manifest filename inside a skill directory.
pub const MANIFEST_FILENAME: &str = "skill.toml";

/// Loads skill manifests from the filesystem.
#[derive(Debug)]
pub struct SkillLoader {
    /// Ordered list of directories to search for skill packages.
    search_paths: Vec<PathBuf>,
}

impl SkillLoader {
    /// Create a loader with explicit search paths.
    pub fn new(search_paths: Vec<PathBuf>) -> Self {
        Self { search_paths }
    }

    /// Create a loader with the default search paths:
    ///
    /// 1. `.polkagent/skills/` (relative to CWD)
    /// 2. `~/.polkagent/skills/`
    pub fn with_default_paths() -> Self {
        let mut paths = Vec::new();

        // Local project skills
        paths.push(PathBuf::from(".polkagent/skills"));

        // User-level skills
        if let Some(home) = dirs::home_dir() {
            paths.push(home.join(".polkagent/skills"));
        }

        Self {
            search_paths: paths,
        }
    }

    /// Return the configured search paths.
    pub fn search_paths(&self) -> &[PathBuf] {
        &self.search_paths
    }

    /// Load a single skill manifest from a directory.
    ///
    /// The directory must contain a `skill.toml` file.
    pub fn load_from_dir(path: &Path) -> Result<SkillManifest, SkillError> {
        let manifest_path = path.join(MANIFEST_FILENAME);
        debug!(path = %manifest_path.display(), "loading skill manifest");

        let content = std::fs::read_to_string(&manifest_path)
            .map_err(|e| SkillError::io(&manifest_path, e))?;

        Self::load_from_toml(&content)
    }

    /// Parse a skill manifest from a TOML string.
    pub fn load_from_toml(content: &str) -> Result<SkillManifest, SkillError> {
        SkillManifest::from_toml(content)
    }

    /// Discover all skills across the configured search paths.
    ///
    /// Each immediate subdirectory of each search path is treated as a
    /// potential skill directory. Directories without a valid `skill.toml`
    /// are silently skipped (with a warning log).
    ///
    /// Skills from earlier search paths take precedence when multiple skills
    /// share the same name: the first occurrence wins.
    pub fn discover_skills(&self) -> Vec<SkillManifest> {
        let mut manifests = Vec::new();
        let mut seen_names = std::collections::HashSet::new();

        for search_path in &self.search_paths {
            if !search_path.is_dir() {
                debug!(
                    path = %search_path.display(),
                    "search path does not exist or is not a directory, skipping"
                );
                continue;
            }

            let entries = match std::fs::read_dir(search_path) {
                Ok(entries) => entries,
                Err(e) => {
                    warn!(
                        path = %search_path.display(),
                        error = %e,
                        "failed to read skill search directory"
                    );
                    continue;
                }
            };

            for entry in entries {
                let entry = match entry {
                    Ok(e) => e,
                    Err(e) => {
                        warn!(error = %e, "failed to read directory entry");
                        continue;
                    }
                };

                let entry_path = entry.path();
                if !entry_path.is_dir() {
                    continue;
                }

                match Self::load_from_dir(&entry_path) {
                    Ok(manifest) => {
                        let name = manifest.skill.name.clone();
                        if seen_names.contains(&name) {
                            debug!(
                                skill = %name,
                                path = %entry_path.display(),
                                "duplicate skill name, skipping (earlier occurrence takes precedence)"
                            );
                        } else {
                            info!(
                                skill = %name,
                                version = %manifest.skill.version,
                                path = %entry_path.display(),
                                "discovered skill"
                            );
                            seen_names.insert(name);
                            manifests.push(manifest);
                        }
                    }
                    Err(e) => {
                        debug!(
                            path = %entry_path.display(),
                            error = %e,
                            "skipping directory (not a valid skill)"
                        );
                    }
                }
            }
        }

        manifests
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_TOML: &str = r#"
[skill]
name = "test-skill"
version = "0.1.0"
description = "A test skill"
"#;

    #[test]
    fn load_from_toml_success() {
        let manifest = SkillLoader::load_from_toml(SAMPLE_TOML).expect("should parse");
        assert_eq!(manifest.skill.name, "test-skill");
        assert_eq!(manifest.skill.version, "0.1.0");
    }

    #[test]
    fn load_from_toml_invalid() {
        let result = SkillLoader::load_from_toml("not valid toml {{{");
        assert!(result.is_err());
    }

    #[test]
    fn load_from_dir_success() {
        let dir = tempfile::tempdir().expect("should create temp dir");
        let manifest_path = dir.path().join(MANIFEST_FILENAME);
        std::fs::write(&manifest_path, SAMPLE_TOML).expect("should write");

        let manifest = SkillLoader::load_from_dir(dir.path()).expect("should load");
        assert_eq!(manifest.skill.name, "test-skill");
    }

    #[test]
    fn load_from_dir_missing_manifest() {
        let dir = tempfile::tempdir().expect("should create temp dir");
        let result = SkillLoader::load_from_dir(dir.path());
        assert!(result.is_err());
    }

    #[test]
    fn discover_skills_finds_valid_skills() {
        let root = tempfile::tempdir().expect("should create temp dir");

        // Create two valid skill directories
        let skill_a = root.path().join("skill-a");
        std::fs::create_dir(&skill_a).expect("should create dir");
        std::fs::write(
            skill_a.join(MANIFEST_FILENAME),
            r#"
[skill]
name = "skill-a"
version = "1.0.0"
"#,
        )
        .expect("should write");

        let skill_b = root.path().join("skill-b");
        std::fs::create_dir(&skill_b).expect("should create dir");
        std::fs::write(
            skill_b.join(MANIFEST_FILENAME),
            r#"
[skill]
name = "skill-b"
version = "2.0.0"
"#,
        )
        .expect("should write");

        // Create an invalid directory (no manifest)
        let invalid = root.path().join("not-a-skill");
        std::fs::create_dir(&invalid).expect("should create dir");

        let loader = SkillLoader::new(vec![root.path().to_path_buf()]);
        let skills = loader.discover_skills();

        assert_eq!(skills.len(), 2);
        let names: Vec<&str> = skills.iter().map(|s| s.skill.name.as_str()).collect();
        assert!(names.contains(&"skill-a"));
        assert!(names.contains(&"skill-b"));
    }

    #[test]
    fn discover_skills_skips_nonexistent_paths() {
        let loader = SkillLoader::new(vec![PathBuf::from("/nonexistent/path/12345")]);
        let skills = loader.discover_skills();
        assert!(skills.is_empty());
    }

    #[test]
    fn discover_skills_precedence() {
        let first = tempfile::tempdir().expect("should create temp dir");
        let second = tempfile::tempdir().expect("should create temp dir");

        // Same skill name in both directories
        let skill_first = first.path().join("dup");
        std::fs::create_dir(&skill_first).expect("should create dir");
        std::fs::write(
            skill_first.join(MANIFEST_FILENAME),
            r#"
[skill]
name = "dup-skill"
version = "1.0.0"
description = "first"
"#,
        )
        .expect("should write");

        let skill_second = second.path().join("dup");
        std::fs::create_dir(&skill_second).expect("should create dir");
        std::fs::write(
            skill_second.join(MANIFEST_FILENAME),
            r#"
[skill]
name = "dup-skill"
version = "2.0.0"
description = "second"
"#,
        )
        .expect("should write");

        let loader = SkillLoader::new(vec![
            first.path().to_path_buf(),
            second.path().to_path_buf(),
        ]);
        let skills = loader.discover_skills();

        assert_eq!(skills.len(), 1);
        // First occurrence (version 1.0.0) should win.
        assert_eq!(skills[0].skill.version, "1.0.0");
        assert_eq!(skills[0].skill.description, "first");
    }

    #[test]
    fn default_paths_construction() {
        let loader = SkillLoader::with_default_paths();
        // Should have at least the local project path.
        assert!(!loader.search_paths().is_empty());
        assert!(loader
            .search_paths()
            .iter()
            .any(|p| p.ends_with(".polkagent/skills")));
    }

    #[test]
    fn discover_skips_files_in_search_dir() {
        let root = tempfile::tempdir().expect("should create temp dir");

        // A file (not a directory) should be ignored.
        std::fs::write(root.path().join("readme.txt"), "hello").expect("should write");

        let loader = SkillLoader::new(vec![root.path().to_path_buf()]);
        let skills = loader.discover_skills();
        assert!(skills.is_empty());
    }
}
