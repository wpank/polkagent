//! Template engine for system prompt variable resolution.
//!
//! [`TemplateEngine`] resolves `{{variable}}` placeholders in system prompt
//! text, replacing them with runtime values such as the agent's name, current
//! time, chain name, and capabilities list.

use std::collections::HashMap;

use chrono::Utc;
use tracing::debug;

use crate::error::{ContextError, ContextResult};

// ---------------------------------------------------------------------------
// TemplateEngine
// ---------------------------------------------------------------------------

/// Resolves `{{variable}}` placeholders in template strings.
///
/// Variables are registered with [`TemplateEngine::set`] and resolved by
/// [`TemplateEngine::render`]. Unknown variables cause an error unless
/// the `allow_unknown` setting is enabled, in which case they are
/// left as-is.
#[derive(Debug, Clone)]
pub struct TemplateEngine {
    /// Registered variables and their values.
    variables: HashMap<String, String>,
    /// Whether to silently skip unknown variables instead of returning an
    /// error.
    allow_unknown: bool,
}

impl TemplateEngine {
    /// Create a new empty template engine.
    #[must_use]
    pub fn new() -> Self {
        Self {
            variables: HashMap::new(),
            allow_unknown: false,
        }
    }

    /// Set a template variable.
    pub fn set(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.variables.insert(name.into(), value.into());
    }

    /// Configure whether unknown variables are silently left intact.
    pub fn set_allow_unknown(&mut self, allow: bool) {
        self.allow_unknown = allow;
    }

    /// Pre-populate the standard built-in variables.
    ///
    /// Sets: `current_time`, `current_date`.
    pub fn set_builtins(&mut self) {
        let now = Utc::now();
        self.set("current_time", now.to_rfc3339());
        self.set("current_date", now.format("%Y-%m-%d").to_string());
    }

    /// Render a template string, replacing all `{{variable}}` placeholders.
    ///
    /// # Errors
    ///
    /// Returns [`ContextError::UnknownVariable`] if a placeholder references a
    /// variable that has not been set (and `allow_unknown` is `false`).
    pub fn render(&self, template: &str) -> ContextResult<String> {
        let mut result = String::with_capacity(template.len());
        let mut chars = template.chars().peekable();

        while let Some(ch) = chars.next() {
            if ch == '{' && chars.peek() == Some(&'{') {
                // Consume the second '{'
                chars.next();
                // Read until '}}'
                let mut var_name = String::new();
                let mut found_close = false;
                while let Some(inner) = chars.next() {
                    if inner == '}' && chars.peek() == Some(&'}') {
                        chars.next(); // consume second '}'
                        found_close = true;
                        break;
                    }
                    var_name.push(inner);
                }

                if !found_close {
                    // Malformed — no closing braces; emit literally.
                    result.push_str("{{");
                    result.push_str(&var_name);
                    continue;
                }

                let var_name = var_name.trim();
                if let Some(value) = self.variables.get(var_name) {
                    debug!(variable = var_name, "resolved template variable");
                    result.push_str(value);
                } else if self.allow_unknown {
                    // Leave placeholder intact.
                    result.push_str("{{");
                    result.push_str(var_name);
                    result.push_str("}}");
                } else {
                    return Err(ContextError::UnknownVariable {
                        name: var_name.to_string(),
                    });
                }
            } else {
                result.push(ch);
            }
        }

        Ok(result)
    }

    /// Return all registered variable names.
    #[must_use]
    pub fn variable_names(&self) -> Vec<&str> {
        self.variables.keys().map(String::as_str).collect()
    }
}

impl Default for TemplateEngine {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_placeholders_passthrough() {
        let engine = TemplateEngine::new();
        let result = engine.render("Hello, world!").expect("render");
        assert_eq!(result, "Hello, world!");
    }

    #[test]
    fn single_variable_replacement() {
        let mut engine = TemplateEngine::new();
        engine.set("agent_name", "PolkaBot");
        let result = engine.render("You are {{agent_name}}.").expect("render");
        assert_eq!(result, "You are PolkaBot.");
    }

    #[test]
    fn multiple_variables() {
        let mut engine = TemplateEngine::new();
        engine.set("agent_name", "PolkaBot");
        engine.set("chain_name", "Polkadot");
        let result = engine
            .render("{{agent_name}} operates on {{chain_name}}.")
            .expect("render");
        assert_eq!(result, "PolkaBot operates on Polkadot.");
    }

    #[test]
    fn repeated_variable() {
        let mut engine = TemplateEngine::new();
        engine.set("name", "Alpha");
        let result = engine
            .render("{{name}} and {{name}} again")
            .expect("render");
        assert_eq!(result, "Alpha and Alpha again");
    }

    #[test]
    fn whitespace_in_variable_name_trimmed() {
        let mut engine = TemplateEngine::new();
        engine.set("agent_name", "Bot");
        let result = engine.render("{{ agent_name }}").expect("render");
        assert_eq!(result, "Bot");
    }

    #[test]
    fn unknown_variable_errors_by_default() {
        let engine = TemplateEngine::new();
        let err = engine.render("Hello {{missing}}").unwrap_err();
        assert!(err.to_string().contains("unknown template variable"));
        assert!(err.to_string().contains("missing"));
    }

    #[test]
    fn unknown_variable_allowed_when_configured() {
        let mut engine = TemplateEngine::new();
        engine.set_allow_unknown(true);
        let result = engine.render("Hello {{missing}}").expect("render");
        assert_eq!(result, "Hello {{missing}}");
    }

    #[test]
    fn set_builtins_populates_time_variables() {
        let mut engine = TemplateEngine::new();
        engine.set_builtins();
        assert!(engine.variable_names().contains(&"current_time"));
        assert!(engine.variable_names().contains(&"current_date"));
        let result = engine.render("Today is {{current_date}}.").expect("render");
        assert!(result.starts_with("Today is 20")); // starts with year
    }

    #[test]
    fn single_braces_not_treated_as_variables() {
        let engine = TemplateEngine::new();
        let result = engine.render("JSON: {\"key\": \"value\"}").expect("render");
        assert_eq!(result, "JSON: {\"key\": \"value\"}");
    }

    #[test]
    fn empty_template() {
        let engine = TemplateEngine::new();
        let result = engine.render("").expect("render");
        assert_eq!(result, "");
    }

    #[test]
    fn capabilities_list_variable() {
        let mut engine = TemplateEngine::new();
        engine.set(
            "capabilities",
            "- Transfer tokens\n- Query balances\n- Sign transactions",
        );
        let template = "You can:\n{{capabilities}}";
        let result = engine.render(template).expect("render");
        assert!(result.contains("Transfer tokens"));
        assert!(result.contains("Sign transactions"));
    }
}
