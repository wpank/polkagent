//! Skill runner: prepares a skill for execution.
//!
//! The runner takes a [`SkillManifest`] and a [`ToolRegistry`] and produces
//! a [`PreparedSkill`] that contains everything needed to start executing
//! the skill: the manifest, the assembled system prompt, and the list of
//! available tool names (verified against the registry).
//!
//! The `ToolRegistry` trait is defined here as a minimal interface to
//! decouple the skill crate from the full tool system. Once `polkagent-tool`
//! is available, this trait can be implemented by its registry type.

use std::collections::HashSet;

use tracing::{debug, info};

use crate::error::SkillError;
use crate::manifest::SkillManifest;

// ---------------------------------------------------------------------------
// ToolRegistry trait (minimal interface)
// ---------------------------------------------------------------------------

/// A minimal interface for checking tool availability.
///
/// This trait exists to decouple the skill crate from the full tool system
/// (`polkagent-tool`). Any tool registry implementation can satisfy this
/// contract by reporting which tool names are available.
pub trait ToolRegistry {
    /// Return the set of all tool names currently registered.
    fn available_tools(&self) -> HashSet<String>;

    /// Check whether a specific tool is registered.
    fn has_tool(&self, name: &str) -> bool {
        self.available_tools().contains(name)
    }
}

// ---------------------------------------------------------------------------
// PreparedSkill
// ---------------------------------------------------------------------------

/// A skill that has been validated and prepared for execution.
///
/// All tool references have been checked against the registry, and the
/// system prompt has been assembled.
#[derive(Debug, Clone)]
pub struct PreparedSkill {
    /// The original manifest.
    pub manifest: SkillManifest,
    /// The system prompt to inject when the skill is activated.
    pub system_prompt: String,
    /// The tool names available to this skill (subset of registry tools
    /// that the manifest requested).
    pub available_tools: Vec<String>,
}

// ---------------------------------------------------------------------------
// SkillRunner
// ---------------------------------------------------------------------------

/// Prepares skills for execution by validating tool requirements and
/// assembling prompts.
#[derive(Debug)]
pub struct SkillRunner;

impl SkillRunner {
    /// Create a new skill runner.
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Prepare a skill for execution.
    ///
    /// This verifies that all tools declared in the manifest's
    /// `[capabilities]` section are available in the registry, then
    /// assembles the system prompt and tool list.
    ///
    /// # Errors
    ///
    /// Returns [`SkillError::ToolNotFound`] if any tool declared in the
    /// manifest is not present in the registry.
    pub fn prepare(
        &self,
        manifest: &SkillManifest,
        registry: &dyn ToolRegistry,
    ) -> Result<PreparedSkill, SkillError> {
        let skill_name = &manifest.skill.name;
        debug!(skill = %skill_name, "preparing skill for execution");

        // Verify all required tools are available.
        let mut available_tools = Vec::new();
        for tool_name in &manifest.capabilities.tools {
            if !registry.has_tool(tool_name) {
                return Err(SkillError::ToolNotFound {
                    tool_name: tool_name.clone(),
                });
            }
            available_tools.push(tool_name.clone());
        }

        // Assemble the system prompt.
        let system_prompt = manifest.prompts.system.clone();

        info!(
            skill = %skill_name,
            tools = ?available_tools,
            prompt_len = system_prompt.len(),
            "skill prepared"
        );

        Ok(PreparedSkill {
            manifest: manifest.clone(),
            system_prompt,
            available_tools,
        })
    }
}

impl Default for SkillRunner {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
// Test fixtures use explicit panic boundaries to identify preparation contract
// failures at their source.
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "unit-test skill runner assertions intentionally panic with focused diagnostics"
)]
mod tests {
    use super::*;
    use crate::manifest::SkillManifest;

    /// A simple in-memory tool registry for testing.
    struct FakeRegistry {
        tools: HashSet<String>,
    }

    impl FakeRegistry {
        fn new(tools: &[&str]) -> Self {
            Self {
                tools: tools.iter().map(|s| (*s).to_string()).collect(),
            }
        }
    }

    impl ToolRegistry for FakeRegistry {
        fn available_tools(&self) -> HashSet<String> {
            self.tools.clone()
        }
    }

    const MANIFEST_WITH_TOOLS: &str = r#"
[skill]
name = "test-skill"
version = "1.0.0"
description = "A test skill"

[capabilities]
required_grants = ["chain.query"]
tools = ["search_memory", "chain_query"]

[prompts]
system = "You are a test assistant."
"#;

    const MANIFEST_NO_TOOLS: &str = r#"
[skill]
name = "simple-skill"
version = "0.1.0"

[prompts]
system = "Hello world."
"#;

    #[test]
    fn prepare_success_with_all_tools() {
        let manifest = SkillManifest::from_toml(MANIFEST_WITH_TOOLS).expect("should parse");
        let registry = FakeRegistry::new(&["search_memory", "chain_query", "extra_tool"]);
        let runner = SkillRunner::new();

        let prepared = runner
            .prepare(&manifest, &registry)
            .expect("should prepare");
        assert_eq!(prepared.manifest.skill.name, "test-skill");
        assert_eq!(prepared.system_prompt, "You are a test assistant.");
        assert_eq!(
            prepared.available_tools,
            vec!["search_memory", "chain_query"]
        );
    }

    #[test]
    fn prepare_fails_with_missing_tool() {
        let manifest = SkillManifest::from_toml(MANIFEST_WITH_TOOLS).expect("should parse");
        // Registry only has search_memory, not chain_query.
        let registry = FakeRegistry::new(&["search_memory"]);
        let runner = SkillRunner::new();

        let result = runner.prepare(&manifest, &registry);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("chain_query"));
    }

    #[test]
    fn prepare_no_tools_required() {
        let manifest = SkillManifest::from_toml(MANIFEST_NO_TOOLS).expect("should parse");
        let registry = FakeRegistry::new(&[]);
        let runner = SkillRunner::new();

        let prepared = runner
            .prepare(&manifest, &registry)
            .expect("should prepare");
        assert!(prepared.available_tools.is_empty());
        assert_eq!(prepared.system_prompt, "Hello world.");
    }

    #[test]
    fn prepare_with_empty_registry() {
        let manifest = SkillManifest::from_toml(MANIFEST_WITH_TOOLS).expect("should parse");
        let registry = FakeRegistry::new(&[]);
        let runner = SkillRunner::new();

        let result = runner.prepare(&manifest, &registry);
        assert!(result.is_err());
    }

    #[test]
    fn prepared_skill_fields() {
        let manifest = SkillManifest::from_toml(MANIFEST_NO_TOOLS).expect("should parse");
        let registry = FakeRegistry::new(&[]);
        let runner = SkillRunner;

        let prepared = runner
            .prepare(&manifest, &registry)
            .expect("should prepare");
        assert_eq!(prepared.manifest.skill.name, "simple-skill");
        assert_eq!(prepared.manifest.skill.version, "0.1.0");
    }

    #[test]
    fn tool_registry_has_tool() {
        let registry = FakeRegistry::new(&["foo", "bar"]);
        assert!(registry.has_tool("foo"));
        assert!(registry.has_tool("bar"));
        assert!(!registry.has_tool("baz"));
    }
}
