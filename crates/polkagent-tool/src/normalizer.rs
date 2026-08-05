//! Tool name normalization and alias resolution.
//!
//! Different LLM backends use different naming conventions for tools:
//!
//! - **Anthropic**: `PascalCase` or `snake_case` (e.g. `Read`, `file_read`)
//! - **`OpenAI`**: `snake_case` (e.g. `read_file`, `search_code`)
//! - **MCP**: dotted namespaces (e.g. `mcp__server__tool`)
//!
//! [`ToolNameNormalizer`] maps these backend-specific names to canonical
//! `snake_case.dotted` names (e.g. `polkagent.file.read`) so the kernel
//! can dispatch uniformly regardless of which executor produced the tool
//! call.

use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Backend
// ---------------------------------------------------------------------------

/// Identifies an LLM backend whose naming conventions differ from the
/// canonical form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Backend {
    /// Anthropic Claude models.
    Anthropic,
    /// `OpenAI` GPT models.
    OpenAI,
    /// Model Context Protocol servers.
    Mcp,
    /// Gemini models.
    Gemini,
}

// ---------------------------------------------------------------------------
// ToolNameNormalizer
// ---------------------------------------------------------------------------

/// Resolves backend-specific tool names to canonical names.
///
/// Canonical names follow the pattern `namespace.category.action` using
/// `snake_case` segments separated by dots (e.g. `polkagent.file.read`).
///
/// # Examples
///
/// ```
/// use polkagent_tool::normalizer::{Backend, ToolNameNormalizer};
///
/// let mut n = ToolNameNormalizer::new();
/// n.register_alias("Read", "polkagent.file.read");
/// n.register_backend_alias(Backend::OpenAI, "read_file", "polkagent.file.read");
///
/// assert_eq!(n.normalize("Read"), "polkagent.file.read");
/// assert_eq!(
///     n.normalize_for_backend("read_file", Backend::OpenAI),
///     "polkagent.file.read",
/// );
/// // Unknown names pass through unchanged.
/// assert_eq!(n.normalize("unknown_tool"), "unknown_tool");
/// ```
pub struct ToolNameNormalizer {
    /// Global alias map (alias -> canonical).
    global_aliases: HashMap<String, String>,

    /// Per-backend alias maps (backend -> (alias -> canonical)).
    backend_aliases: HashMap<Backend, HashMap<String, String>>,

    /// Whether to perform case-insensitive matching.
    case_insensitive: bool,
}

impl ToolNameNormalizer {
    /// Create a new normalizer with case-sensitive matching.
    #[must_use]
    pub fn new() -> Self {
        Self {
            global_aliases: HashMap::new(),
            backend_aliases: HashMap::new(),
            case_insensitive: false,
        }
    }

    /// Create a new normalizer with case-insensitive matching enabled.
    #[must_use]
    pub fn case_insensitive() -> Self {
        Self {
            global_aliases: HashMap::new(),
            backend_aliases: HashMap::new(),
            case_insensitive: true,
        }
    }

    /// Set whether alias matching is case-insensitive.
    ///
    /// When enabled, alias lookups fold both the registered alias and the
    /// query to lowercase before comparing.
    pub fn set_case_insensitive(&mut self, enabled: bool) {
        self.case_insensitive = enabled;
    }

    // -- Registration -------------------------------------------------------

    /// Register a global alias that maps to a canonical name.
    ///
    /// Global aliases are checked before any backend-specific map.
    pub fn register_alias(&mut self, alias: &str, canonical: &str) {
        let key = self.normalize_key(alias);
        self.global_aliases.insert(key, canonical.to_string());
    }

    /// Register an alias scoped to a specific backend.
    pub fn register_backend_alias(&mut self, backend: Backend, alias: &str, canonical: &str) {
        let key = self.normalize_key(alias);
        self.backend_aliases
            .entry(backend)
            .or_default()
            .insert(key, canonical.to_string());
    }

    /// Register multiple global aliases at once from an iterator of
    /// `(alias, canonical)` pairs.
    pub fn register_aliases<I, A, C>(&mut self, aliases: I)
    where
        I: IntoIterator<Item = (A, C)>,
        A: AsRef<str>,
        C: AsRef<str>,
    {
        for (alias, canonical) in aliases {
            self.register_alias(alias.as_ref(), canonical.as_ref());
        }
    }

    /// Register multiple backend-scoped aliases at once.
    pub fn register_backend_aliases<I, A, C>(&mut self, backend: Backend, aliases: I)
    where
        I: IntoIterator<Item = (A, C)>,
        A: AsRef<str>,
        C: AsRef<str>,
    {
        for (alias, canonical) in aliases {
            self.register_backend_alias(backend, alias.as_ref(), canonical.as_ref());
        }
    }

    // -- Lookup -------------------------------------------------------------

    /// Normalize a tool name using the global alias map.
    ///
    /// Returns the canonical name if an alias matches, or the original name
    /// if no alias is found.
    pub fn normalize<'a>(&'a self, name: &'a str) -> &'a str {
        let key = self.normalize_key(name);
        match self.global_aliases.get(&key) {
            Some(canonical) => canonical.as_str(),
            None => name,
        }
    }

    /// Normalize a tool name using the backend-specific alias map first,
    /// falling back to the global map, and finally returning the original
    /// name if no match is found.
    pub fn normalize_for_backend<'a>(&'a self, name: &'a str, backend: Backend) -> &'a str {
        let key = self.normalize_key(name);

        // 1. Backend-specific map.
        if let Some(backend_map) = self.backend_aliases.get(&backend) {
            if let Some(canonical) = backend_map.get(&key) {
                return canonical.as_str();
            }
        }

        // 2. Global map.
        if let Some(canonical) = self.global_aliases.get(&key) {
            return canonical.as_str();
        }

        // 3. Pass-through.
        name
    }

    /// Look up the canonical name for an alias, returning `None` if the
    /// name is not a known alias.
    pub fn canonical_name(&self, name: &str) -> Option<&str> {
        let key = self.normalize_key(name);
        self.global_aliases.get(&key).map(String::as_str)
    }

    /// Look up the canonical name for a backend-specific alias, falling
    /// back to the global map. Returns `None` if no alias matches.
    pub fn canonical_name_for_backend(&self, name: &str, backend: Backend) -> Option<&str> {
        let key = self.normalize_key(name);

        if let Some(backend_map) = self.backend_aliases.get(&backend) {
            if let Some(canonical) = backend_map.get(&key) {
                return Some(canonical.as_str());
            }
        }

        self.global_aliases.get(&key).map(String::as_str)
    }

    /// Return all global aliases as `(alias, canonical)` pairs.
    pub fn global_aliases(&self) -> impl Iterator<Item = (&str, &str)> {
        self.global_aliases
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
    }

    /// Return aliases for a specific backend as `(alias, canonical)` pairs.
    pub fn backend_alias_entries(&self, backend: Backend) -> impl Iterator<Item = (&str, &str)> {
        self.backend_aliases
            .get(&backend)
            .into_iter()
            .flat_map(|m| m.iter().map(|(k, v)| (k.as_str(), v.as_str())))
    }

    /// Return the total number of registered aliases (global + all backends).
    #[must_use]
    pub fn alias_count(&self) -> usize {
        let backend: usize = self.backend_aliases.values().map(HashMap::len).sum();
        self.global_aliases.len() + backend
    }

    // -- Internal -----------------------------------------------------------

    /// Normalize a key for map lookup according to case-sensitivity setting.
    fn normalize_key(&self, name: &str) -> String {
        if self.case_insensitive {
            name.to_lowercase()
        } else {
            name.to_string()
        }
    }
}

impl Default for ToolNameNormalizer {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for ToolNameNormalizer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolNameNormalizer")
            .field("global_aliases", &self.global_aliases.len())
            .field("backend_count", &self.backend_aliases.len())
            .field("case_insensitive", &self.case_insensitive)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- Basic registration and lookup --

    #[test]
    fn normalize_returns_canonical_for_known_alias() {
        let mut n = ToolNameNormalizer::new();
        n.register_alias("Read", "polkagent.file.read");

        assert_eq!(n.normalize("Read"), "polkagent.file.read");
    }

    #[test]
    fn normalize_returns_original_for_unknown_name() {
        let n = ToolNameNormalizer::new();
        assert_eq!(n.normalize("unknown_tool"), "unknown_tool");
    }

    #[test]
    fn canonical_name_returns_some_for_alias() {
        let mut n = ToolNameNormalizer::new();
        n.register_alias("Read", "polkagent.file.read");

        assert_eq!(n.canonical_name("Read"), Some("polkagent.file.read"));
    }

    #[test]
    fn canonical_name_returns_none_for_unknown() {
        let n = ToolNameNormalizer::new();
        assert_eq!(n.canonical_name("nope"), None);
    }

    // -- Multiple aliases to same canonical --

    #[test]
    fn multiple_aliases_resolve_to_same_canonical() {
        let mut n = ToolNameNormalizer::new();
        n.register_alias("Read", "polkagent.file.read");
        n.register_alias("read_file", "polkagent.file.read");
        n.register_alias("file_read", "polkagent.file.read");

        assert_eq!(n.normalize("Read"), "polkagent.file.read");
        assert_eq!(n.normalize("read_file"), "polkagent.file.read");
        assert_eq!(n.normalize("file_read"), "polkagent.file.read");
    }

    // -- Case-insensitive matching --

    #[test]
    fn case_insensitive_matching() {
        let mut n = ToolNameNormalizer::case_insensitive();
        n.register_alias("Read", "polkagent.file.read");

        assert_eq!(n.normalize("read"), "polkagent.file.read");
        assert_eq!(n.normalize("READ"), "polkagent.file.read");
        assert_eq!(n.normalize("Read"), "polkagent.file.read");
    }

    #[test]
    fn case_sensitive_does_not_match_different_case() {
        let mut n = ToolNameNormalizer::new();
        n.register_alias("Read", "polkagent.file.read");

        // Exact match works.
        assert_eq!(n.normalize("Read"), "polkagent.file.read");
        // Different case does not match - returns original.
        assert_eq!(n.normalize("read"), "read");
    }

    #[test]
    fn toggle_case_sensitivity() {
        let mut n = ToolNameNormalizer::new();
        n.register_alias("Read", "polkagent.file.read");

        assert_eq!(n.normalize("read"), "read"); // no match

        n.set_case_insensitive(true);
        // After re-registering with new setting active:
        n.register_alias("Read", "polkagent.file.read");
        assert_eq!(n.normalize("read"), "polkagent.file.read");
    }

    // -- Backend-specific aliases --

    #[test]
    fn backend_alias_takes_precedence() {
        let mut n = ToolNameNormalizer::new();
        n.register_alias("read", "polkagent.file.read");
        n.register_backend_alias(
            Backend::OpenAI,
            "read",
            "polkagent.file.read_openai_variant",
        );

        // Global lookup.
        assert_eq!(n.normalize("read"), "polkagent.file.read");

        // Backend-specific lookup takes precedence.
        assert_eq!(
            n.normalize_for_backend("read", Backend::OpenAI),
            "polkagent.file.read_openai_variant"
        );
    }

    #[test]
    fn backend_alias_falls_back_to_global() {
        let mut n = ToolNameNormalizer::new();
        n.register_alias("Read", "polkagent.file.read");
        n.register_backend_alias(Backend::OpenAI, "search_code", "polkagent.search.code");

        // "Read" is not in OpenAI backend map, falls back to global.
        assert_eq!(
            n.normalize_for_backend("Read", Backend::OpenAI),
            "polkagent.file.read"
        );
    }

    #[test]
    fn backend_alias_returns_original_when_no_match() {
        let n = ToolNameNormalizer::new();
        assert_eq!(
            n.normalize_for_backend("unknown", Backend::Anthropic),
            "unknown"
        );
    }

    #[test]
    fn canonical_name_for_backend_checks_backend_first() {
        let mut n = ToolNameNormalizer::new();
        n.register_alias("read", "global.read");
        n.register_backend_alias(Backend::Mcp, "read", "mcp.read");

        assert_eq!(
            n.canonical_name_for_backend("read", Backend::Mcp),
            Some("mcp.read"),
        );
        // Different backend falls back to global.
        assert_eq!(
            n.canonical_name_for_backend("read", Backend::Anthropic),
            Some("global.read"),
        );
    }

    #[test]
    fn canonical_name_for_backend_returns_none_when_no_match() {
        let n = ToolNameNormalizer::new();
        assert_eq!(n.canonical_name_for_backend("nope", Backend::Gemini), None,);
    }

    // -- Batch registration --

    #[test]
    fn register_aliases_batch() {
        let mut n = ToolNameNormalizer::new();
        n.register_aliases([
            ("Read", "polkagent.file.read"),
            ("Write", "polkagent.file.write"),
            ("Search", "polkagent.search.code"),
        ]);

        assert_eq!(n.normalize("Read"), "polkagent.file.read");
        assert_eq!(n.normalize("Write"), "polkagent.file.write");
        assert_eq!(n.normalize("Search"), "polkagent.search.code");
    }

    #[test]
    fn register_backend_aliases_batch() {
        let mut n = ToolNameNormalizer::new();
        n.register_backend_aliases(
            Backend::Anthropic,
            [
                ("Read", "polkagent.file.read"),
                ("Bash", "polkagent.shell.exec"),
            ],
        );

        assert_eq!(
            n.normalize_for_backend("Read", Backend::Anthropic),
            "polkagent.file.read"
        );
        assert_eq!(
            n.normalize_for_backend("Bash", Backend::Anthropic),
            "polkagent.shell.exec"
        );
    }

    // -- Counting and iteration --

    #[test]
    fn alias_count_sums_global_and_backend() {
        let mut n = ToolNameNormalizer::new();
        n.register_alias("a", "canonical.a");
        n.register_alias("b", "canonical.b");
        n.register_backend_alias(Backend::OpenAI, "c", "canonical.c");
        n.register_backend_alias(Backend::Mcp, "d", "canonical.d");

        assert_eq!(n.alias_count(), 4);
    }

    #[test]
    fn global_aliases_iterator() {
        let mut n = ToolNameNormalizer::new();
        n.register_alias("Read", "polkagent.file.read");
        n.register_alias("Write", "polkagent.file.write");

        let aliases: HashMap<&str, &str> = n.global_aliases().collect();
        assert_eq!(aliases.len(), 2);
        assert_eq!(aliases["Read"], "polkagent.file.read");
        assert_eq!(aliases["Write"], "polkagent.file.write");
    }

    #[test]
    fn backend_alias_entries_iterator() {
        let mut n = ToolNameNormalizer::new();
        n.register_backend_alias(Backend::Anthropic, "Read", "polkagent.file.read");

        let entries: Vec<_> = n.backend_alias_entries(Backend::Anthropic).collect();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0], ("Read", "polkagent.file.read"));

        // Empty for another backend.
        let entries: Vec<_> = n.backend_alias_entries(Backend::OpenAI).collect();
        assert!(entries.is_empty());
    }

    // -- Default and Debug --

    #[test]
    fn default_creates_empty_normalizer() {
        let n = ToolNameNormalizer::default();
        assert_eq!(n.alias_count(), 0);
    }

    #[test]
    fn debug_format() {
        let mut n = ToolNameNormalizer::new();
        n.register_alias("a", "b");
        let debug = format!("{n:?}");
        assert!(debug.contains("ToolNameNormalizer"));
        assert!(debug.contains("global_aliases: 1"));
    }

    // -- Overwrite alias --

    #[test]
    fn overwrite_alias_replaces_canonical() {
        let mut n = ToolNameNormalizer::new();
        n.register_alias("Read", "old.canonical");
        assert_eq!(n.normalize("Read"), "old.canonical");

        n.register_alias("Read", "new.canonical");
        assert_eq!(n.normalize("Read"), "new.canonical");
        assert_eq!(n.alias_count(), 1); // not duplicated
    }

    // -- Canonical name is also passthrough --

    #[test]
    fn canonical_name_as_input_passes_through() {
        let mut n = ToolNameNormalizer::new();
        n.register_alias("Read", "polkagent.file.read");

        // The canonical name itself is not registered as an alias.
        assert_eq!(n.normalize("polkagent.file.read"), "polkagent.file.read");
    }
}
