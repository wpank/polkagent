//! Episode logger — append-only JSONL writer for conversation episodes.
//!
//! Provides [`EpisodeLogger`] for writing [`EpisodeEntry`] records to JSONL
//! files with automatic rotation (by size) and optional zstd compression of
//! rotated files.
//!
//! Before writing, entries are passed through a [`StandardRedactor`] that
//! strips secrets, PII, API keys, and other sensitive data using a set of
//! configurable [`RedactionRule`]s.
//!
//! Replay is supported via [`replay`] which streams entries from a JSONL file.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use regex::Regex;
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::error::{MemoryError, MemoryResult};
use crate::types::EpisodeId;

// ---------------------------------------------------------------------------
// Role
// ---------------------------------------------------------------------------

/// The role of a participant in an episode entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// System message (instructions, context).
    System,
    /// User message.
    User,
    /// Assistant (agent) message.
    Assistant,
    /// Tool invocation result.
    Tool,
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::System => write!(f, "system"),
            Self::User => write!(f, "user"),
            Self::Assistant => write!(f, "assistant"),
            Self::Tool => write!(f, "tool"),
        }
    }
}

impl std::str::FromStr for Role {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "system" => Ok(Self::System),
            "user" => Ok(Self::User),
            "assistant" => Ok(Self::Assistant),
            "tool" => Ok(Self::Tool),
            other => Err(format!("unknown role: {other}")),
        }
    }
}

// ---------------------------------------------------------------------------
// ToolCallRecord
// ---------------------------------------------------------------------------

/// A record of a tool invocation made during an episode turn.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRecord {
    /// The name of the tool that was invoked.
    pub name: String,
    /// The input arguments (as JSON value).
    pub input: serde_json::Value,
    /// The output of the tool invocation (as JSON value), if available.
    pub output: Option<serde_json::Value>,
}

// ---------------------------------------------------------------------------
// EpisodeEntry
// ---------------------------------------------------------------------------

/// A single turn in an episode conversation log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpisodeEntry {
    /// When this entry was recorded.
    pub timestamp: DateTime<Utc>,
    /// The role of the participant.
    pub role: Role,
    /// The textual content of this turn.
    pub content: String,
    /// Tool calls made during this turn (empty if none).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCallRecord>,
    /// Arbitrary metadata attached to the entry.
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub metadata: serde_json::Value,
}

// ---------------------------------------------------------------------------
// RedactionRule
// ---------------------------------------------------------------------------

/// A single redaction rule that matches and replaces sensitive patterns.
#[derive(Debug, Clone)]
pub struct RedactionRule {
    /// The compiled regex pattern.
    pub pattern: Regex,
    /// The replacement string (may use capture group references like `$1`).
    pub replacement: String,
    /// Human-readable label for this rule (e.g. "`api_key`", "email").
    pub label: String,
}

impl RedactionRule {
    /// Create a new redaction rule.
    ///
    /// # Panics
    ///
    /// Panics if `pattern` is not a valid regex.
    pub fn new(pattern: &str, replacement: &str, label: &str) -> Self {
        Self {
            pattern: Regex::new(pattern)
                .unwrap_or_else(|e| panic!("invalid redaction pattern '{pattern}': {e}")),
            replacement: replacement.to_string(),
            label: label.to_string(),
        }
    }

    /// Apply this rule to the given text, returning the redacted version.
    pub fn apply(&self, text: &str) -> String {
        self.pattern
            .replace_all(text, self.replacement.as_str())
            .into_owned()
    }
}

// ---------------------------------------------------------------------------
// StandardRedactor
// ---------------------------------------------------------------------------

/// Strips secrets, PII, API keys, and other sensitive data from episode entries.
///
/// Built-in rules cover:
/// - API keys (Bearer tokens, common prefixes like `sk-`, `pk-`, etc.)
/// - Hex private keys (64-char hex strings prefixed with `0x`)
/// - Email addresses
/// - SS58 addresses longer than 10 characters
pub struct StandardRedactor {
    rules: Vec<RedactionRule>,
}

impl StandardRedactor {
    /// Create a redactor with the built-in rules.
    pub fn new() -> Self {
        Self {
            rules: builtin_rules(),
        }
    }

    /// Create a redactor with the built-in rules plus additional custom rules.
    pub fn with_extra_rules(extra: Vec<RedactionRule>) -> Self {
        let mut rules = builtin_rules();
        rules.extend(extra);
        Self { rules }
    }

    /// Create a redactor with only the provided rules (no built-in rules).
    pub fn custom(rules: Vec<RedactionRule>) -> Self {
        Self { rules }
    }

    /// Return the current set of rules.
    pub fn rules(&self) -> &[RedactionRule] {
        &self.rules
    }

    /// Redact sensitive data from the given text.
    pub fn redact(&self, text: &str) -> String {
        let mut result = text.to_string();
        for rule in &self.rules {
            result = rule.apply(&result);
        }
        result
    }

    /// Redact sensitive data from an episode entry in place.
    pub fn redact_entry(&self, entry: &mut EpisodeEntry) {
        entry.content = self.redact(&entry.content);

        for tc in &mut entry.tool_calls {
            if let serde_json::Value::String(ref s) = tc.input {
                tc.input = serde_json::Value::String(self.redact(s));
            } else {
                let s = tc.input.to_string();
                let redacted = self.redact(&s);
                if redacted != s {
                    if let Ok(v) = serde_json::from_str(&redacted) {
                        tc.input = v;
                    } else {
                        tc.input = serde_json::Value::String(redacted);
                    }
                }
            }

            if let Some(serde_json::Value::String(ref s)) = tc.output {
                tc.output = Some(serde_json::Value::String(self.redact(s)));
            } else if let Some(ref v) = tc.output {
                let s = v.to_string();
                let redacted = self.redact(&s);
                if redacted != s {
                    if let Ok(v) = serde_json::from_str(&redacted) {
                        tc.output = Some(v);
                    } else {
                        tc.output = Some(serde_json::Value::String(redacted));
                    }
                }
            }
        }
    }
}

impl Default for StandardRedactor {
    fn default() -> Self {
        Self::new()
    }
}

/// Built-in redaction rules.
fn builtin_rules() -> Vec<RedactionRule> {
    vec![
        // API keys: Bearer tokens
        RedactionRule::new(
            r"(?i)(Bearer\s+)[A-Za-z0-9\-_\.]+",
            "${1}[REDACTED_TOKEN]",
            "bearer_token",
        ),
        // API keys: common prefixes (sk-, pk-, api-, key-)
        RedactionRule::new(
            r"\b(sk-|pk-|api-|key-)[A-Za-z0-9]{20,}",
            "[REDACTED_API_KEY]",
            "api_key",
        ),
        // Hex private keys (0x followed by 64 hex chars)
        RedactionRule::new(
            r"0x[0-9a-fA-F]{64}\b",
            "[REDACTED_HEX_KEY]",
            "hex_private_key",
        ),
        // Email addresses
        RedactionRule::new(
            r"[a-zA-Z0-9._%+\-]+@[a-zA-Z0-9.\-]+\.[a-zA-Z]{2,}",
            "[REDACTED_EMAIL]",
            "email",
        ),
        // SS58 addresses (base58 chars, > 10 characters)
        RedactionRule::new(
            r"\b[1-9A-HJ-NP-Za-km-z]{11,50}\b",
            "[REDACTED_SS58]",
            "ss58_address",
        ),
    ]
}

// ---------------------------------------------------------------------------
// EpisodeLoggerConfig
// ---------------------------------------------------------------------------

/// Default maximum size of a single JSONL file before rotation (10 MB).
pub const DEFAULT_MAX_FILE_SIZE: u64 = 10 * 1024 * 1024;

/// Configuration for [`EpisodeLogger`].
#[derive(Debug, Clone)]
pub struct EpisodeLoggerConfig {
    /// Directory where JSONL log files are stored.
    pub log_dir: PathBuf,
    /// Maximum file size in bytes before rotation. Default: 10 MB.
    pub max_file_size: u64,
    /// Whether to zstd-compress rotated files. Default: true.
    pub compress_rotated: bool,
}

impl EpisodeLoggerConfig {
    /// Create a config with default settings for the given log directory.
    pub fn new(log_dir: impl Into<PathBuf>) -> Self {
        Self {
            log_dir: log_dir.into(),
            max_file_size: DEFAULT_MAX_FILE_SIZE,
            compress_rotated: true,
        }
    }

    /// Set the maximum file size before rotation.
    #[must_use]
    pub fn with_max_file_size(mut self, size: u64) -> Self {
        self.max_file_size = size;
        self
    }

    /// Set whether to zstd-compress rotated files.
    #[must_use]
    pub fn with_compression(mut self, enabled: bool) -> Self {
        self.compress_rotated = enabled;
        self
    }
}

// ---------------------------------------------------------------------------
// EpisodeLogger
// ---------------------------------------------------------------------------

/// Internal mutable state for the logger.
struct LoggerState {
    /// Currently active JSONL file path.
    active_path: PathBuf,
    /// Buffered writer for the active file.
    writer: BufWriter<File>,
    /// Current size of the active file in bytes.
    current_size: u64,
}

/// Append-only JSONL writer for conversation episodes.
///
/// Each episode gets its own directory under the configured log directory.
/// Entries are written as one JSON object per line with automatic rotation
/// and optional compression.
pub struct EpisodeLogger {
    config: EpisodeLoggerConfig,
    redactor: StandardRedactor,
    episode_id: EpisodeId,
    state: Arc<Mutex<LoggerState>>,
}

impl EpisodeLogger {
    /// Create a new episode logger for the given episode.
    ///
    /// Creates the episode directory if it does not exist.
    pub fn new(
        config: EpisodeLoggerConfig,
        redactor: StandardRedactor,
        episode_id: EpisodeId,
    ) -> MemoryResult<Self> {
        let episode_dir = config.log_dir.join(episode_id.to_string());
        fs::create_dir_all(&episode_dir)?;

        let active_path = episode_dir.join("episode.jsonl");
        let current_size = if active_path.exists() {
            fs::metadata(&active_path)?.len()
        } else {
            0
        };

        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&active_path)?;
        let writer = BufWriter::new(file);

        debug!(
            episode_id = %episode_id,
            path = %active_path.display(),
            "episode logger created"
        );

        Ok(Self {
            config,
            redactor,
            episode_id,
            state: Arc::new(Mutex::new(LoggerState {
                active_path,
                writer,
                current_size,
            })),
        })
    }

    /// Return the episode ID this logger is writing to.
    pub fn episode_id(&self) -> EpisodeId {
        self.episode_id
    }

    /// Append an entry to the episode log.
    ///
    /// The entry is redacted before writing. If the current file exceeds the
    /// configured max size, it is rotated (and optionally compressed).
    pub fn append(&self, mut entry: EpisodeEntry) -> MemoryResult<()> {
        self.redactor.redact_entry(&mut entry);

        let line = serde_json::to_string(&entry).map_err(MemoryError::Json)?;
        let line_bytes = line.as_bytes();
        let line_len = line_bytes.len() as u64 + 1; // +1 for newline

        let mut state = self.state.lock();

        // Check if rotation is needed.
        if state.current_size + line_len > self.config.max_file_size && state.current_size > 0 {
            self.rotate_locked(&mut state)?;
        }

        state.writer.write_all(line_bytes)?;
        state.writer.write_all(b"\n")?;
        state.writer.flush()?;
        state.current_size += line_len;

        Ok(())
    }

    /// Return the number of entries in the current active file.
    pub fn entry_count(&self) -> MemoryResult<usize> {
        let state = self.state.lock();
        let file = File::open(&state.active_path)?;
        let reader = BufReader::new(file);
        Ok(reader.lines().count())
    }

    /// Return the path to the active JSONL file.
    pub fn active_path(&self) -> PathBuf {
        self.state.lock().active_path.clone()
    }

    /// Return the episode directory.
    pub fn episode_dir(&self) -> PathBuf {
        self.config.log_dir.join(self.episode_id.to_string())
    }

    /// Rotate the current log file.
    fn rotate_locked(&self, state: &mut LoggerState) -> MemoryResult<()> {
        // Flush the current writer.
        state.writer.flush()?;

        // Generate a rotation filename with a timestamp.
        let ts = Utc::now().format("%Y%m%dT%H%M%S%.3f");
        let episode_dir = self.config.log_dir.join(self.episode_id.to_string());
        let rotated_name = format!("episode-{ts}.jsonl");
        let rotated_path = episode_dir.join(&rotated_name);

        // Rename the active file.
        fs::rename(&state.active_path, &rotated_path)?;

        debug!(
            episode_id = %self.episode_id,
            rotated = %rotated_path.display(),
            "rotated episode log"
        );

        // Compress if configured.
        if self.config.compress_rotated {
            Self::compress_file(&rotated_path)?;
        }

        // Open a new active file.
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&state.active_path)?;
        state.writer = BufWriter::new(file);
        state.current_size = 0;

        Ok(())
    }

    /// Compress a file using zstd and remove the original.
    fn compress_file(path: &Path) -> MemoryResult<()> {
        let compressed_path = path.with_extension("jsonl.zst");
        let input = File::open(path)?;
        let output = File::create(&compressed_path)?;

        let mut encoder = zstd::Encoder::new(output, 3)?;
        io::copy(&mut BufReader::new(input), &mut encoder)?;
        encoder.finish()?;

        fs::remove_file(path)?;

        debug!(
            path = %compressed_path.display(),
            "compressed rotated log"
        );

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Replay
// ---------------------------------------------------------------------------

/// Replay entries from a JSONL episode log file.
///
/// Returns a [`futures::Stream`] of [`EpisodeEntry`] items.
///
/// Supports both plain `.jsonl` files and zstd-compressed `.jsonl.zst` files.
pub fn replay(
    path: &Path,
) -> Pin<Box<dyn futures::Stream<Item = MemoryResult<EpisodeEntry>> + Send>> {
    let path = path.to_path_buf();

    Box::pin(async_stream::stream! {
        let is_compressed = path.extension().is_some_and(|ext| ext == "zst");

        let reader: Box<dyn BufRead + Send> = if is_compressed {
            let file = match File::open(&path) {
                Ok(f) => f,
                Err(e) => {
                    yield Err(MemoryError::Io(e));
                    return;
                }
            };
            let decoder = match zstd::Decoder::new(file) {
                Ok(d) => d,
                Err(e) => {
                    yield Err(MemoryError::Io(e));
                    return;
                }
            };
            Box::new(BufReader::new(decoder))
        } else {
            let file = match File::open(&path) {
                Ok(f) => f,
                Err(e) => {
                    yield Err(MemoryError::Io(e));
                    return;
                }
            };
            Box::new(BufReader::new(file))
        };

        for line in reader.lines() {
            match line {
                Ok(line) if line.trim().is_empty() => {}
                Ok(line) => {
                    match serde_json::from_str::<EpisodeEntry>(&line) {
                        Ok(entry) => yield Ok(entry),
                        Err(e) => yield Err(MemoryError::Json(e)),
                    }
                }
                Err(e) => {
                    yield Err(MemoryError::Io(e));
                    return;
                }
            }
        }
    })
}

/// Replay entries from the active log of a specific episode directory.
///
/// This is a convenience wrapper that constructs the expected path from an
/// episode log directory and episode ID.
pub fn replay_episode(
    log_dir: &Path,
    episode_id: EpisodeId,
) -> Pin<Box<dyn futures::Stream<Item = MemoryResult<EpisodeEntry>> + Send>> {
    let path = log_dir.join(episode_id.to_string()).join("episode.jsonl");
    replay(&path)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;

    fn tmp_config(dir: &Path) -> EpisodeLoggerConfig {
        EpisodeLoggerConfig::new(dir)
            .with_max_file_size(DEFAULT_MAX_FILE_SIZE)
            .with_compression(true)
    }

    fn make_entry(role: Role, content: &str) -> EpisodeEntry {
        EpisodeEntry {
            timestamp: Utc::now(),
            role,
            content: content.to_string(),
            tool_calls: vec![],
            metadata: serde_json::Value::Null,
        }
    }

    // -----------------------------------------------------------------------
    // Role tests
    // -----------------------------------------------------------------------

    #[test]
    fn role_display() {
        assert_eq!(Role::System.to_string(), "system");
        assert_eq!(Role::User.to_string(), "user");
        assert_eq!(Role::Assistant.to_string(), "assistant");
        assert_eq!(Role::Tool.to_string(), "tool");
    }

    #[test]
    fn role_from_str() {
        assert_eq!("system".parse::<Role>().unwrap(), Role::System);
        assert_eq!("user".parse::<Role>().unwrap(), Role::User);
        assert_eq!("assistant".parse::<Role>().unwrap(), Role::Assistant);
        assert_eq!("tool".parse::<Role>().unwrap(), Role::Tool);
        assert!("unknown".parse::<Role>().is_err());
    }

    #[test]
    fn role_serde_round_trip() {
        for role in [Role::System, Role::User, Role::Assistant, Role::Tool] {
            let json = serde_json::to_string(&role).unwrap();
            let back: Role = serde_json::from_str(&json).unwrap();
            assert_eq!(role, back);
        }
    }

    // -----------------------------------------------------------------------
    // EpisodeEntry serde
    // -----------------------------------------------------------------------

    #[test]
    fn episode_entry_serde_round_trip() {
        let entry = EpisodeEntry {
            timestamp: Utc::now(),
            role: Role::Assistant,
            content: "Hello, world!".to_string(),
            tool_calls: vec![ToolCallRecord {
                name: "search".to_string(),
                input: serde_json::json!({"query": "test"}),
                output: Some(serde_json::json!({"results": []})),
            }],
            metadata: serde_json::json!({"turn": 1}),
        };

        let json = serde_json::to_string(&entry).unwrap();
        let back: EpisodeEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(back.role, Role::Assistant);
        assert_eq!(back.content, "Hello, world!");
        assert_eq!(back.tool_calls.len(), 1);
        assert_eq!(back.tool_calls[0].name, "search");
    }

    #[test]
    fn episode_entry_empty_tool_calls_omitted() {
        let entry = make_entry(Role::User, "test");
        let json = serde_json::to_string(&entry).unwrap();
        assert!(!json.contains("tool_calls"));
    }

    #[test]
    fn episode_entry_null_metadata_omitted() {
        let entry = make_entry(Role::User, "test");
        let json = serde_json::to_string(&entry).unwrap();
        assert!(!json.contains("metadata"));
    }

    // -----------------------------------------------------------------------
    // RedactionRule tests
    // -----------------------------------------------------------------------

    #[test]
    fn redaction_rule_applies_pattern() {
        let rule = RedactionRule::new(r"\bfoo\b", "[GONE]", "test");
        assert_eq!(rule.apply("hello foo bar"), "hello [GONE] bar");
    }

    #[test]
    fn redaction_rule_no_match_returns_original() {
        let rule = RedactionRule::new(r"\bfoo\b", "[GONE]", "test");
        assert_eq!(rule.apply("hello bar baz"), "hello bar baz");
    }

    #[test]
    fn redaction_rule_replaces_all_occurrences() {
        let rule = RedactionRule::new(r"\bfoo\b", "[X]", "test");
        assert_eq!(rule.apply("foo and foo"), "[X] and [X]");
    }

    // -----------------------------------------------------------------------
    // StandardRedactor tests
    // -----------------------------------------------------------------------

    #[test]
    fn redactor_strips_bearer_token() {
        let r = StandardRedactor::new();
        let input = "Auth: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.abc";
        let redacted = r.redact(input);
        assert!(redacted.contains("[REDACTED_TOKEN]"));
        assert!(!redacted.contains("eyJhbG"));
    }

    #[test]
    fn redactor_strips_api_key() {
        let r = StandardRedactor::new();
        let input = "My key is sk-1234567890abcdefghijklmnop";
        let redacted = r.redact(input);
        assert!(redacted.contains("[REDACTED_API_KEY]"));
        assert!(!redacted.contains("1234567890"));
    }

    #[test]
    fn redactor_strips_hex_private_key() {
        let r = StandardRedactor::new();
        let hex = "0x".to_string() + &"ab".repeat(32);
        let input = format!("Key: {hex}");
        let redacted = r.redact(&input);
        assert!(redacted.contains("[REDACTED_HEX_KEY]"));
        assert!(!redacted.contains(&hex));
    }

    #[test]
    fn redactor_strips_email() {
        let r = StandardRedactor::new();
        let input = "Contact user@example.com for info";
        let redacted = r.redact(input);
        assert!(redacted.contains("[REDACTED_EMAIL]"));
        assert!(!redacted.contains("user@example.com"));
    }

    #[test]
    fn redactor_strips_ss58_address() {
        let r = StandardRedactor::new();
        // A typical SS58 address (> 10 chars of base58)
        let input = "Send to 5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY";
        let redacted = r.redact(input);
        assert!(redacted.contains("[REDACTED_SS58]"));
        assert!(!redacted.contains("5GrwvaEF5"));
    }

    #[test]
    fn redactor_preserves_short_words() {
        let r = StandardRedactor::new();
        // Short base58-like strings (<= 10 chars) should NOT be redacted.
        let input = "Hello world foo bar";
        let redacted = r.redact(input);
        assert_eq!(redacted, input);
    }

    #[test]
    fn redactor_custom_rules_only() {
        let rules = vec![RedactionRule::new(r"\bsecret\b", "[NOPE]", "custom")];
        let r = StandardRedactor::custom(rules);
        let input = "This is a secret message";
        let redacted = r.redact(input);
        assert_eq!(redacted, "This is a [NOPE] message");
    }

    #[test]
    fn redactor_extra_rules_added_to_builtin() {
        let extra = vec![RedactionRule::new(r"\bfoobar\b", "[X]", "custom")];
        let r = StandardRedactor::with_extra_rules(extra);
        // Extra rule applies.
        assert!(r.redact("foobar test").contains("[X]"));
        // Built-in rules still apply.
        assert!(r
            .redact("sk-abcdefghijklmnopqrstuv")
            .contains("[REDACTED_API_KEY]"));
    }

    #[test]
    fn redactor_redacts_entry_content() {
        let r = StandardRedactor::new();
        let mut entry = make_entry(Role::User, "My key is sk-abcdefghijklmnopqrstuv");
        r.redact_entry(&mut entry);
        assert!(entry.content.contains("[REDACTED_API_KEY]"));
    }

    #[test]
    fn redactor_redacts_tool_call_input() {
        let r = StandardRedactor::new();
        let mut entry = EpisodeEntry {
            timestamp: Utc::now(),
            role: Role::Tool,
            content: "ok".to_string(),
            tool_calls: vec![ToolCallRecord {
                name: "send".to_string(),
                input: serde_json::json!({"email": "user@example.com"}),
                output: None,
            }],
            metadata: serde_json::Value::Null,
        };
        r.redact_entry(&mut entry);
        let input_str = entry.tool_calls[0].input.to_string();
        assert!(input_str.contains("[REDACTED_EMAIL]"));
    }

    #[test]
    fn redactor_redacts_tool_call_output() {
        let r = StandardRedactor::new();
        let mut entry = EpisodeEntry {
            timestamp: Utc::now(),
            role: Role::Tool,
            content: "ok".to_string(),
            tool_calls: vec![ToolCallRecord {
                name: "lookup".to_string(),
                input: serde_json::json!({}),
                output: Some(serde_json::Value::String("Contact user@test.com".into())),
            }],
            metadata: serde_json::Value::Null,
        };
        r.redact_entry(&mut entry);
        let output_str = entry.tool_calls[0]
            .output
            .as_ref()
            .unwrap()
            .as_str()
            .unwrap();
        assert!(output_str.contains("[REDACTED_EMAIL]"));
    }

    // -----------------------------------------------------------------------
    // EpisodeLogger tests
    // -----------------------------------------------------------------------

    #[test]
    fn logger_creates_episode_directory() {
        let dir = tempfile::tempdir().unwrap();
        let ep = EpisodeId::new();
        let config = tmp_config(dir.path());
        let _logger = EpisodeLogger::new(config, StandardRedactor::new(), ep).unwrap();
        assert!(dir.path().join(ep.to_string()).exists());
    }

    #[test]
    fn logger_appends_entries() {
        let dir = tempfile::tempdir().unwrap();
        let ep = EpisodeId::new();
        let config = tmp_config(dir.path());
        let logger = EpisodeLogger::new(config, StandardRedactor::new(), ep).unwrap();

        logger.append(make_entry(Role::User, "Hello")).unwrap();
        logger
            .append(make_entry(Role::Assistant, "Hi there"))
            .unwrap();

        assert_eq!(logger.entry_count().unwrap(), 2);
    }

    #[test]
    fn logger_writes_valid_jsonl() {
        let dir = tempfile::tempdir().unwrap();
        let ep = EpisodeId::new();
        let config = tmp_config(dir.path());
        let logger = EpisodeLogger::new(config, StandardRedactor::new(), ep).unwrap();

        logger.append(make_entry(Role::User, "Question")).unwrap();
        logger
            .append(make_entry(Role::Assistant, "Answer"))
            .unwrap();

        let content = fs::read_to_string(logger.active_path()).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 2);

        let first: EpisodeEntry = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(first.role, Role::User);
        assert_eq!(first.content, "Question");

        let second: EpisodeEntry = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(second.role, Role::Assistant);
        assert_eq!(second.content, "Answer");
    }

    #[test]
    fn logger_redacts_before_writing() {
        let dir = tempfile::tempdir().unwrap();
        let ep = EpisodeId::new();
        let config = tmp_config(dir.path());
        let logger = EpisodeLogger::new(config, StandardRedactor::new(), ep).unwrap();

        logger
            .append(make_entry(
                Role::User,
                "My key is sk-abcdefghijklmnopqrstuv",
            ))
            .unwrap();

        let content = fs::read_to_string(logger.active_path()).unwrap();
        assert!(content.contains("[REDACTED_API_KEY]"));
        assert!(!content.contains("abcdefghijklmnop"));
    }

    #[test]
    fn logger_rotates_at_max_size() {
        let dir = tempfile::tempdir().unwrap();
        let ep = EpisodeId::new();
        // Tiny max size to trigger rotation quickly.
        let config = EpisodeLoggerConfig::new(dir.path())
            .with_max_file_size(100)
            .with_compression(false);
        let logger = EpisodeLogger::new(config, StandardRedactor::new(), ep).unwrap();

        // Write enough entries to trigger rotation.
        for i in 0..10 {
            logger
                .append(make_entry(
                    Role::User,
                    &format!("Message number {i} with some padding"),
                ))
                .unwrap();
        }

        // There should be rotated files in the episode directory.
        let episode_dir = dir.path().join(ep.to_string());
        let files: Vec<_> = fs::read_dir(&episode_dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().starts_with("episode-"))
            .collect();
        assert!(
            !files.is_empty(),
            "should have rotated files after exceeding max size"
        );
    }

    #[test]
    fn logger_compresses_rotated_files() {
        let dir = tempfile::tempdir().unwrap();
        let ep = EpisodeId::new();
        let config = EpisodeLoggerConfig::new(dir.path())
            .with_max_file_size(100)
            .with_compression(true);
        let logger = EpisodeLogger::new(config, StandardRedactor::new(), ep).unwrap();

        for i in 0..10 {
            logger
                .append(make_entry(
                    Role::User,
                    &format!("Message number {i} with some padding"),
                ))
                .unwrap();
        }

        let episode_dir = dir.path().join(ep.to_string());
        let zst_files: Vec<_> = fs::read_dir(&episode_dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().ends_with(".zst"))
            .collect();
        assert!(
            !zst_files.is_empty(),
            "rotated files should be zstd-compressed"
        );
    }

    #[test]
    fn logger_episode_id_accessor() {
        let dir = tempfile::tempdir().unwrap();
        let ep = EpisodeId::new();
        let config = tmp_config(dir.path());
        let logger = EpisodeLogger::new(config, StandardRedactor::new(), ep).unwrap();
        assert_eq!(logger.episode_id(), ep);
    }

    // -----------------------------------------------------------------------
    // Replay tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn replay_reads_entries_from_file() {
        let dir = tempfile::tempdir().unwrap();
        let ep = EpisodeId::new();
        let config = tmp_config(dir.path());
        let logger = EpisodeLogger::new(config, StandardRedactor::new(), ep).unwrap();

        logger.append(make_entry(Role::User, "Hello")).unwrap();
        logger.append(make_entry(Role::Assistant, "Hi")).unwrap();
        logger.append(make_entry(Role::User, "Bye")).unwrap();

        let path = logger.active_path();
        let mut stream = replay(&path);
        let mut entries = vec![];
        while let Some(item) = stream.next().await {
            entries.push(item.unwrap());
        }

        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].role, Role::User);
        assert_eq!(entries[0].content, "Hello");
        assert_eq!(entries[1].role, Role::Assistant);
        assert_eq!(entries[2].content, "Bye");
    }

    #[tokio::test]
    async fn replay_compressed_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.jsonl");
        let compressed_path = dir.path().join("test.jsonl.zst");

        // Write a plain JSONL file.
        {
            let mut f = File::create(&path).unwrap();
            let entry = make_entry(Role::User, "compressed entry");
            let line = serde_json::to_string(&entry).unwrap();
            writeln!(f, "{line}").unwrap();
        }

        // Compress it.
        {
            let input = File::open(&path).unwrap();
            let output = File::create(&compressed_path).unwrap();
            let mut encoder = zstd::Encoder::new(output, 3).unwrap();
            io::copy(&mut BufReader::new(input), &mut encoder).unwrap();
            encoder.finish().unwrap();
        }

        let mut stream = replay(&compressed_path);
        let entry = stream.next().await.unwrap().unwrap();
        assert_eq!(entry.content, "compressed entry");
        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn replay_empty_file_returns_empty_stream() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.jsonl");
        File::create(&path).unwrap();

        let mut stream = replay(&path);
        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn replay_missing_file_returns_error() {
        let path = PathBuf::from("/nonexistent/path/episode.jsonl");
        let mut stream = replay(&path);
        let result = stream.next().await;
        assert!(result.is_some());
        assert!(result.unwrap().is_err());
    }

    #[tokio::test]
    async fn replay_episode_convenience() {
        let dir = tempfile::tempdir().unwrap();
        let ep = EpisodeId::new();
        let config = tmp_config(dir.path());
        let logger = EpisodeLogger::new(config, StandardRedactor::new(), ep).unwrap();

        logger
            .append(make_entry(Role::System, "System prompt"))
            .unwrap();
        logger.append(make_entry(Role::User, "Question")).unwrap();

        let mut stream = replay_episode(dir.path(), ep);
        let mut entries = vec![];
        while let Some(item) = stream.next().await {
            entries.push(item.unwrap());
        }

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].role, Role::System);
        assert_eq!(entries[1].role, Role::User);
    }

    // -----------------------------------------------------------------------
    // Config tests
    // -----------------------------------------------------------------------

    #[test]
    fn config_defaults() {
        let config = EpisodeLoggerConfig::new("/tmp/logs");
        assert_eq!(config.max_file_size, DEFAULT_MAX_FILE_SIZE);
        assert!(config.compress_rotated);
    }

    #[test]
    fn config_builder_methods() {
        let config = EpisodeLoggerConfig::new("/tmp/logs")
            .with_max_file_size(1024)
            .with_compression(false);
        assert_eq!(config.max_file_size, 1024);
        assert!(!config.compress_rotated);
    }

    // -----------------------------------------------------------------------
    // Tool call record tests
    // -----------------------------------------------------------------------

    #[test]
    fn tool_call_record_serde() {
        let tc = ToolCallRecord {
            name: "search".to_string(),
            input: serde_json::json!({"q": "test"}),
            output: Some(serde_json::json!(["result1", "result2"])),
        };
        let json = serde_json::to_string(&tc).unwrap();
        let back: ToolCallRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(back.name, "search");
    }

    #[test]
    fn tool_call_record_without_output() {
        let tc = ToolCallRecord {
            name: "fire_and_forget".to_string(),
            input: serde_json::json!(null),
            output: None,
        };
        let json = serde_json::to_string(&tc).unwrap();
        let back: ToolCallRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(back.name, "fire_and_forget");
        assert!(back.output.is_none());
    }
}
