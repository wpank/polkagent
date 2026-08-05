//! Canonical tool name normalization with per-backend alias maps.
//!
//! Each AI coding agent backend (Claude Code, Codex, Copilot) uses its own
//! tool names. This module provides a static mapping between canonical
//! Polkagent tool names and the backend-specific aliases, enabling the kernel
//! to work with a single unified namespace.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// ToolCategory
// ---------------------------------------------------------------------------

/// Broad category for a canonical tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCategory {
    /// Tools that read files or search the filesystem.
    Read,
    /// Tools that create or modify files.
    Write,
    /// Tools that execute arbitrary commands.
    Exec,
    /// Tools that access the network.
    Network,
    /// Meta / orchestration tools (sub-agents, tasks).
    Meta,
    /// Jupyter notebook manipulation tools.
    Notebook,
}

// ---------------------------------------------------------------------------
// CanonicalTool
// ---------------------------------------------------------------------------

/// A single entry in the canonical tool table.
///
/// Each entry maps a canonical Polkagent tool name to its backend-specific
/// aliases. A `None` alias means the backend does not expose that tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CanonicalTool {
    /// The canonical (Polkagent-internal) name, e.g. `"read_file"`.
    pub name: &'static str,
    /// The broad category of the tool.
    pub category: ToolCategory,
    /// Claude Code name, if the tool exists in Claude Code.
    pub claude_name: Option<&'static str>,
    /// Codex CLI name, if the tool exists in Codex.
    pub codex_name: Option<&'static str>,
    /// Copilot Coding Agent name, if the tool exists in Copilot.
    pub copilot_name: Option<&'static str>,
}

// ---------------------------------------------------------------------------
// Static tool table (PRD S7.1)
// ---------------------------------------------------------------------------

/// The complete set of canonical tools recognized by Polkagent.
static TOOLS: &[CanonicalTool] = &[
    CanonicalTool {
        name: "read_file",
        category: ToolCategory::Read,
        claude_name: Some("Read"),
        codex_name: Some("codex_read_file"),
        copilot_name: Some("read"),
    },
    CanonicalTool {
        name: "write_file",
        category: ToolCategory::Write,
        claude_name: Some("Write"),
        codex_name: Some("codex_write_file"),
        copilot_name: Some("write"),
    },
    CanonicalTool {
        name: "edit_file",
        category: ToolCategory::Write,
        claude_name: Some("Edit"),
        codex_name: Some("codex_edit_file"),
        copilot_name: Some("edit"),
    },
    CanonicalTool {
        name: "glob",
        category: ToolCategory::Read,
        claude_name: Some("Glob"),
        codex_name: None,
        copilot_name: Some("grep"),
    },
    CanonicalTool {
        name: "grep",
        category: ToolCategory::Read,
        claude_name: Some("Grep"),
        codex_name: None,
        copilot_name: Some("grep"),
    },
    CanonicalTool {
        name: "bash",
        category: ToolCategory::Exec,
        claude_name: Some("Bash"),
        codex_name: Some("codex_bash"),
        copilot_name: Some("shell"),
    },
    CanonicalTool {
        name: "web_fetch",
        category: ToolCategory::Network,
        claude_name: Some("WebFetch"),
        codex_name: None,
        copilot_name: None,
    },
    CanonicalTool {
        name: "web_search",
        category: ToolCategory::Network,
        claude_name: Some("WebSearch"),
        codex_name: None,
        copilot_name: None,
    },
    CanonicalTool {
        name: "task",
        category: ToolCategory::Meta,
        claude_name: Some("Agent"),
        codex_name: None,
        copilot_name: None,
    },
    CanonicalTool {
        name: "notebook_edit",
        category: ToolCategory::Notebook,
        claude_name: Some("NotebookEdit"),
        codex_name: None,
        copilot_name: None,
    },
    CanonicalTool {
        name: "apply_patch",
        category: ToolCategory::Write,
        claude_name: None,
        codex_name: Some("codex_apply_patch"),
        copilot_name: None,
    },
];

// ---------------------------------------------------------------------------
// Resolution functions
// ---------------------------------------------------------------------------

/// Given a Claude Code tool name, return the canonical name.
pub fn canonical_of_claude(claude_name: &str) -> Option<&'static str> {
    TOOLS
        .iter()
        .find(|t| t.claude_name == Some(claude_name))
        .map(|t| t.name)
}

/// Given a canonical name, return the Claude Code tool name.
pub fn claude_of_canonical(canonical: &str) -> Option<&'static str> {
    TOOLS
        .iter()
        .find(|t| t.name == canonical)
        .and_then(|t| t.claude_name)
}

/// Given a Codex CLI tool name, return the canonical name.
pub fn canonical_of_codex(codex_name: &str) -> Option<&'static str> {
    TOOLS
        .iter()
        .find(|t| t.codex_name == Some(codex_name))
        .map(|t| t.name)
}

/// Given a canonical name, return the Codex CLI tool name.
pub fn codex_of_canonical(canonical: &str) -> Option<&'static str> {
    TOOLS
        .iter()
        .find(|t| t.name == canonical)
        .and_then(|t| t.codex_name)
}

/// Given a Copilot Coding Agent tool name, return the canonical name.
///
/// **Note:** Copilot maps `"grep"` to both `glob` and `grep` canonical tools.
/// This function returns the first match (`glob`). Use [`all_canonical_tools`]
/// for full resolution if disambiguation is needed.
pub fn canonical_of_copilot(copilot_name: &str) -> Option<&'static str> {
    TOOLS
        .iter()
        .find(|t| t.copilot_name == Some(copilot_name))
        .map(|t| t.name)
}

/// Given a canonical name, return the Copilot Coding Agent tool name.
pub fn copilot_of_canonical(canonical: &str) -> Option<&'static str> {
    TOOLS
        .iter()
        .find(|t| t.name == canonical)
        .and_then(|t| t.copilot_name)
}

/// Return the [`ToolCategory`] for a canonical tool name, or `None` if
/// the name is not recognized.
pub fn category_of(canonical: &str) -> Option<ToolCategory> {
    TOOLS
        .iter()
        .find(|t| t.name == canonical)
        .map(|t| t.category)
}

/// Return the full static slice of all canonical tools.
pub fn all_canonical_tools() -> &'static [CanonicalTool] {
    TOOLS
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
// Serialization fixtures use expect so a failed tool-schema round trip points
// directly at the broken boundary.
#[allow(
    clippy::expect_used,
    reason = "unit-test serialization assertions intentionally panic with focused diagnostics"
)]
mod tests {
    use super::*;

    // -- Claude -> canonical ------------------------------------------------

    #[test]
    fn claude_read_maps_to_read_file() {
        assert_eq!(canonical_of_claude("Read"), Some("read_file"));
    }

    #[test]
    fn claude_write_maps_to_write_file() {
        assert_eq!(canonical_of_claude("Write"), Some("write_file"));
    }

    #[test]
    fn claude_edit_maps_to_edit_file() {
        assert_eq!(canonical_of_claude("Edit"), Some("edit_file"));
    }

    #[test]
    fn claude_glob_maps_to_glob() {
        assert_eq!(canonical_of_claude("Glob"), Some("glob"));
    }

    #[test]
    fn claude_grep_maps_to_grep() {
        assert_eq!(canonical_of_claude("Grep"), Some("grep"));
    }

    #[test]
    fn claude_bash_maps_to_bash() {
        assert_eq!(canonical_of_claude("Bash"), Some("bash"));
    }

    #[test]
    fn claude_web_fetch_maps_to_web_fetch() {
        assert_eq!(canonical_of_claude("WebFetch"), Some("web_fetch"));
    }

    #[test]
    fn claude_web_search_maps_to_web_search() {
        assert_eq!(canonical_of_claude("WebSearch"), Some("web_search"));
    }

    #[test]
    fn claude_agent_maps_to_task() {
        assert_eq!(canonical_of_claude("Agent"), Some("task"));
    }

    #[test]
    fn claude_notebook_edit_maps_to_notebook_edit() {
        assert_eq!(canonical_of_claude("NotebookEdit"), Some("notebook_edit"));
    }

    // -- Codex -> canonical -------------------------------------------------

    #[test]
    fn codex_read_file_maps_to_read_file() {
        assert_eq!(canonical_of_codex("codex_read_file"), Some("read_file"));
    }

    #[test]
    fn codex_write_file_maps_to_write_file() {
        assert_eq!(canonical_of_codex("codex_write_file"), Some("write_file"));
    }

    #[test]
    fn codex_edit_file_maps_to_edit_file() {
        assert_eq!(canonical_of_codex("codex_edit_file"), Some("edit_file"));
    }

    #[test]
    fn codex_bash_maps_to_bash() {
        assert_eq!(canonical_of_codex("codex_bash"), Some("bash"));
    }

    #[test]
    fn codex_apply_patch_maps_to_apply_patch() {
        assert_eq!(canonical_of_codex("codex_apply_patch"), Some("apply_patch"));
    }

    // -- Copilot -> canonical -----------------------------------------------

    #[test]
    fn copilot_read_maps_to_read_file() {
        assert_eq!(canonical_of_copilot("read"), Some("read_file"));
    }

    #[test]
    fn copilot_write_maps_to_write_file() {
        assert_eq!(canonical_of_copilot("write"), Some("write_file"));
    }

    #[test]
    fn copilot_edit_maps_to_edit_file() {
        assert_eq!(canonical_of_copilot("edit"), Some("edit_file"));
    }

    #[test]
    fn copilot_grep_maps_to_glob_first_match() {
        // Copilot "grep" maps to both glob and grep; first match wins.
        assert_eq!(canonical_of_copilot("grep"), Some("glob"));
    }

    #[test]
    fn copilot_shell_maps_to_bash() {
        assert_eq!(canonical_of_copilot("shell"), Some("bash"));
    }

    // -- Reverse: canonical -> backend --------------------------------------

    #[test]
    fn canonical_read_file_to_claude() {
        assert_eq!(claude_of_canonical("read_file"), Some("Read"));
    }

    #[test]
    fn canonical_write_file_to_codex() {
        assert_eq!(codex_of_canonical("write_file"), Some("codex_write_file"));
    }

    #[test]
    fn canonical_bash_to_copilot() {
        assert_eq!(copilot_of_canonical("bash"), Some("shell"));
    }

    #[test]
    fn canonical_apply_patch_has_no_claude_name() {
        assert_eq!(claude_of_canonical("apply_patch"), None);
    }

    #[test]
    fn canonical_web_fetch_has_no_codex_name() {
        assert_eq!(codex_of_canonical("web_fetch"), None);
    }

    #[test]
    fn canonical_task_has_no_copilot_name() {
        assert_eq!(copilot_of_canonical("task"), None);
    }

    // -- Unknown names return None ------------------------------------------

    #[test]
    fn unknown_claude_name_returns_none() {
        assert_eq!(canonical_of_claude("DoesNotExist"), None);
    }

    #[test]
    fn unknown_codex_name_returns_none() {
        assert_eq!(canonical_of_codex("codex_unknown"), None);
    }

    #[test]
    fn unknown_copilot_name_returns_none() {
        assert_eq!(canonical_of_copilot("unknown_tool"), None);
    }

    #[test]
    fn unknown_canonical_returns_none_for_all_backends() {
        assert_eq!(claude_of_canonical("nope"), None);
        assert_eq!(codex_of_canonical("nope"), None);
        assert_eq!(copilot_of_canonical("nope"), None);
    }

    // -- category_of --------------------------------------------------------

    #[test]
    fn category_of_read_file_is_read() {
        assert_eq!(category_of("read_file"), Some(ToolCategory::Read));
    }

    #[test]
    fn category_of_write_file_is_write() {
        assert_eq!(category_of("write_file"), Some(ToolCategory::Write));
    }

    #[test]
    fn category_of_edit_file_is_write() {
        assert_eq!(category_of("edit_file"), Some(ToolCategory::Write));
    }

    #[test]
    fn category_of_glob_is_read() {
        assert_eq!(category_of("glob"), Some(ToolCategory::Read));
    }

    #[test]
    fn category_of_grep_is_read() {
        assert_eq!(category_of("grep"), Some(ToolCategory::Read));
    }

    #[test]
    fn category_of_bash_is_exec() {
        assert_eq!(category_of("bash"), Some(ToolCategory::Exec));
    }

    #[test]
    fn category_of_web_fetch_is_network() {
        assert_eq!(category_of("web_fetch"), Some(ToolCategory::Network));
    }

    #[test]
    fn category_of_web_search_is_network() {
        assert_eq!(category_of("web_search"), Some(ToolCategory::Network));
    }

    #[test]
    fn category_of_task_is_meta() {
        assert_eq!(category_of("task"), Some(ToolCategory::Meta));
    }

    #[test]
    fn category_of_notebook_edit_is_notebook() {
        assert_eq!(category_of("notebook_edit"), Some(ToolCategory::Notebook));
    }

    #[test]
    fn category_of_apply_patch_is_write() {
        assert_eq!(category_of("apply_patch"), Some(ToolCategory::Write));
    }

    #[test]
    fn category_of_unknown_returns_none() {
        assert_eq!(category_of("unknown"), None);
    }

    // -- all_canonical_tools ------------------------------------------------

    #[test]
    fn all_canonical_tools_returns_11_entries() {
        assert_eq!(all_canonical_tools().len(), 11);
    }

    #[test]
    fn all_canonical_tools_names_are_unique() {
        let names: Vec<&str> = all_canonical_tools().iter().map(|t| t.name).collect();
        let mut deduped = names.clone();
        deduped.sort_unstable();
        deduped.dedup();
        assert_eq!(names.len(), deduped.len());
    }

    // -- Serde round-trip for ToolCategory ----------------------------------

    #[test]
    fn tool_category_serde_round_trip() {
        let categories = [
            ToolCategory::Read,
            ToolCategory::Write,
            ToolCategory::Exec,
            ToolCategory::Network,
            ToolCategory::Meta,
            ToolCategory::Notebook,
        ];
        for cat in &categories {
            let json = serde_json::to_string(cat).expect("serialize");
            let back: ToolCategory = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(*cat, back);
        }
    }

    #[test]
    fn tool_category_serializes_as_snake_case() {
        assert_eq!(
            serde_json::to_string(&ToolCategory::Read).expect("serialize"),
            r#""read""#
        );
        assert_eq!(
            serde_json::to_string(&ToolCategory::Network).expect("serialize"),
            r#""network""#
        );
    }
}
