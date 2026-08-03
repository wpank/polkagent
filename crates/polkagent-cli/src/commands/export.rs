//! `polkagent export` -- export data from the Polkagent database.
//!
//! Subcommands export runs, effects, artifacts, events, audit trails, and
//! configuration in JSON, CSV, or JSON Lines format.  Output is streamed
//! through a [`StreamingWriter`] so that arbitrarily large result sets never
//! need to be buffered entirely in memory.

use std::fmt;
use std::io::{self, BufWriter, Write};
use std::path::PathBuf;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use clap::{Args, Subcommand, ValueEnum};
use serde::Serialize;

use polkagent_store_sqlite::SqlitePool;

// ---------------------------------------------------------------------------
// Format enum
// ---------------------------------------------------------------------------

/// Output serialisation format for exported data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, ValueEnum)]
pub enum Format {
    /// Pretty-printed JSON array.
    #[default]
    Json,

    /// Comma-separated values with RFC 4180 escaping.
    Csv,

    /// Newline-delimited JSON (one object per line).
    #[value(name = "jsonl")]
    JsonLines,
}

impl fmt::Display for Format {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json => f.write_str("json"),
            Self::Csv => f.write_str("csv"),
            Self::JsonLines => f.write_str("jsonl"),
        }
    }
}

impl std::str::FromStr for Format {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "json" => Ok(Self::Json),
            "csv" => Ok(Self::Csv),
            "jsonl" | "jsonlines" | "json-lines" => Ok(Self::JsonLines),
            other => Err(format!(
                "unknown format '{other}'; expected one of: json, csv, jsonl"
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// Shared filter / config
// ---------------------------------------------------------------------------

/// Shared options present on every `export` subcommand.
#[derive(Debug, Clone, Args)]
pub struct ExportConfig {
    /// Output format.
    #[arg(long, value_enum, default_value = "json")]
    pub format: Format,

    /// Write output to a file instead of stdout.
    #[arg(long, value_name = "PATH")]
    pub output: Option<PathBuf>,

    /// Include only records created at or after this ISO-8601 datetime.
    #[arg(long, value_name = "DATETIME")]
    pub since: Option<String>,

    /// Include only records created at or before this ISO-8601 datetime.
    #[arg(long, value_name = "DATETIME")]
    pub until: Option<String>,

    /// Filter by agent ID.
    #[arg(long, value_name = "ID")]
    pub agent_id: Option<String>,

    /// Filter by run ID.
    #[arg(long, value_name = "ID")]
    pub run_id: Option<String>,

    /// Maximum number of records to export.
    #[arg(long, value_name = "N")]
    pub limit: Option<usize>,
}

#[allow(dead_code)]
impl ExportConfig {
    /// Parse `--since` into a [`DateTime<Utc>`], if provided.
    pub fn since_dt(&self) -> Result<Option<DateTime<Utc>>> {
        self.since
            .as_deref()
            .map(|s| {
                s.parse::<DateTime<Utc>>()
                    .with_context(|| format!("invalid --since datetime: {s}"))
            })
            .transpose()
    }

    /// Parse `--until` into a [`DateTime<Utc>`], if provided.
    pub fn until_dt(&self) -> Result<Option<DateTime<Utc>>> {
        self.until
            .as_deref()
            .map(|s| {
                s.parse::<DateTime<Utc>>()
                    .with_context(|| format!("invalid --until datetime: {s}"))
            })
            .transpose()
    }

    /// Build the SQL WHERE clause fragments and bind parameters from the
    /// shared filter fields.
    ///
    /// Returns `(clauses, params)` where each clause is a string like
    /// `"created_at >= ?N"` and each param is the corresponding string value.
    pub fn sql_filters(
        &self,
        param_offset: usize,
    ) -> Result<(Vec<String>, Vec<String>)> {
        let mut clauses = Vec::new();
        let mut params = Vec::new();
        let mut idx = param_offset;

        if let Some(since) = &self.since {
            idx += 1;
            clauses.push(format!("created_at >= ?{idx}"));
            params.push(since.clone());
        }
        if let Some(until) = &self.until {
            idx += 1;
            clauses.push(format!("created_at <= ?{idx}"));
            params.push(until.clone());
        }
        if let Some(agent_id) = &self.agent_id {
            idx += 1;
            clauses.push(format!("agent_id = ?{idx}"));
            params.push(agent_id.clone());
        }
        if let Some(run_id) = &self.run_id {
            idx += 1;
            clauses.push(format!("run_id = ?{idx}"));
            params.push(run_id.clone());
        }

        Ok((clauses, params))
    }

    /// Return the SQL `LIMIT` clause string, or an empty string if no limit.
    pub fn sql_limit(&self) -> String {
        match self.limit {
            Some(n) => format!(" LIMIT {n}"),
            None => String::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// ExportArgs (subcommand enum)
// ---------------------------------------------------------------------------

/// Export data from the Polkagent database.
#[derive(Debug, Subcommand)]
pub enum ExportArgs {
    /// Export run history.
    Runs(ExportSubCmd),

    /// Export effect log.
    Effects(ExportSubCmd),

    /// Export artifact metadata.
    Artifacts(ExportArtifactsCmd),

    /// Export event log.
    Events(ExportSubCmd),

    /// Export audit trail.
    Audit(ExportSubCmd),

    /// Export current configuration as canonical TOML.
    Config(ExportConfigOnlyCmd),
}

/// A generic subcommand that only carries the shared [`ExportConfig`].
#[derive(Debug, Args)]
pub struct ExportSubCmd {
    #[command(flatten)]
    pub config: ExportConfig,
}

/// Artifacts subcommand with an extra `--include-bodies` flag.
#[derive(Debug, Args)]
pub struct ExportArtifactsCmd {
    #[command(flatten)]
    pub config: ExportConfig,

    /// Include artifact bodies in the output (may be large).
    #[arg(long)]
    pub include_bodies: bool,
}

/// Config subcommand has no filters -- it just dumps the resolved config.
#[derive(Debug, Args)]
pub struct ExportConfigOnlyCmd {
    /// Write output to a file instead of stdout.
    #[arg(long, value_name = "PATH")]
    pub output: Option<PathBuf>,
}

// ---------------------------------------------------------------------------
// StreamingWriter
// ---------------------------------------------------------------------------

/// A writer that streams records in the chosen format without buffering the
/// entire result set in memory.
///
/// Usage:
///
/// ```ignore
/// let mut w = StreamingWriter::new(format, &["id", "name"]);
/// w.begin(&mut out)?;
/// for record in records {
///     w.write_record(&mut out, &record)?;
/// }
/// w.end(&mut out)?;
/// ```
pub struct StreamingWriter {
    format: Format,
    columns: Vec<String>,
    count: usize,
}

#[allow(dead_code)]
impl StreamingWriter {
    /// Create a new streaming writer.
    ///
    /// `columns` is used only for CSV output (header row).
    pub fn new(format: Format, columns: &[&str]) -> Self {
        Self {
            format,
            columns: columns.iter().map(|c| (*c).to_owned()).collect(),
            count: 0,
        }
    }

    /// Write the format prologue (JSON `[`, CSV header, or nothing for JSONL).
    pub fn begin<W: Write>(&self, w: &mut W) -> io::Result<()> {
        match self.format {
            Format::Json => w.write_all(b"[\n"),
            Format::Csv => {
                let header = self
                    .columns
                    .iter()
                    .map(|c| csv_escape_field(c))
                    .collect::<Vec<_>>()
                    .join(",");
                writeln!(w, "{header}")
            }
            Format::JsonLines => Ok(()),
        }
    }

    /// Write a single record.
    pub fn write_record<W: Write, T: Serialize>(
        &mut self,
        w: &mut W,
        record: &T,
    ) -> io::Result<()> {
        match self.format {
            Format::Json => {
                if self.count > 0 {
                    w.write_all(b",\n")?;
                }
                let json = serde_json::to_string_pretty(record)
                    .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
                // Indent each line by two spaces inside the array.
                for (i, line) in json.lines().enumerate() {
                    if i > 0 {
                        w.write_all(b"\n")?;
                    }
                    write!(w, "  {line}")?;
                }
            }
            Format::Csv => {
                let value = serde_json::to_value(record)
                    .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
                let fields: Vec<String> = self
                    .columns
                    .iter()
                    .map(|col| {
                        value
                            .get(col)
                            .map(value_to_csv_cell)
                            .unwrap_or_default()
                    })
                    .collect();
                let line = fields
                    .iter()
                    .map(|f| csv_escape_field(f))
                    .collect::<Vec<_>>()
                    .join(",");
                writeln!(w, "{line}")?;
            }
            Format::JsonLines => {
                let json = serde_json::to_string(record)
                    .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
                writeln!(w, "{json}")?;
            }
        }
        self.count += 1;
        Ok(())
    }

    /// Write the format epilogue (JSON `]` or nothing).
    pub fn end<W: Write>(&self, w: &mut W) -> io::Result<()> {
        match self.format {
            Format::Json => {
                if self.count > 0 {
                    w.write_all(b"\n")?;
                }
                w.write_all(b"]\n")
            }
            Format::Csv | Format::JsonLines => Ok(()),
        }
    }

    /// Return the number of records written so far.
    pub fn count(&self) -> usize {
        self.count
    }
}

// ---------------------------------------------------------------------------
// CSV helpers
// ---------------------------------------------------------------------------

/// Escape a field value for CSV output per RFC 4180.
///
/// Fields containing commas, double-quotes, or newlines are wrapped in
/// double-quotes, with any embedded double-quotes doubled.
pub fn csv_escape_field(field: &str) -> String {
    if field.contains(',') || field.contains('"') || field.contains('\n') || field.contains('\r') {
        let escaped = field.replace('"', "\"\"");
        format!("\"{escaped}\"")
    } else {
        field.to_owned()
    }
}

/// Convert a [`serde_json::Value`] into a plain string suitable for a CSV
/// cell.  Strings are unquoted; nulls become empty; objects/arrays become
/// their JSON representation.
fn value_to_csv_cell(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Null => String::new(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Standalone format helpers
// ---------------------------------------------------------------------------

/// Serialise a slice of records as a pretty-printed JSON array.
#[allow(dead_code)]
pub fn to_json<T: Serialize>(records: &[T]) -> Result<String> {
    serde_json::to_string_pretty(records).context("serialising to JSON")
}

/// Serialise records as CSV with the given column headers.
///
/// Columns are extracted from each record's serialised JSON form by key name.
#[allow(dead_code)]
pub fn to_csv<T: Serialize>(records: &[T], columns: &[&str]) -> Result<String> {
    let mut buf = Vec::new();
    let mut w = StreamingWriter::new(Format::Csv, columns);
    w.begin(&mut buf)?;
    for rec in records {
        w.write_record(&mut buf, rec)?;
    }
    w.end(&mut buf)?;
    String::from_utf8(buf).context("CSV output was not valid UTF-8")
}

/// Serialise records as newline-delimited JSON (JSON Lines).
#[allow(dead_code)]
pub fn to_jsonl<T: Serialize>(records: &[T]) -> Result<String> {
    let mut buf = Vec::new();
    let mut w = StreamingWriter::new(Format::JsonLines, &[]);
    w.begin(&mut buf)?;
    for rec in records {
        w.write_record(&mut buf, rec)?;
    }
    w.end(&mut buf)?;
    String::from_utf8(buf).context("JSONL output was not valid UTF-8")
}

// ---------------------------------------------------------------------------
// Row types (for database query results)
// ---------------------------------------------------------------------------

/// A row from the `runs` table.
#[derive(Debug, Clone, Serialize)]
pub struct RunRow {
    pub id: String,
    pub agent_id: String,
    pub state: String,
    pub params_json: String,
    pub created_at: String,
    pub updated_at: String,
}

/// A row from the `effect_intents` table.
#[derive(Debug, Clone, Serialize)]
pub struct EffectRow {
    pub id: String,
    pub run_id: String,
    pub turn_id: String,
    pub kind: String,
    pub claimed_by: String,
    pub params_json: String,
    pub created_at: String,
}

/// A row from the `artifacts` table.
#[derive(Debug, Clone, Serialize)]
pub struct ArtifactRow {
    pub id: String,
    pub run_id: String,
    pub kind: String,
    pub digest_hex: String,
    pub size_bytes: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    pub created_at: String,
}

/// A row from the `run_events` table.
#[derive(Debug, Clone, Serialize)]
pub struct EventRow {
    pub sequence: i64,
    pub run_id: String,
    pub kind: String,
    pub data_json: String,
    pub timestamp: String,
}

/// A row from the `run_events` table (used for audit export).
#[derive(Debug, Clone, Serialize)]
pub struct AuditRow {
    pub id: String,
    pub actor: String,
    pub action: String,
    pub target: String,
    pub detail: String,
    pub timestamp: String,
}

// ---------------------------------------------------------------------------
// Open output destination
// ---------------------------------------------------------------------------

/// Open the output destination: a file if `--output` was specified, or a
/// buffered stdout wrapper otherwise.
fn open_output(path: &Option<PathBuf>) -> Result<Box<dyn Write>> {
    match path {
        Some(p) => {
            let file = std::fs::File::create(p)
                .with_context(|| format!("creating output file: {}", p.display()))?;
            Ok(Box::new(BufWriter::new(file)))
        }
        None => Ok(Box::new(BufWriter::new(io::stdout().lock()))),
    }
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Execute the `export` subcommand.
pub fn run(cmd: &ExportArgs, pool: &SqlitePool) -> Result<()> {
    match cmd {
        ExportArgs::Runs(sub) => export_runs(&sub.config, pool),
        ExportArgs::Effects(sub) => export_effects(&sub.config, pool),
        ExportArgs::Artifacts(sub) => export_artifacts(&sub.config, sub.include_bodies, pool),
        ExportArgs::Events(sub) => export_events(&sub.config, pool),
        ExportArgs::Audit(sub) => export_audit(&sub.config, pool),
        ExportArgs::Config(sub) => export_config(&sub.output),
    }
}

// ---------------------------------------------------------------------------
// Subcommand implementations
// ---------------------------------------------------------------------------

fn export_runs(cfg: &ExportConfig, pool: &SqlitePool) -> Result<()> {
    let reader = pool
        .reader()
        .map_err(|e| anyhow::anyhow!("opening database reader: {e}"))?;

    let (where_clauses, params) = cfg.sql_filters(0)?;
    let where_sql = if where_clauses.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", where_clauses.join(" AND "))
    };
    let limit_sql = cfg.sql_limit();

    let sql = format!(
        "SELECT id, agent_id, state, params_json, created_at, updated_at \
         FROM runs{where_sql} ORDER BY created_at DESC{limit_sql}"
    );

    let mut stmt = reader.prepare(&sql)?;
    let row_iter = stmt
        .query_map(rusqlite::params_from_iter(params.iter()), |row| {
            Ok(RunRow {
                id: row.get(0)?,
                agent_id: row.get(1)?,
                state: row.get(2)?,
                params_json: row.get(3)?,
                created_at: row.get(4)?,
                updated_at: row.get(5)?,
            })
        })?;

    let columns = &["id", "agent_id", "state", "params_json", "created_at", "updated_at"];
    let mut out = open_output(&cfg.output)?;
    let mut writer = StreamingWriter::new(cfg.format, columns);
    writer.begin(&mut out)?;

    for row in row_iter {
        let row = row?;
        writer.write_record(&mut out, &row)?;
    }

    writer.end(&mut out)?;
    Ok(())
}

fn export_effects(cfg: &ExportConfig, pool: &SqlitePool) -> Result<()> {
    let reader = pool
        .reader()
        .map_err(|e| anyhow::anyhow!("opening database reader: {e}"))?;

    let (where_clauses, params) = cfg.sql_filters(0)?;
    let where_sql = if where_clauses.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", where_clauses.join(" AND "))
    };
    let limit_sql = cfg.sql_limit();

    let sql = format!(
        "SELECT id, run_id, turn_id, kind, claimed_by, params_json, created_at \
         FROM effect_intents{where_sql} ORDER BY created_at DESC{limit_sql}"
    );

    let mut stmt = reader.prepare(&sql)?;
    let row_iter = stmt
        .query_map(rusqlite::params_from_iter(params.iter()), |row| {
            Ok(EffectRow {
                id: row.get(0)?,
                run_id: row.get(1)?,
                turn_id: row.get(2)?,
                kind: row.get(3)?,
                claimed_by: row.get(4)?,
                params_json: row.get(5)?,
                created_at: row.get(6)?,
            })
        })?;

    let columns = &[
        "id",
        "run_id",
        "turn_id",
        "kind",
        "claimed_by",
        "params_json",
        "created_at",
    ];
    let mut out = open_output(&cfg.output)?;
    let mut writer = StreamingWriter::new(cfg.format, columns);
    writer.begin(&mut out)?;

    for row in row_iter {
        let row = row?;
        writer.write_record(&mut out, &row)?;
    }

    writer.end(&mut out)?;
    Ok(())
}

fn export_artifacts(
    cfg: &ExportConfig,
    include_bodies: bool,
    pool: &SqlitePool,
) -> Result<()> {
    let reader = pool
        .reader()
        .map_err(|e| anyhow::anyhow!("opening database reader: {e}"))?;

    let (where_clauses, params) = cfg.sql_filters(0)?;
    let where_sql = if where_clauses.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", where_clauses.join(" AND "))
    };
    let limit_sql = cfg.sql_limit();

    let body_col = if include_bodies { ", body" } else { "" };
    let sql = format!(
        "SELECT id, run_id, kind, digest_hex, size_bytes{body_col}, created_at \
         FROM artifacts{where_sql} ORDER BY created_at DESC{limit_sql}"
    );

    let mut stmt = reader.prepare(&sql)?;
    let row_iter = stmt
        .query_map(rusqlite::params_from_iter(params.iter()), |row| {
            if include_bodies {
                Ok(ArtifactRow {
                    id: row.get(0)?,
                    run_id: row.get(1)?,
                    kind: row.get(2)?,
                    digest_hex: row.get(3)?,
                    size_bytes: row.get(4)?,
                    body: row.get(5)?,
                    created_at: row.get(6)?,
                })
            } else {
                Ok(ArtifactRow {
                    id: row.get(0)?,
                    run_id: row.get(1)?,
                    kind: row.get(2)?,
                    digest_hex: row.get(3)?,
                    size_bytes: row.get(4)?,
                    body: None,
                    created_at: row.get(5)?,
                })
            }
        })?;

    let mut columns: Vec<&str> = vec![
        "id",
        "run_id",
        "kind",
        "digest_hex",
        "size_bytes",
    ];
    if include_bodies {
        columns.push("body");
    }
    columns.push("created_at");

    let mut out = open_output(&cfg.output)?;
    let mut writer = StreamingWriter::new(cfg.format, &columns);
    writer.begin(&mut out)?;

    for row in row_iter {
        let row = row?;
        writer.write_record(&mut out, &row)?;
    }

    writer.end(&mut out)?;
    Ok(())
}

fn export_events(cfg: &ExportConfig, pool: &SqlitePool) -> Result<()> {
    let reader = pool
        .reader()
        .map_err(|e| anyhow::anyhow!("opening database reader: {e}"))?;

    let (where_clauses, params) = cfg.sql_filters(0)?;
    let where_sql = if where_clauses.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", where_clauses.join(" AND "))
    };
    let limit_sql = cfg.sql_limit();

    let sql = format!(
        "SELECT sequence, run_id, kind, data_json, timestamp \
         FROM run_events{where_sql} ORDER BY sequence ASC{limit_sql}"
    );

    let mut stmt = reader.prepare(&sql)?;
    let row_iter = stmt
        .query_map(rusqlite::params_from_iter(params.iter()), |row| {
            Ok(EventRow {
                sequence: row.get(0)?,
                run_id: row.get(1)?,
                kind: row.get(2)?,
                data_json: row.get(3)?,
                timestamp: row.get(4)?,
            })
        })?;

    // Events default to JSONL format when no explicit format is given, but we
    // respect the user's choice.
    let columns = &["sequence", "run_id", "kind", "data_json", "timestamp"];
    let mut out = open_output(&cfg.output)?;
    let mut writer = StreamingWriter::new(cfg.format, columns);
    writer.begin(&mut out)?;

    for row in row_iter {
        let row = row?;
        writer.write_record(&mut out, &row)?;
    }

    writer.end(&mut out)?;
    Ok(())
}

fn export_audit(cfg: &ExportConfig, pool: &SqlitePool) -> Result<()> {
    let reader = pool
        .reader()
        .map_err(|e| anyhow::anyhow!("opening database reader: {e}"))?;

    let (where_clauses, params) = cfg.sql_filters(0)?;
    let where_sql = if where_clauses.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", where_clauses.join(" AND "))
    };
    let limit_sql = cfg.sql_limit();

    let sql = format!(
        "SELECT id, run_id, kind, data_json, timestamp \
         FROM run_events{where_sql} ORDER BY timestamp DESC{limit_sql}"
    );

    let mut stmt = reader.prepare(&sql)?;
    let row_iter = stmt
        .query_map(rusqlite::params_from_iter(params.iter()), |row| {
            Ok(AuditRow {
                id: row.get(0)?,
                actor: row.get::<_, String>(1)?,
                action: row.get(2)?,
                target: String::new(),
                detail: row.get(3)?,
                timestamp: row.get(4)?,
            })
        })?;

    let columns = &["id", "actor", "action", "target", "detail", "timestamp"];
    let mut out = open_output(&cfg.output)?;
    let mut writer = StreamingWriter::new(cfg.format, columns);
    writer.begin(&mut out)?;

    for row in row_iter {
        let row = row?;
        writer.write_record(&mut out, &row)?;
    }

    writer.end(&mut out)?;
    Ok(())
}

fn export_config(output_path: &Option<PathBuf>) -> Result<()> {
    let config = polkagent_config::Config::default();
    let toml_str =
        toml::to_string_pretty(&config).context("serialising config to TOML")?;

    let mut out = open_output(output_path)?;
    out.write_all(toml_str.as_bytes())?;
    if !toml_str.ends_with('\n') {
        out.write_all(b"\n")?;
    }
    out.flush()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Datelike;

    // -- Format parsing ---------------------------------------------------

    #[test]
    fn parse_format_json() {
        assert_eq!("json".parse::<Format>().unwrap(), Format::Json);
    }

    #[test]
    fn parse_format_csv() {
        assert_eq!("csv".parse::<Format>().unwrap(), Format::Csv);
    }

    #[test]
    fn parse_format_jsonl() {
        assert_eq!("jsonl".parse::<Format>().unwrap(), Format::JsonLines);
    }

    #[test]
    fn parse_format_jsonlines_alias() {
        assert_eq!("jsonlines".parse::<Format>().unwrap(), Format::JsonLines);
        assert_eq!("json-lines".parse::<Format>().unwrap(), Format::JsonLines);
    }

    #[test]
    fn parse_format_case_insensitive() {
        assert_eq!("JSON".parse::<Format>().unwrap(), Format::Json);
        assert_eq!("Csv".parse::<Format>().unwrap(), Format::Csv);
        assert_eq!("JSONL".parse::<Format>().unwrap(), Format::JsonLines);
    }

    #[test]
    fn parse_format_unknown_returns_error() {
        let err = "xml".parse::<Format>().unwrap_err();
        assert!(err.contains("unknown format"));
    }

    #[test]
    fn format_display_roundtrip() {
        for fmt in [Format::Json, Format::Csv, Format::JsonLines] {
            let s = fmt.to_string();
            let parsed: Format = s.parse().unwrap();
            assert_eq!(parsed, fmt);
        }
    }

    // -- CSV escaping -----------------------------------------------------

    #[test]
    fn csv_escape_plain_field() {
        assert_eq!(csv_escape_field("hello"), "hello");
    }

    #[test]
    fn csv_escape_field_with_comma() {
        assert_eq!(csv_escape_field("a,b"), "\"a,b\"");
    }

    #[test]
    fn csv_escape_field_with_quotes() {
        assert_eq!(csv_escape_field("say \"hi\""), "\"say \"\"hi\"\"\"");
    }

    #[test]
    fn csv_escape_field_with_newline() {
        assert_eq!(csv_escape_field("line1\nline2"), "\"line1\nline2\"");
    }

    #[test]
    fn csv_escape_field_with_carriage_return() {
        assert_eq!(csv_escape_field("a\rb"), "\"a\rb\"");
    }

    #[test]
    fn csv_escape_mixed_special_chars() {
        // Field with both comma and quotes.
        let result = csv_escape_field("a,\"b\"");
        assert_eq!(result, "\"a,\"\"b\"\"\"");
    }

    #[test]
    fn csv_escape_empty_string() {
        assert_eq!(csv_escape_field(""), "");
    }

    // -- value_to_csv_cell ------------------------------------------------

    #[test]
    fn csv_cell_null_becomes_empty() {
        assert_eq!(value_to_csv_cell(&serde_json::Value::Null), "");
    }

    #[test]
    fn csv_cell_bool() {
        assert_eq!(
            value_to_csv_cell(&serde_json::Value::Bool(true)),
            "true"
        );
    }

    #[test]
    fn csv_cell_number() {
        let v = serde_json::json!(42);
        assert_eq!(value_to_csv_cell(&v), "42");
    }

    #[test]
    fn csv_cell_string() {
        let v = serde_json::json!("hello");
        assert_eq!(value_to_csv_cell(&v), "hello");
    }

    #[test]
    fn csv_cell_object_becomes_json() {
        let v = serde_json::json!({"key": "val"});
        let cell = value_to_csv_cell(&v);
        assert!(cell.contains("key"));
        // Should be valid JSON.
        let _: serde_json::Value = serde_json::from_str(&cell).unwrap();
    }

    // -- JSON format helpers ----------------------------------------------

    #[test]
    fn to_json_produces_valid_json_array() {
        let records = vec![
            RunRow {
                id: "r1".into(),
                agent_id: "a1".into(),
                state: "completed".into(),
                params_json: "hello".into(),
                created_at: "2024-01-01T00:00:00Z".into(),
                updated_at: "2024-01-01T00:00:00Z".into(),
            },
            RunRow {
                id: "r2".into(),
                agent_id: "a2".into(),
                state: "running".into(),
                params_json: "world".into(),
                created_at: "2024-01-02T00:00:00Z".into(),
                updated_at: "2024-01-02T00:00:00Z".into(),
            },
        ];
        let json = to_json(&records).unwrap();
        let parsed: Vec<serde_json::Value> = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0]["id"], "r1");
        assert_eq!(parsed[1]["state"], "running");
    }

    #[test]
    fn to_json_empty_records() {
        let records: Vec<RunRow> = vec![];
        let json = to_json(&records).unwrap();
        let parsed: Vec<serde_json::Value> = serde_json::from_str(&json).unwrap();
        assert!(parsed.is_empty());
    }

    // -- JSONL format helper ----------------------------------------------

    #[test]
    fn to_jsonl_one_object_per_line() {
        let records = vec![
            EventRow {
                sequence: 1,
                run_id: "r1".into(),
                kind: "RunCreated".into(),
                data_json: "{}".into(),
                timestamp: "2024-01-01T00:00:00Z".into(),
            },
            EventRow {
                sequence: 2,
                run_id: "r1".into(),
                kind: "RunCompleted".into(),
                data_json: "{}".into(),
                timestamp: "2024-01-01T00:01:00Z".into(),
            },
        ];
        let jsonl = to_jsonl(&records).unwrap();
        let lines: Vec<&str> = jsonl.trim().lines().collect();
        assert_eq!(lines.len(), 2);
        // Each line must be valid JSON.
        for line in &lines {
            let _: serde_json::Value = serde_json::from_str(line).unwrap();
        }
    }

    // -- CSV format helper ------------------------------------------------

    #[test]
    fn to_csv_header_and_rows() {
        let records = vec![
            AuditRow {
                id: "1".into(),
                actor: "system".into(),
                action: "create".into(),
                target: "agent".into(),
                detail: "created agent".into(),
                timestamp: "2024-01-01T00:00:00Z".into(),
            },
        ];
        let csv = to_csv(&records, &["id", "actor", "action"]).unwrap();
        let lines: Vec<&str> = csv.trim().lines().collect();
        assert_eq!(lines.len(), 2); // header + 1 row
        assert_eq!(lines[0], "id,actor,action");
        assert_eq!(lines[1], "1,system,create");
    }

    #[test]
    fn to_csv_escapes_values_with_commas() {
        let records = vec![AuditRow {
            id: "1".into(),
            actor: "user,admin".into(),
            action: "update".into(),
            target: "config".into(),
            detail: "changed".into(),
            timestamp: "2024-01-01T00:00:00Z".into(),
        }];
        let csv = to_csv(&records, &["id", "actor"]).unwrap();
        let lines: Vec<&str> = csv.trim().lines().collect();
        assert_eq!(lines[1], "1,\"user,admin\"");
    }

    // -- StreamingWriter --------------------------------------------------

    #[test]
    fn streaming_writer_json_wraps_in_array() {
        let mut buf = Vec::new();
        let mut w = StreamingWriter::new(Format::Json, &["id", "name"]);
        w.begin(&mut buf).unwrap();

        #[derive(Serialize)]
        struct R {
            id: u32,
            name: String,
        }

        w.write_record(&mut buf, &R { id: 1, name: "a".into() })
            .unwrap();
        w.write_record(&mut buf, &R { id: 2, name: "b".into() })
            .unwrap();
        w.end(&mut buf).unwrap();

        let output = String::from_utf8(buf).unwrap();
        let parsed: Vec<serde_json::Value> = serde_json::from_str(&output).unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(w.count(), 2);
    }

    #[test]
    fn streaming_writer_csv_outputs_header_plus_rows() {
        let mut buf = Vec::new();
        let mut w = StreamingWriter::new(Format::Csv, &["x", "y"]);
        w.begin(&mut buf).unwrap();

        #[derive(Serialize)]
        struct P {
            x: i32,
            y: i32,
        }

        w.write_record(&mut buf, &P { x: 10, y: 20 }).unwrap();
        w.end(&mut buf).unwrap();

        let output = String::from_utf8(buf).unwrap();
        let lines: Vec<&str> = output.trim().lines().collect();
        assert_eq!(lines[0], "x,y");
        assert_eq!(lines[1], "10,20");
        assert_eq!(w.count(), 1);
    }

    #[test]
    fn streaming_writer_jsonl_one_per_line() {
        let mut buf = Vec::new();
        let mut w = StreamingWriter::new(Format::JsonLines, &[]);
        w.begin(&mut buf).unwrap();

        #[derive(Serialize)]
        struct Item {
            v: u32,
        }

        w.write_record(&mut buf, &Item { v: 1 }).unwrap();
        w.write_record(&mut buf, &Item { v: 2 }).unwrap();
        w.write_record(&mut buf, &Item { v: 3 }).unwrap();
        w.end(&mut buf).unwrap();

        let output = String::from_utf8(buf).unwrap();
        let lines: Vec<&str> = output.trim().lines().collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(w.count(), 3);
    }

    #[test]
    fn streaming_writer_empty_json_produces_empty_array() {
        let mut buf = Vec::new();
        let w = StreamingWriter::new(Format::Json, &[]);
        w.begin(&mut buf).unwrap();
        w.end(&mut buf).unwrap();

        let output = String::from_utf8(buf).unwrap();
        let parsed: Vec<serde_json::Value> = serde_json::from_str(&output).unwrap();
        assert!(parsed.is_empty());
        assert_eq!(w.count(), 0);
    }

    // -- ExportConfig filters ---------------------------------------------

    #[test]
    fn sql_filters_empty_config() {
        let cfg = ExportConfig {
            format: Format::Json,
            output: None,
            since: None,
            until: None,
            agent_id: None,
            run_id: None,
            limit: None,
        };
        let (clauses, params) = cfg.sql_filters(0).unwrap();
        assert!(clauses.is_empty());
        assert!(params.is_empty());
    }

    #[test]
    fn sql_filters_all_fields() {
        let cfg = ExportConfig {
            format: Format::Json,
            output: None,
            since: Some("2024-01-01T00:00:00Z".into()),
            until: Some("2024-12-31T23:59:59Z".into()),
            agent_id: Some("agent-1".into()),
            run_id: Some("run-1".into()),
            limit: None,
        };
        let (clauses, params) = cfg.sql_filters(0).unwrap();
        assert_eq!(clauses.len(), 4);
        assert_eq!(params.len(), 4);
        assert!(clauses[0].contains("created_at >= ?1"));
        assert!(clauses[1].contains("created_at <= ?2"));
        assert!(clauses[2].contains("agent_id = ?3"));
        assert!(clauses[3].contains("run_id = ?4"));
    }

    #[test]
    fn sql_filters_respects_param_offset() {
        let cfg = ExportConfig {
            format: Format::Json,
            output: None,
            since: Some("2024-06-01T00:00:00Z".into()),
            until: None,
            agent_id: None,
            run_id: None,
            limit: None,
        };
        let (clauses, _) = cfg.sql_filters(5).unwrap();
        assert_eq!(clauses.len(), 1);
        assert!(clauses[0].contains("?6"));
    }

    #[test]
    fn sql_limit_none_returns_empty() {
        let cfg = ExportConfig {
            format: Format::Json,
            output: None,
            since: None,
            until: None,
            agent_id: None,
            run_id: None,
            limit: None,
        };
        assert_eq!(cfg.sql_limit(), "");
    }

    #[test]
    fn sql_limit_some_returns_clause() {
        let cfg = ExportConfig {
            format: Format::Json,
            output: None,
            since: None,
            until: None,
            agent_id: None,
            run_id: None,
            limit: Some(50),
        };
        assert_eq!(cfg.sql_limit(), " LIMIT 50");
    }

    // -- Datetime parsing -------------------------------------------------

    #[test]
    fn since_dt_parses_valid_datetime() {
        let cfg = ExportConfig {
            format: Format::Json,
            output: None,
            since: Some("2024-06-15T12:30:00Z".into()),
            until: None,
            agent_id: None,
            run_id: None,
            limit: None,
        };
        let dt = cfg.since_dt().unwrap().unwrap();
        assert_eq!(dt.year(), 2024);
        assert_eq!(dt.month(), 6);
    }

    #[test]
    fn since_dt_none_returns_none() {
        let cfg = ExportConfig {
            format: Format::Json,
            output: None,
            since: None,
            until: None,
            agent_id: None,
            run_id: None,
            limit: None,
        };
        assert!(cfg.since_dt().unwrap().is_none());
    }

    #[test]
    fn since_dt_invalid_returns_error() {
        let cfg = ExportConfig {
            format: Format::Json,
            output: None,
            since: Some("not-a-date".into()),
            until: None,
            agent_id: None,
            run_id: None,
            limit: None,
        };
        assert!(cfg.since_dt().is_err());
    }

    #[test]
    fn until_dt_parses_valid_datetime() {
        let cfg = ExportConfig {
            format: Format::Json,
            output: None,
            since: None,
            until: Some("2025-12-31T23:59:59Z".into()),
            agent_id: None,
            run_id: None,
            limit: None,
        };
        let dt = cfg.until_dt().unwrap().unwrap();
        assert_eq!(dt.year(), 2025);
    }

    // -- Row serialization ------------------------------------------------

    #[test]
    fn run_row_serializes_all_fields() {
        let row = RunRow {
            id: "abc".into(),
            agent_id: "def".into(),
            state: "completed".into(),
            params_json: "do stuff".into(),
            created_at: "2024-01-01T00:00:00Z".into(),
            updated_at: "2024-01-01T00:00:00Z".into(),
        };
        let v = serde_json::to_value(&row).unwrap();
        assert_eq!(v["id"], "abc");
        assert_eq!(v["params_json"], "do stuff");
    }

    #[test]
    fn artifact_row_skips_body_when_none() {
        let row = ArtifactRow {
            id: "a1".into(),
            run_id: "r1".into(),
            kind: "file.txt".into(),
            digest_hex: "text/plain".into(),
            size_bytes: 1024,
            body: None,
            created_at: "2024-01-01T00:00:00Z".into(),
        };
        let v = serde_json::to_value(&row).unwrap();
        assert!(v.get("body").is_none());
    }

    #[test]
    fn artifact_row_includes_body_when_some() {
        let row = ArtifactRow {
            id: "a1".into(),
            run_id: "r1".into(),
            kind: "file.txt".into(),
            digest_hex: "text/plain".into(),
            size_bytes: 5,
            body: Some("hello".into()),
            created_at: "2024-01-01T00:00:00Z".into(),
        };
        let v = serde_json::to_value(&row).unwrap();
        assert_eq!(v["body"], "hello");
    }
}
