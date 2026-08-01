//! Output formatting helpers for the `polkagent` CLI.
//!
//! The [`OutputFormat`] enum is parsed from the global `--format` flag and
//! threaded through command handlers.  [`format_output`] converts any
//! [`serde::Serialize`] value into the requested format string.

use std::fmt;

use serde::Serialize;

// ---------------------------------------------------------------------------
// OutputFormat
// ---------------------------------------------------------------------------

/// The output format requested by the user via `--format`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutputFormat {
    /// Human-readable, plain-text output (default).
    #[default]
    Human,

    /// Compact JSON (`{"key":"value"}`).
    Json,

    /// Pretty-printed JSON (indented with 2 spaces).
    JsonPretty,

    /// ASCII table layout (falls back to human when width is unknown).
    Table,
}

impl fmt::Display for OutputFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Human => f.write_str("human"),
            Self::Json => f.write_str("json"),
            Self::JsonPretty => f.write_str("json-pretty"),
            Self::Table => f.write_str("table"),
        }
    }
}

impl std::str::FromStr for OutputFormat {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "human" => Ok(Self::Human),
            "json" => Ok(Self::Json),
            "json-pretty" | "jsonpretty" => Ok(Self::JsonPretty),
            "table" => Ok(Self::Table),
            other => Err(format!(
                "unknown format '{other}'; expected one of: human, json, json-pretty, table"
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// Metadata envelope
// ---------------------------------------------------------------------------

/// Metadata envelope attached to JSON output when the format includes `_meta`.
// Private implementation detail used by format_output / meta().
#[allow(dead_code)]
#[derive(Debug, Serialize)]
struct Meta<'a> {
    command: &'a str,
    version: &'a str,
    timestamp: String,
}

/// Wrapper that injects `_meta` into the serialised object.
// Private implementation detail used by format_output.
#[allow(dead_code)]
#[derive(Debug, Serialize)]
struct WithMeta<'a, T: Serialize> {
    #[serde(flatten)]
    data: &'a T,
    _meta: Meta<'a>,
}

// ---------------------------------------------------------------------------
// format_output
// ---------------------------------------------------------------------------

/// Serialise `data` according to `format`.
///
/// For JSON formats the output includes a `_meta` object with the command
/// name, binary version, and an ISO-8601 timestamp.
///
/// Returns an owned [`String`] ready to print with `println!`.
///
/// # Panics
///
/// Panics if `serde_json` serialisation fails (should never happen for
/// well-formed types).
// Public API — will be used by command handlers as the CLI matures.
#[allow(dead_code)]
#[must_use]
pub fn format_output<T: Serialize>(data: &T, format: OutputFormat, command: &str) -> String {
    match format {
        OutputFormat::Human | OutputFormat::Table => {
            // Callers handle human/table output themselves; this path is for
            // programmatic use when the format flag is forced to a non-JSON
            // variant but the caller still wants a serialised representation.
            serde_json::to_string_pretty(data).unwrap_or_else(|e| format!("{{\"error\":\"{e}\"}}"))
        }

        OutputFormat::Json => {
            let envelope = WithMeta {
                data,
                _meta: meta(command),
            };
            serde_json::to_string(&envelope)
                .unwrap_or_else(|e| format!("{{\"error\":\"{e}\"}}"))
        }

        OutputFormat::JsonPretty => {
            let envelope = WithMeta {
                data,
                _meta: meta(command),
            };
            serde_json::to_string_pretty(&envelope)
                .unwrap_or_else(|e| format!("{{\"error\":\"{e}\"}}"))
        }
    }
}

// Private helper used only by format_output.
#[allow(dead_code)]
fn meta(command: &str) -> Meta<'_> {
    Meta {
        command,
        version: env!("CARGO_PKG_VERSION"),
        timestamp: chrono::Utc::now().to_rfc3339(),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[derive(Serialize)]
    struct Dummy {
        name: String,
        count: u32,
    }

    #[test]
    fn parse_all_formats() {
        assert_eq!("human".parse::<OutputFormat>().unwrap(), OutputFormat::Human);
        assert_eq!("json".parse::<OutputFormat>().unwrap(), OutputFormat::Json);
        assert_eq!(
            "json-pretty".parse::<OutputFormat>().unwrap(),
            OutputFormat::JsonPretty
        );
        assert_eq!(
            "table".parse::<OutputFormat>().unwrap(),
            OutputFormat::Table
        );
    }

    #[test]
    fn parse_unknown_returns_error() {
        assert!("xml".parse::<OutputFormat>().is_err());
    }

    #[test]
    fn json_output_contains_meta() {
        let d = Dummy {
            name: "test".into(),
            count: 42,
        };
        let out = format_output(&d, OutputFormat::Json, "test-cmd");
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["name"], "test");
        assert_eq!(v["count"], 42);
        assert!(v["_meta"]["command"].is_string());
        assert!(v["_meta"]["version"].is_string());
        assert!(v["_meta"]["timestamp"].is_string());
    }

    #[test]
    fn json_pretty_output_contains_meta() {
        let d = Dummy {
            name: "x".into(),
            count: 1,
        };
        let out = format_output(&d, OutputFormat::JsonPretty, "cmd");
        let v: Value = serde_json::from_str(&out).unwrap();
        assert!(v["_meta"].is_object());
    }

    #[test]
    fn display_roundtrips() {
        for fmt in [
            OutputFormat::Human,
            OutputFormat::Json,
            OutputFormat::JsonPretty,
            OutputFormat::Table,
        ] {
            let s = fmt.to_string();
            let parsed: OutputFormat = s.parse().unwrap();
            assert_eq!(parsed, fmt);
        }
    }
}
