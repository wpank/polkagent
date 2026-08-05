//! Output formatters for audit entries.
//!
//! Supports three output modes for compliance review and export:
//! - **JSON Lines** — one JSON object per line, machine-parseable.
//! - **Human-readable text** — tabular, aligned for terminal display.
//! - **CSV** — comma-separated values for spreadsheet import.

use crate::entry::AuditEntry;
use crate::error::AuditResult;

// ---------------------------------------------------------------------------
// OutputFormat
// ---------------------------------------------------------------------------

/// The output format for rendering audit entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    /// One JSON object per line (JSON Lines / NDJSON).
    JsonLines,
    /// Human-readable text suitable for terminal display.
    Text,
    /// Comma-separated values.
    Csv,
}

// ---------------------------------------------------------------------------
// Formatting functions
// ---------------------------------------------------------------------------

/// Format a single audit entry as a JSON line (no trailing newline).
pub fn format_json_line(entry: &AuditEntry) -> AuditResult<String> {
    Ok(serde_json::to_string(entry)?)
}

/// Format a single audit entry as human-readable text (no trailing newline).
#[must_use]
pub fn format_text(entry: &AuditEntry) -> String {
    format!(
        "{ts}  {actor:<30}  {action:<20}  {resource:<30}  {outcome:<10}  {hash}",
        ts = entry.timestamp.format("%Y-%m-%dT%H:%M:%S%.3fZ"),
        actor = entry.actor.to_string(),
        action = entry.action.to_string(),
        resource = entry.resource.to_string(),
        outcome = entry.outcome.to_string(),
        hash = &entry.integrity_hash[..8.min(entry.integrity_hash.len())],
    )
}

/// Format a single audit entry as a CSV row (no trailing newline).
///
/// Fields: `timestamp`, `actor_type`, `actor_id`, `actor_name`, `action`,
/// `resource_type`, `resource_id`, `outcome`, `integrity_hash`
#[must_use]
pub fn format_csv_row(entry: &AuditEntry) -> String {
    fn escape_csv(s: &str) -> String {
        if s.contains(',') || s.contains('"') || s.contains('\n') {
            format!("\"{}\"", s.replace('"', "\"\""))
        } else {
            s.to_string()
        }
    }

    format!(
        "{ts},{actor_type},{actor_id},{actor_name},{action},{resource_type},{resource_id},{outcome},{hash}",
        ts = entry.timestamp.format("%Y-%m-%dT%H:%M:%S%.3fZ"),
        actor_type = entry.actor.actor_type,
        actor_id = escape_csv(&entry.actor.id),
        actor_name = escape_csv(entry.actor.name.as_deref().unwrap_or("")),
        action = entry.action,
        resource_type = escape_csv(&entry.resource.resource_type),
        resource_id = escape_csv(&entry.resource.resource_id),
        outcome = entry.outcome,
        hash = entry.integrity_hash,
    )
}

/// Return the CSV header row.
#[must_use]
pub fn csv_header() -> &'static str {
    "timestamp,actor_type,actor_id,actor_name,action,resource_type,resource_id,outcome,integrity_hash"
}

/// Format multiple entries in the specified format, joining with newlines.
pub fn format_entries(entries: &[AuditEntry], format: OutputFormat) -> AuditResult<String> {
    let mut lines = Vec::with_capacity(entries.len() + 1);

    if format == OutputFormat::Csv {
        lines.push(csv_header().to_string());
    }

    for entry in entries {
        let line = match format {
            OutputFormat::JsonLines => format_json_line(entry)?,
            OutputFormat::Text => format_text(entry),
            OutputFormat::Csv => format_csv_row(entry),
        };
        lines.push(line);
    }

    Ok(lines.join("\n"))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
// Test fixtures use explicit panic boundaries to identify broken invariants.
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "unit-test audit assertions intentionally panic with focused diagnostics"
)]
mod tests {
    use super::*;
    use crate::action::AuditAction;
    use crate::actor::ActorInfo;
    use crate::entry::{ActionOutcome, AuditEntry, ResourceInfo};

    fn make_entry() -> AuditEntry {
        let mut entry = AuditEntry::new(
            ActorInfo::agent("agent-1").with_name("Governance Bot"),
            AuditAction::ToolInvoked,
            ResourceInfo::new("tool", "balance-check"),
            ActionOutcome::Success,
            serde_json::json!({"param": "value"}),
        );
        entry.integrity_hash =
            "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789".to_string();
        entry
    }

    #[test]
    fn format_json_line_produces_valid_json() {
        let entry = make_entry();
        let line = format_json_line(&entry).expect("format");
        // Should be valid JSON.
        let parsed: serde_json::Value = serde_json::from_str(&line).expect("parse");
        assert!(parsed.is_object());
        assert_eq!(parsed["action"], "tool_invoked");
    }

    #[test]
    fn format_text_contains_fields() {
        let entry = make_entry();
        let text = format_text(&entry);
        assert!(text.contains("agent:agent-1 (Governance Bot)"));
        assert!(text.contains("tool_invoked"));
        assert!(text.contains("tool:balance-check"));
        assert!(text.contains("success"));
        assert!(text.contains("abcdef01"));
    }

    #[test]
    fn format_csv_row_correct_field_count() {
        let entry = make_entry();
        let row = format_csv_row(&entry);
        let fields: Vec<&str> = row.split(',').collect();
        // 9 fields: timestamp, actor_type, actor_id, actor_name, action,
        // resource_type, resource_id, outcome, integrity_hash
        assert_eq!(fields.len(), 9);
    }

    #[test]
    fn csv_header_matches_row_count() {
        let header_fields: Vec<&str> = csv_header().split(',').collect();
        let entry = make_entry();
        let row_str = format_csv_row(&entry);
        let row_fields: Vec<&str> = row_str.split(',').collect();
        assert_eq!(header_fields.len(), row_fields.len());
    }

    #[test]
    fn format_entries_json_lines() {
        let entries = vec![make_entry(), make_entry()];
        let output = format_entries(&entries, OutputFormat::JsonLines).expect("format");
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines.len(), 2);
        // Each line should be valid JSON.
        for line in &lines {
            serde_json::from_str::<serde_json::Value>(line).expect("valid JSON");
        }
    }

    #[test]
    fn format_entries_csv_includes_header() {
        let entries = vec![make_entry()];
        let output = format_entries(&entries, OutputFormat::Csv).expect("format");
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines.len(), 2); // header + 1 row
        assert_eq!(lines[0], csv_header());
    }

    #[test]
    fn format_entries_text() {
        let entries = vec![make_entry(), make_entry()];
        let output = format_entries(&entries, OutputFormat::Text).expect("format");
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn format_entries_empty_list() {
        let output = format_entries(&[], OutputFormat::JsonLines).expect("format");
        assert!(output.is_empty());

        // CSV with empty entries should still produce just the header.
        let csv_output = format_entries(&[], OutputFormat::Csv).expect("format");
        assert_eq!(csv_output, csv_header());
    }

    #[test]
    fn csv_escapes_commas_in_fields() {
        let mut entry = make_entry();
        entry.actor = ActorInfo::agent("agent,with,commas");
        let row = format_csv_row(&entry);
        assert!(row.contains("\"agent,with,commas\""));
    }
}
