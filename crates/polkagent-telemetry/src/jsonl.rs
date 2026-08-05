//! JSONL event writer for durable run event recording.
//!
//! [`JsonlWriter`] appends serialized [`RunEvent`]
//! records to a `.jsonl` file, one JSON object per line. Each line is
//! flushed immediately to ensure durability.

use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use chrono::Utc;
use polkagent_core::event::RunEvent;
use serde::Serialize;

/// Error type for JSONL write operations.
#[derive(Debug, thiserror::Error)]
pub enum JsonlError {
    /// I/O error writing to the file.
    #[error("JSONL I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// Serialization error.
    #[error("JSONL serialization error: {0}")]
    Serialize(#[from] serde_json::Error),
}

/// A single line in the JSONL file.
///
/// This envelope wraps the run event with a top-level timestamp and
/// event type for easy log processing.
#[derive(Debug, Serialize)]
struct JsonlLine<'a> {
    timestamp: String,
    event_type: &'a str,
    run_id: String,
    data: &'a RunEvent,
}

/// Writer that appends [`RunEvent`] records to a JSONL file.
///
/// Each call to [`write_event`](JsonlWriter::write_event) serializes the
/// event as a single JSON line and flushes immediately.
pub struct JsonlWriter {
    writer: BufWriter<File>,
    path: PathBuf,
}

impl JsonlWriter {
    /// Open (or create) a JSONL file at `path` for appending.
    ///
    /// # Errors
    ///
    /// Returns [`JsonlError::Io`] if the file cannot be opened.
    pub fn new(path: impl AsRef<Path>) -> Result<Self, JsonlError> {
        let path = path.as_ref().to_path_buf();
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        Ok(Self {
            writer: BufWriter::new(file),
            path,
        })
    }

    /// Serialize `event` as JSON and write a single line, then flush.
    ///
    /// # Line format
    ///
    /// ```json
    /// {"timestamp":"...","event_type":"...","run_id":"...","data":{...}}
    /// ```
    ///
    /// # Errors
    ///
    /// Returns [`JsonlError`] on serialization or I/O failure.
    pub fn write_event(&mut self, event: &RunEvent) -> Result<(), JsonlError> {
        let event_type = event_kind_label(&event.kind);
        let line = JsonlLine {
            timestamp: Utc::now().to_rfc3339(),
            event_type,
            run_id: event.run_id.to_string(),
            data: event,
        };
        serde_json::to_writer(&mut self.writer, &line)?;
        self.writer.write_all(b"\n")?;
        self.writer.flush()?;
        Ok(())
    }

    /// Return the path this writer is writing to.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl std::fmt::Debug for JsonlWriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JsonlWriter")
            .field("path", &self.path)
            .finish()
    }
}

/// Extract a human-readable label from an [`EventKind`] variant.
fn event_kind_label(kind: &polkagent_core::event::EventKind) -> &'static str {
    use polkagent_core::event::EventKind;
    match kind {
        EventKind::RunCreated => "run_created",
        EventKind::RunQueued => "run_queued",
        EventKind::RunStarted => "run_started",
        EventKind::ApprovalRequested { .. } => "approval_requested",
        EventKind::ApprovalGranted { .. } => "approval_granted",
        EventKind::ApprovalDenied { .. } => "approval_denied",
        EventKind::RunCompleting => "run_completing",
        EventKind::RunCompleted { .. } => "run_completed",
        EventKind::RunFailed { .. } => "run_failed",
        EventKind::RunCancelled { .. } => "run_cancelled",
        EventKind::RunTimedOut => "run_timed_out",
        EventKind::RunRetryQueued => "run_retry_queued",
        EventKind::TurnStarted { .. } => "turn_started",
        EventKind::TurnCompleted { .. } => "turn_completed",
        EventKind::StepStarted { .. } => "step_started",
        EventKind::StepCompleted { .. } => "step_completed",
        EventKind::EffectIntentCreated { .. } => "effect_intent_created",
        EventKind::EffectAttemptStarted { .. } => "effect_attempt_started",
        EventKind::EffectOutcomeRecorded { .. } => "effect_outcome_recorded",
        EventKind::EffectsResolved => "effects_resolved",
        EventKind::ArtifactCreated { .. } => "artifact_created",
        EventKind::StreamingToken { .. } => "streaming_token",
        EventKind::ProgressUpdate { .. } => "progress_update",
        EventKind::ToolCallStarted { .. } => "tool_call_started",
        EventKind::ToolCallCompleted { .. } => "tool_call_completed",
        EventKind::DeliveryStarted => "delivery_started",
        EventKind::DeliveryCompleted => "delivery_completed",
        EventKind::DiagnosticLog { .. } => "diagnostic_log",
        EventKind::BudgetConsumed { .. } => "budget_consumed",
        EventKind::BudgetWarning { .. } => "budget_warning",
        EventKind::MetadataDriftDetected { .. } => "metadata_drift_detected",
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use polkagent_core::event::{EventCorrelation, EventKind};
    use polkagent_core::ids::{EventId, RunId};

    fn make_test_event(run_id: RunId) -> RunEvent {
        RunEvent::new_durable(
            EventId::new(),
            run_id.clone(),
            1,
            EventKind::RunCreated,
            EventCorrelation {
                run_id,
                ..Default::default()
            },
        )
    }

    #[test]
    fn write_and_read_single_event() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let path = dir.path().join("test.jsonl");

        let run_id = RunId::new();
        let event = make_test_event(run_id);

        {
            let mut writer = JsonlWriter::new(&path).expect("open writer");
            writer.write_event(&event).expect("write event");
        }

        let contents = std::fs::read_to_string(&path).expect("read file");
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(lines.len(), 1);

        // Parse the JSON line to verify structure.
        let parsed: serde_json::Value = serde_json::from_str(lines[0]).expect("valid JSON");
        assert!(parsed.get("timestamp").is_some());
        assert_eq!(parsed["event_type"], "run_created");
        assert!(parsed.get("run_id").is_some());
        assert!(parsed.get("data").is_some());
    }

    #[test]
    fn write_multiple_events_appends() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let path = dir.path().join("multi.jsonl");

        let run_id = RunId::new();

        {
            let mut writer = JsonlWriter::new(&path).expect("open writer");
            for seq in 1..=5 {
                let event = RunEvent::new_durable(
                    EventId::new(),
                    run_id.clone(),
                    seq,
                    EventKind::RunCreated,
                    EventCorrelation {
                        run_id: run_id.clone(),
                        ..Default::default()
                    },
                );
                writer.write_event(&event).expect("write event");
            }
        }

        let contents = std::fs::read_to_string(&path).expect("read file");
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(lines.len(), 5);
    }

    #[test]
    fn append_mode_preserves_existing() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let path = dir.path().join("append.jsonl");

        let run_id = RunId::new();
        let event = make_test_event(run_id.clone());

        // Write first event.
        {
            let mut writer = JsonlWriter::new(&path).expect("open writer");
            writer.write_event(&event).expect("write");
        }

        // Open again and write another event.
        {
            let event2 = RunEvent::new_durable(
                EventId::new(),
                run_id.clone(),
                2,
                EventKind::RunQueued,
                EventCorrelation {
                    run_id,
                    ..Default::default()
                },
            );
            let mut writer = JsonlWriter::new(&path).expect("reopen writer");
            writer.write_event(&event2).expect("write");
        }

        let contents = std::fs::read_to_string(&path).expect("read file");
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn debug_impl_shows_path() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let path = dir.path().join("debug.jsonl");
        let writer = JsonlWriter::new(&path).expect("open writer");
        let debug = format!("{writer:?}");
        assert!(debug.contains("debug.jsonl"));
    }

    #[test]
    fn event_kind_label_coverage() {
        // Verify that all enum variants produce a non-empty label.
        let kinds = vec![
            EventKind::RunCreated,
            EventKind::RunQueued,
            EventKind::RunStarted,
            EventKind::RunCompleting,
            EventKind::RunTimedOut,
            EventKind::RunRetryQueued,
            EventKind::EffectsResolved,
            EventKind::DeliveryStarted,
            EventKind::DeliveryCompleted,
        ];
        for kind in &kinds {
            let label = event_kind_label(kind);
            assert!(!label.is_empty());
        }
    }
}
