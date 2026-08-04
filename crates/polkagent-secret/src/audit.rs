//! Secret access audit log.
//!
//! Records all secret operations (get, set, delete) to a JSONL file at
//! `~/.polkagent/audit/secrets.jsonl`.
//!
//! Each line is a JSON object with:
//! - `timestamp` — ISO-8601 timestamp
//! - `operation` — the operation performed
//! - `secret_id` — the secret ID (never the value)
//! - `source` — which store served the request
//! - `result` — `"ok"` or an error description
//!
//! **The secret value is never logged.**

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use tracing::{debug, warn};

use crate::store::SecretError;
use crate::types::SecretId;

// ---------------------------------------------------------------------------
// AuditOperation
// ---------------------------------------------------------------------------

/// The type of secret operation that was performed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditOperation {
    /// A secret was read.
    Get,
    /// A secret was created or updated.
    Set,
    /// A secret was deleted.
    Delete,
    /// A secret existence check was performed.
    Exists,
    /// All secrets were listed.
    List,
}

impl std::fmt::Display for AuditOperation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Get => write!(f, "get"),
            Self::Set => write!(f, "set"),
            Self::Delete => write!(f, "delete"),
            Self::Exists => write!(f, "exists"),
            Self::List => write!(f, "list"),
        }
    }
}

// ---------------------------------------------------------------------------
// AuditEntry
// ---------------------------------------------------------------------------

/// A single audit log entry. Serialized as one line of JSON in the JSONL file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    /// When the operation occurred.
    pub timestamp: DateTime<Utc>,
    /// What operation was performed.
    pub operation: AuditOperation,
    /// Which secret was involved (empty string for `list`).
    pub secret_id: String,
    /// Which store source served the request.
    pub source: String,
    /// Outcome: `"ok"` or an error description.
    pub result: String,
}

// ---------------------------------------------------------------------------
// SecretAuditLog
// ---------------------------------------------------------------------------

/// Append-only JSONL audit log for secret access events.
///
/// Default location: `~/.polkagent/audit/secrets.jsonl`.
pub struct SecretAuditLog {
    log_path: PathBuf,
}

impl SecretAuditLog {
    /// Create a new audit log at the default location
    /// (`~/.polkagent/audit/secrets.jsonl`).
    ///
    /// Creates the parent directory if it does not exist.
    pub fn new() -> Result<Self, SecretError> {
        let home = dirs::home_dir().ok_or_else(|| SecretError::Internal {
            message: "could not determine home directory".into(),
        })?;
        let log_path = home.join(".polkagent").join("audit").join("secrets.jsonl");
        Self::with_path(log_path)
    }

    /// Create an audit log at a custom path.
    ///
    /// Creates the parent directory if it does not exist.
    pub fn with_path(log_path: PathBuf) -> Result<Self, SecretError> {
        if let Some(parent) = log_path.parent() {
            fs::create_dir_all(parent)?;
        }
        debug!(path = %log_path.display(), "initialised secret audit log");
        Ok(Self { log_path })
    }

    /// Record a successful operation.
    pub fn record_ok(&self, operation: AuditOperation, secret_id: &SecretId, source: &str) {
        let entry = AuditEntry {
            timestamp: Utc::now(),
            operation,
            secret_id: secret_id.as_str().to_string(),
            source: source.to_string(),
            result: "ok".to_string(),
        };
        self.append(&entry);
    }

    /// Record a failed operation.
    pub fn record_error(
        &self,
        operation: AuditOperation,
        secret_id: &SecretId,
        source: &str,
        error: &SecretError,
    ) {
        let entry = AuditEntry {
            timestamp: Utc::now(),
            operation,
            secret_id: secret_id.as_str().to_string(),
            source: source.to_string(),
            result: format!("error: {error}"),
        };
        self.append(&entry);
    }

    /// Record a list operation (no specific secret ID).
    pub fn record_list(&self, source: &str, result: &str) {
        let entry = AuditEntry {
            timestamp: Utc::now(),
            operation: AuditOperation::List,
            secret_id: String::new(),
            source: source.to_string(),
            result: result.to_string(),
        };
        self.append(&entry);
    }

    /// Read all entries from the log file. Useful for testing.
    pub fn read_entries(&self) -> Result<Vec<AuditEntry>, SecretError> {
        if !self.log_path.exists() {
            return Ok(Vec::new());
        }
        let content = fs::read_to_string(&self.log_path)?;
        let mut entries = Vec::new();
        for line in content.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let entry: AuditEntry = serde_json::from_str(line)?;
            entries.push(entry);
        }
        Ok(entries)
    }

    /// Return the path to the log file.
    #[must_use]
    pub fn path(&self) -> &PathBuf {
        &self.log_path
    }

    /// Append a single entry to the JSONL file.
    fn append(&self, entry: &AuditEntry) {
        match self.try_append(entry) {
            Ok(()) => {}
            Err(e) => {
                warn!(
                    error = %e,
                    operation = %entry.operation,
                    secret_id = %entry.secret_id,
                    "failed to write audit log entry"
                );
            }
        }
    }

    /// Fallible append used internally.
    fn try_append(&self, entry: &AuditEntry) -> Result<(), SecretError> {
        let line = serde_json::to_string(entry)?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log_path)?;
        writeln!(file, "{line}")?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_audit() -> (tempfile::TempDir, SecretAuditLog) {
        let dir = tempfile::tempdir().expect("tempdir");
        let log_path = dir.path().join("audit").join("secrets.jsonl");
        let audit = SecretAuditLog::with_path(log_path).expect("create audit log");
        (dir, audit)
    }

    #[test]
    fn record_ok_writes_entry() {
        let (_dir, audit) = temp_audit();
        let id = SecretId::new("test-key");

        audit.record_ok(AuditOperation::Get, &id, "env");

        let entries = audit.read_entries().expect("read");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].operation, AuditOperation::Get);
        assert_eq!(entries[0].secret_id, "test-key");
        assert_eq!(entries[0].source, "env");
        assert_eq!(entries[0].result, "ok");
    }

    #[test]
    fn record_error_writes_entry() {
        let (_dir, audit) = temp_audit();
        let id = SecretId::new("missing");
        let err = SecretError::NotFound {
            id: "missing".into(),
        };

        audit.record_error(AuditOperation::Get, &id, "file", &err);

        let entries = audit.read_entries().expect("read");
        assert_eq!(entries.len(), 1);
        assert!(entries[0].result.contains("error"));
        assert!(entries[0].result.contains("not found"));
    }

    #[test]
    fn record_list_writes_entry() {
        let (_dir, audit) = temp_audit();
        audit.record_list("chain", "ok");

        let entries = audit.read_entries().expect("read");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].operation, AuditOperation::List);
        assert!(entries[0].secret_id.is_empty());
    }

    #[test]
    fn multiple_entries_are_appended() {
        let (_dir, audit) = temp_audit();
        let id = SecretId::new("multi");

        audit.record_ok(AuditOperation::Get, &id, "env");
        audit.record_ok(AuditOperation::Set, &id, "file");
        audit.record_ok(AuditOperation::Delete, &id, "file");

        let entries = audit.read_entries().expect("read");
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].operation, AuditOperation::Get);
        assert_eq!(entries[1].operation, AuditOperation::Set);
        assert_eq!(entries[2].operation, AuditOperation::Delete);
    }

    #[test]
    fn read_entries_from_empty_file() {
        let (_dir, audit) = temp_audit();
        let entries = audit.read_entries().expect("read");
        assert!(entries.is_empty());
    }

    #[test]
    fn entries_never_contain_secret_values() {
        let (_dir, audit) = temp_audit();
        let id = SecretId::new("sensitive");

        audit.record_ok(AuditOperation::Get, &id, "file");
        audit.record_ok(AuditOperation::Set, &id, "file");

        let content = std::fs::read_to_string(audit.path()).expect("read file");
        // We never wrote a secret value, but let's verify the structure
        // doesn't accidentally include anything unexpected.
        assert!(!content.contains("value"));
        assert!(content.contains("sensitive"));
        assert!(content.contains("secret_id"));
    }

    #[test]
    fn audit_operation_display() {
        assert_eq!(format!("{}", AuditOperation::Get), "get");
        assert_eq!(format!("{}", AuditOperation::Set), "set");
        assert_eq!(format!("{}", AuditOperation::Delete), "delete");
        assert_eq!(format!("{}", AuditOperation::Exists), "exists");
        assert_eq!(format!("{}", AuditOperation::List), "list");
    }

    #[test]
    fn audit_entry_serializes_correctly() {
        let entry = AuditEntry {
            timestamp: Utc::now(),
            operation: AuditOperation::Get,
            secret_id: "my-key".to_string(),
            source: "env".to_string(),
            result: "ok".to_string(),
        };

        let json = serde_json::to_string(&entry).expect("serialize");
        assert!(json.contains("\"operation\":\"get\""));
        assert!(json.contains("\"secret_id\":\"my-key\""));

        let back: AuditEntry = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.operation, AuditOperation::Get);
        assert_eq!(back.secret_id, "my-key");
    }
}
