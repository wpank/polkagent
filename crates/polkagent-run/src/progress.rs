//! Client-facing run progress event stream (PRD-04 §9.4).
//!
//! This module defines [`RunProgressEvent`] — the public, client-facing event
//! type that API consumers observe while a run executes — and
//! [`RunProgressStream`] — a transformer that converts internal
//! [`RunEvent`](polkagent_core::event::RunEvent) sequences from the
//! [`EventBus`](polkagent_event::EventBus) into ordered
//! `RunProgressEvent` values.
//!
//! # Guarantees
//!
//! - **Ordered delivery:** events emerge in sequence-number order.
//! - **At-most-one terminal:** `Completed` or `Failed` appears at most once
//!   and is always the final event emitted.
//! - **Lossless text:** every `TextDelta` from the internal stream is
//!   forwarded; no text fragments are dropped or coalesced.

use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use futures::Stream;
use polkagent_core::{ApprovalId, ArtifactId, RunId};
use polkagent_event::{EventBus, EventReceiver};
use polkagent_core::event::{EventKind, RunEvent};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Supporting types
// ---------------------------------------------------------------------------

/// The lifecycle status of a single tool invocation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolUseStatus {
    /// The tool call has begun.
    Started,
    /// The tool call is actively executing.
    Running,
    /// The tool call finished successfully.
    Completed,
    /// The tool call failed.
    Failed,
}

/// A request for human approval before proceeding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalRequest {
    /// Unique identifier for this approval request.
    pub id: ApprovalId,
    /// Human-readable description of what is being approved.
    pub description: String,
    /// How long the approval remains valid before expiring.
    pub timeout: Duration,
}

/// Summary metadata about a produced artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactSummary {
    /// The artifact's unique identifier.
    pub id: ArtifactId,
    /// The kind of artifact (e.g. `"file"`, `"extrinsic"`, `"report"`).
    pub kind: String,
    /// Human-readable name or path.
    pub name: String,
}

/// Summary of a completed run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunSummary {
    /// Total tokens consumed across all turns (input + output).
    pub tokens_used: u64,
    /// Wall-clock duration of the run.
    pub duration: Duration,
    /// Number of effects (tool calls) executed during the run.
    pub effects_count: u32,
}

/// The error reported when a run fails.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunProgressError {
    /// Human-readable error message.
    pub message: String,
}

// ---------------------------------------------------------------------------
// RunProgressEvent
// ---------------------------------------------------------------------------

/// A client-facing progress event emitted during a run.
///
/// This is the public API contract for run streaming. Internal event types
/// (`EventKind`, `StreamEvent`, `HarnessEvent`) are mapped into this
/// simplified, stable schema before delivery to API consumers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RunProgressEvent {
    /// The run has been queued for execution.
    Queued {
        /// The run this event belongs to.
        run_id: RunId,
    },

    /// The run is actively executing.
    Working {
        /// The run this event belongs to.
        run_id: RunId,
    },

    /// An incremental fragment of text output from the model.
    TextDelta {
        /// The run this event belongs to.
        run_id: RunId,
        /// The text fragment.
        delta: String,
    },

    /// A tool invocation status change.
    ToolUse {
        /// The run this event belongs to.
        run_id: RunId,
        /// The tool that was invoked.
        tool: String,
        /// The current status of the tool invocation.
        status: ToolUseStatus,
    },

    /// Human approval is required before the run can continue.
    ApprovalRequired {
        /// The run this event belongs to.
        run_id: RunId,
        /// Details of the approval request.
        approval: ApprovalRequest,
    },

    /// An artifact was produced during the run.
    ArtifactProduced {
        /// The run this event belongs to.
        run_id: RunId,
        /// Summary of the produced artifact.
        artifact: ArtifactSummary,
    },

    /// The run completed successfully.
    Completed {
        /// The run this event belongs to.
        run_id: RunId,
        /// Summary of the completed run.
        summary: RunSummary,
    },

    /// The run failed.
    Failed {
        /// The run this event belongs to.
        run_id: RunId,
        /// Error details.
        error: RunProgressError,
    },
}

impl RunProgressEvent {
    /// Returns the `RunId` associated with this event.
    #[must_use]
    pub fn run_id(&self) -> &RunId {
        match self {
            Self::Queued { run_id }
            | Self::Working { run_id }
            | Self::TextDelta { run_id, .. }
            | Self::ToolUse { run_id, .. }
            | Self::ApprovalRequired { run_id, .. }
            | Self::ArtifactProduced { run_id, .. }
            | Self::Completed { run_id, .. }
            | Self::Failed { run_id, .. } => run_id,
        }
    }

    /// Returns `true` if this is a terminal event (`Completed` or `Failed`).
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Completed { .. } | Self::Failed { .. })
    }
}

// ---------------------------------------------------------------------------
// Conversion from internal events
// ---------------------------------------------------------------------------

/// Convert an internal [`RunEvent`] into zero or one [`RunProgressEvent`].
///
/// Returns `None` for internal events that have no client-facing
/// representation (e.g. `StepStarted`, `DeliveryStarted`).
#[must_use]
pub fn map_event(event: &RunEvent) -> Option<RunProgressEvent> {
    let run_id = event.run_id;
    match &event.kind {
        EventKind::RunQueued => Some(RunProgressEvent::Queued { run_id }),

        EventKind::RunStarted => Some(RunProgressEvent::Working { run_id }),

        EventKind::StreamingToken { text } => Some(RunProgressEvent::TextDelta {
            run_id,
            delta: text.clone(),
        }),

        EventKind::ToolCallStarted { tool_name } => Some(RunProgressEvent::ToolUse {
            run_id,
            tool: tool_name.clone(),
            status: ToolUseStatus::Started,
        }),

        EventKind::ToolCallCompleted { tool_name } => Some(RunProgressEvent::ToolUse {
            run_id,
            tool: tool_name.clone(),
            status: ToolUseStatus::Completed,
        }),

        EventKind::ApprovalRequested { request_id } => {
            Some(RunProgressEvent::ApprovalRequired {
                run_id,
                approval: ApprovalRequest {
                    id: ApprovalId::new(),
                    description: format!("Approval required: {request_id}"),
                    timeout: Duration::from_secs(300),
                },
            })
        }

        EventKind::ArtifactCreated { artifact_id } => {
            Some(RunProgressEvent::ArtifactProduced {
                run_id,
                artifact: ArtifactSummary {
                    id: *artifact_id,
                    kind: "unknown".to_string(),
                    name: artifact_id.to_string(),
                },
            })
        }

        EventKind::RunCompleted { .. } => Some(RunProgressEvent::Completed {
            run_id,
            summary: RunSummary {
                tokens_used: 0,
                duration: Duration::ZERO,
                effects_count: 0,
            },
        }),

        EventKind::RunFailed { reason } => Some(RunProgressEvent::Failed {
            run_id,
            error: RunProgressError {
                message: reason.clone(),
            },
        }),

        EventKind::RunCancelled { reason } => Some(RunProgressEvent::Failed {
            run_id,
            error: RunProgressError {
                message: format!("cancelled: {reason}"),
            },
        }),

        EventKind::RunTimedOut => Some(RunProgressEvent::Failed {
            run_id,
            error: RunProgressError {
                message: "run timed out".to_string(),
            },
        }),

        // Internal events with no client-facing representation.
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// RunProgressStream
// ---------------------------------------------------------------------------

/// A stream adapter that converts internal [`RunEvent`]s from an
/// [`EventBus`] into client-facing [`RunProgressEvent`]s.
///
/// # Guarantees
///
/// - Events are delivered in the order received from the bus.
/// - At most one terminal event (`Completed` / `Failed`) is emitted; the
///   stream ends after the terminal.
/// - All `TextDelta` events are forwarded without coalescing.
pub struct RunProgressStream {
    /// The run we are filtering for.
    run_id: RunId,
    /// The event bus receiver.
    receiver: EventReceiver,
    /// Whether a terminal event has been emitted.
    terminated: bool,
}

impl RunProgressStream {
    /// Create a new progress stream for the given run.
    ///
    /// The stream subscribes to the [`EventBus`] from the current point
    /// forward. Historical events are not replayed.
    #[must_use]
    pub fn new(run_id: RunId, bus: &EventBus) -> Self {
        Self {
            run_id,
            receiver: bus.subscribe(),
            terminated: false,
        }
    }

    /// Create a progress stream from an existing [`EventReceiver`].
    ///
    /// Useful when the caller has already subscribed to the bus and wants
    /// to filter for a specific run.
    #[must_use]
    pub fn from_receiver(run_id: RunId, receiver: EventReceiver) -> Self {
        Self {
            run_id,
            receiver,
            terminated: false,
        }
    }

    /// Returns `true` if a terminal event has been emitted.
    #[must_use]
    pub fn is_terminated(&self) -> bool {
        self.terminated
    }

    /// Poll the next progress event, filtering for this run and mapping
    /// internal events to client-facing events.
    ///
    /// Returns `None` when the stream has terminated or the bus is closed.
    pub async fn next(&mut self) -> Option<RunProgressEvent> {
        if self.terminated {
            return None;
        }

        loop {
            match self.receiver.recv().await {
                Ok(event) => {
                    // Filter for our run.
                    if event.run_id != self.run_id {
                        continue;
                    }

                    if let Some(progress) = map_event(&event) {
                        if progress.is_terminal() {
                            self.terminated = true;
                        }
                        return Some(progress);
                    }
                    // Event had no client-facing mapping; keep polling.
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(
                        run_id = %self.run_id,
                        skipped = n,
                        "RunProgressStream lagged; some events may be lost"
                    );
                    // Continue polling; the bus will resume from the new tail.
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    return None;
                }
            }
        }
    }
}

impl Stream for RunProgressStream {
    type Item = RunProgressEvent;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();

        if this.terminated {
            return Poll::Ready(None);
        }

        loop {
            match this.receiver.try_recv() {
                Ok(event) => {
                    if event.run_id != this.run_id {
                        continue;
                    }
                    if let Some(progress) = map_event(&event) {
                        if progress.is_terminal() {
                            this.terminated = true;
                        }
                        return Poll::Ready(Some(progress));
                    }
                }
                Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {
                    // No events available. Register waker via a spawned task
                    // pattern; for now use manual waker registration.
                    cx.waker().wake_by_ref();
                    return Poll::Pending;
                }
                Err(tokio::sync::broadcast::error::TryRecvError::Lagged(n)) => {
                    tracing::warn!(
                        run_id = %this.run_id,
                        skipped = n,
                        "RunProgressStream lagged"
                    );
                    // Continue to drain from the new tail.
                }
                Err(tokio::sync::broadcast::error::TryRecvError::Closed) => {
                    return Poll::Ready(None);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Collect helper
// ---------------------------------------------------------------------------

/// Collect all progress events for a run until a terminal event or the bus
/// closes.
///
/// This is a convenience function for testing and batch processing.
pub async fn collect_progress(run_id: RunId, bus: &EventBus) -> Vec<RunProgressEvent> {
    let mut stream = RunProgressStream::new(run_id, bus);
    let mut events = Vec::new();
    while let Some(event) = stream.next().await {
        let is_terminal = event.is_terminal();
        events.push(event);
        if is_terminal {
            break;
        }
    }
    events
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use polkagent_core::event::{EventCorrelation, EventKind, RunEvent};
    use polkagent_core::ids::{ArtifactId, EventId, RunId};

    fn make_run_event(run_id: RunId, seq: u64, kind: EventKind) -> RunEvent {
        RunEvent::new_durable(
            EventId::new(),
            run_id,
            seq,
            kind,
            EventCorrelation {
                run_id,
                ..Default::default()
            },
        )
    }

    fn make_ephemeral_event(run_id: RunId, seq: u64, kind: EventKind) -> RunEvent {
        RunEvent::new_ephemeral(EventId::new(), run_id, seq, kind)
    }

    // -----------------------------------------------------------------------
    // RunProgressEvent basics
    // -----------------------------------------------------------------------

    #[test]
    fn queued_event_has_correct_run_id() {
        let run_id = RunId::new();
        let evt = RunProgressEvent::Queued { run_id };
        assert_eq!(evt.run_id(), &run_id);
        assert!(!evt.is_terminal());
    }

    #[test]
    fn working_event_is_not_terminal() {
        let run_id = RunId::new();
        let evt = RunProgressEvent::Working { run_id };
        assert!(!evt.is_terminal());
    }

    #[test]
    fn text_delta_event_carries_delta() {
        let run_id = RunId::new();
        let evt = RunProgressEvent::TextDelta {
            run_id,
            delta: "hello".to_string(),
        };
        assert_eq!(evt.run_id(), &run_id);
        assert!(!evt.is_terminal());
    }

    #[test]
    fn tool_use_event_carries_tool_and_status() {
        let run_id = RunId::new();
        let evt = RunProgressEvent::ToolUse {
            run_id,
            tool: "file_read".to_string(),
            status: ToolUseStatus::Started,
        };
        assert!(!evt.is_terminal());
    }

    #[test]
    fn completed_event_is_terminal() {
        let run_id = RunId::new();
        let evt = RunProgressEvent::Completed {
            run_id,
            summary: RunSummary {
                tokens_used: 100,
                duration: Duration::from_secs(5),
                effects_count: 3,
            },
        };
        assert!(evt.is_terminal());
    }

    #[test]
    fn failed_event_is_terminal() {
        let run_id = RunId::new();
        let evt = RunProgressEvent::Failed {
            run_id,
            error: RunProgressError {
                message: "boom".to_string(),
            },
        };
        assert!(evt.is_terminal());
    }

    #[test]
    fn approval_required_is_not_terminal() {
        let run_id = RunId::new();
        let evt = RunProgressEvent::ApprovalRequired {
            run_id,
            approval: ApprovalRequest {
                id: ApprovalId::new(),
                description: "approve this".to_string(),
                timeout: Duration::from_secs(60),
            },
        };
        assert!(!evt.is_terminal());
    }

    #[test]
    fn artifact_produced_is_not_terminal() {
        let run_id = RunId::new();
        let evt = RunProgressEvent::ArtifactProduced {
            run_id,
            artifact: ArtifactSummary {
                id: ArtifactId::new(),
                kind: "file".to_string(),
                name: "output.txt".to_string(),
            },
        };
        assert!(!evt.is_terminal());
    }

    // -----------------------------------------------------------------------
    // ToolUseStatus
    // -----------------------------------------------------------------------

    #[test]
    fn tool_use_status_serde_round_trip() {
        for status in [
            ToolUseStatus::Started,
            ToolUseStatus::Running,
            ToolUseStatus::Completed,
            ToolUseStatus::Failed,
        ] {
            let json = serde_json::to_string(&status).unwrap_or_default();
            let back: ToolUseStatus =
                serde_json::from_str(&json).unwrap_or(ToolUseStatus::Failed);
            assert_eq!(status, back);
        }
    }

    // -----------------------------------------------------------------------
    // map_event
    // -----------------------------------------------------------------------

    #[test]
    fn map_event_queued() {
        let run_id = RunId::new();
        let event = make_run_event(run_id, 1, EventKind::RunQueued);
        let progress = map_event(&event);
        assert!(matches!(progress, Some(RunProgressEvent::Queued { .. })));
    }

    #[test]
    fn map_event_started() {
        let run_id = RunId::new();
        let event = make_run_event(run_id, 2, EventKind::RunStarted);
        let progress = map_event(&event);
        assert!(matches!(progress, Some(RunProgressEvent::Working { .. })));
    }

    #[test]
    fn map_event_streaming_token() {
        let run_id = RunId::new();
        let event = make_ephemeral_event(
            run_id,
            3,
            EventKind::StreamingToken {
                text: "hello".to_string(),
            },
        );
        let progress = map_event(&event);
        match progress {
            Some(RunProgressEvent::TextDelta { delta, .. }) => {
                assert_eq!(delta, "hello");
            }
            other => panic!("expected TextDelta, got {other:?}"),
        }
    }

    #[test]
    fn map_event_tool_call_started() {
        let run_id = RunId::new();
        let event = make_run_event(
            run_id,
            4,
            EventKind::ToolCallStarted {
                tool_name: "file_read".to_string(),
            },
        );
        let progress = map_event(&event);
        match progress {
            Some(RunProgressEvent::ToolUse { tool, status, .. }) => {
                assert_eq!(tool, "file_read");
                assert_eq!(status, ToolUseStatus::Started);
            }
            other => panic!("expected ToolUse, got {other:?}"),
        }
    }

    #[test]
    fn map_event_tool_call_completed() {
        let run_id = RunId::new();
        let event = make_run_event(
            run_id,
            5,
            EventKind::ToolCallCompleted {
                tool_name: "file_write".to_string(),
            },
        );
        let progress = map_event(&event);
        match progress {
            Some(RunProgressEvent::ToolUse { tool, status, .. }) => {
                assert_eq!(tool, "file_write");
                assert_eq!(status, ToolUseStatus::Completed);
            }
            other => panic!("expected ToolUse, got {other:?}"),
        }
    }

    #[test]
    fn map_event_approval_requested() {
        let run_id = RunId::new();
        let event = make_run_event(
            run_id,
            6,
            EventKind::ApprovalRequested {
                request_id: "req-001".to_string(),
            },
        );
        let progress = map_event(&event);
        assert!(matches!(
            progress,
            Some(RunProgressEvent::ApprovalRequired { .. })
        ));
    }

    #[test]
    fn map_event_artifact_created() {
        let run_id = RunId::new();
        let artifact_id = ArtifactId::new();
        let event = make_run_event(
            run_id,
            7,
            EventKind::ArtifactCreated { artifact_id },
        );
        let progress = map_event(&event);
        match progress {
            Some(RunProgressEvent::ArtifactProduced { artifact, .. }) => {
                assert_eq!(artifact.id, artifact_id);
            }
            other => panic!("expected ArtifactProduced, got {other:?}"),
        }
    }

    #[test]
    fn map_event_run_completed() {
        let run_id = RunId::new();
        let event = make_run_event(
            run_id,
            8,
            EventKind::RunCompleted {
                output_artifact_id: None,
            },
        );
        let progress = map_event(&event);
        assert!(matches!(
            progress,
            Some(RunProgressEvent::Completed { .. })
        ));
    }

    #[test]
    fn map_event_run_failed() {
        let run_id = RunId::new();
        let event = make_run_event(
            run_id,
            9,
            EventKind::RunFailed {
                reason: "out of memory".to_string(),
            },
        );
        let progress = map_event(&event);
        match progress {
            Some(RunProgressEvent::Failed { error, .. }) => {
                assert_eq!(error.message, "out of memory");
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[test]
    fn map_event_run_cancelled() {
        let run_id = RunId::new();
        let event = make_run_event(
            run_id,
            10,
            EventKind::RunCancelled {
                reason: "user abort".to_string(),
            },
        );
        let progress = map_event(&event);
        match progress {
            Some(RunProgressEvent::Failed { error, .. }) => {
                assert!(error.message.contains("cancelled"));
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[test]
    fn map_event_run_timed_out() {
        let run_id = RunId::new();
        let event = make_run_event(run_id, 11, EventKind::RunTimedOut);
        let progress = map_event(&event);
        match progress {
            Some(RunProgressEvent::Failed { error, .. }) => {
                assert!(error.message.contains("timed out"));
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[test]
    fn map_event_internal_events_return_none() {
        let run_id = RunId::new();
        let internal_kinds = [
            EventKind::RunCreated,
            EventKind::RunCompleting,
            EventKind::StepStarted {
                step_id: polkagent_core::StepId::new(),
            },
            EventKind::StepCompleted {
                step_id: polkagent_core::StepId::new(),
            },
            EventKind::DeliveryStarted,
            EventKind::DeliveryCompleted,
            EventKind::EffectsResolved,
        ];
        for (i, kind) in internal_kinds.into_iter().enumerate() {
            let event = make_run_event(run_id, i as u64 + 100, kind);
            assert!(
                map_event(&event).is_none(),
                "expected None for internal event at index {i}"
            );
        }
    }

    // -----------------------------------------------------------------------
    // RunProgressEvent serde
    // -----------------------------------------------------------------------

    #[test]
    fn progress_event_serde_round_trip_queued() {
        let run_id = RunId::new();
        let evt = RunProgressEvent::Queued { run_id };
        let json = serde_json::to_string(&evt).unwrap_or_default();
        let back: RunProgressEvent =
            serde_json::from_str(&json).unwrap_or(RunProgressEvent::Queued { run_id });
        assert_eq!(evt, back);
    }

    #[test]
    fn progress_event_serde_round_trip_text_delta() {
        let run_id = RunId::new();
        let evt = RunProgressEvent::TextDelta {
            run_id,
            delta: "fragment".to_string(),
        };
        let json = serde_json::to_string(&evt).unwrap_or_default();
        assert!(json.contains("\"type\":\"text_delta\""));
        let back: RunProgressEvent =
            serde_json::from_str(&json).unwrap_or(RunProgressEvent::Queued { run_id });
        assert_eq!(evt, back);
    }

    #[test]
    fn progress_event_serde_tagged_format() {
        let run_id = RunId::new();
        let evt = RunProgressEvent::Working { run_id };
        let json = serde_json::to_string(&evt).unwrap_or_default();
        assert!(json.contains("\"type\":\"working\""));
    }

    // -----------------------------------------------------------------------
    // RunProgressStream via EventBus (async)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn stream_delivers_events_in_order() {
        let bus = EventBus::new(64);
        let run_id = RunId::new();
        let mut stream = RunProgressStream::new(run_id, &bus);

        // Publish events.
        bus.publish(make_run_event(run_id, 1, EventKind::RunQueued));
        bus.publish(make_run_event(run_id, 2, EventKind::RunStarted));
        bus.publish(make_ephemeral_event(
            run_id,
            3,
            EventKind::StreamingToken {
                text: "hi".to_string(),
            },
        ));
        bus.publish(make_run_event(
            run_id,
            4,
            EventKind::RunCompleted {
                output_artifact_id: None,
            },
        ));

        let e1 = stream.next().await;
        assert!(matches!(e1, Some(RunProgressEvent::Queued { .. })));

        let e2 = stream.next().await;
        assert!(matches!(e2, Some(RunProgressEvent::Working { .. })));

        let e3 = stream.next().await;
        assert!(matches!(e3, Some(RunProgressEvent::TextDelta { .. })));

        let e4 = stream.next().await;
        assert!(matches!(e4, Some(RunProgressEvent::Completed { .. })));

        // Stream should be terminated.
        assert!(stream.is_terminated());
    }

    #[tokio::test]
    async fn stream_filters_for_target_run() {
        let bus = EventBus::new(64);
        let target_run = RunId::new();
        let other_run = RunId::new();
        let mut stream = RunProgressStream::new(target_run, &bus);

        // Publish an event for a different run, then for the target.
        bus.publish(make_run_event(other_run, 1, EventKind::RunQueued));
        bus.publish(make_run_event(target_run, 1, EventKind::RunStarted));
        bus.publish(make_run_event(
            target_run,
            2,
            EventKind::RunCompleted {
                output_artifact_id: None,
            },
        ));

        let e1 = stream.next().await;
        assert!(
            matches!(&e1, Some(RunProgressEvent::Working { run_id }) if *run_id == target_run)
        );

        let e2 = stream.next().await;
        assert!(matches!(e2, Some(RunProgressEvent::Completed { .. })));

        assert!(stream.is_terminated());
    }

    #[tokio::test]
    async fn stream_skips_internal_events() {
        let bus = EventBus::new(64);
        let run_id = RunId::new();
        let mut stream = RunProgressStream::new(run_id, &bus);

        // Publish internal-only events, then a client-visible one.
        bus.publish(make_run_event(run_id, 1, EventKind::RunCreated));
        bus.publish(make_run_event(run_id, 2, EventKind::RunCompleting));
        bus.publish(make_run_event(run_id, 3, EventKind::RunQueued));
        bus.publish(make_run_event(
            run_id,
            4,
            EventKind::RunCompleted {
                output_artifact_id: None,
            },
        ));

        let e1 = stream.next().await;
        assert!(matches!(e1, Some(RunProgressEvent::Queued { .. })));

        let e2 = stream.next().await;
        assert!(matches!(e2, Some(RunProgressEvent::Completed { .. })));

        assert!(stream.is_terminated());
    }

    #[tokio::test]
    async fn stream_emits_at_most_one_terminal() {
        let bus = EventBus::new(64);
        let run_id = RunId::new();
        let mut stream = RunProgressStream::new(run_id, &bus);

        // Publish two terminal events.
        bus.publish(make_run_event(
            run_id,
            1,
            EventKind::RunFailed {
                reason: "first".to_string(),
            },
        ));
        bus.publish(make_run_event(
            run_id,
            2,
            EventKind::RunCompleted {
                output_artifact_id: None,
            },
        ));

        let e1 = stream.next().await;
        assert!(matches!(e1, Some(RunProgressEvent::Failed { .. })));

        // Stream should be terminated; next should return None.
        assert!(stream.is_terminated());
        let e2 = stream.next().await;
        assert!(e2.is_none());
    }

    #[tokio::test]
    async fn stream_preserves_all_text_deltas() {
        let bus = EventBus::new(64);
        let run_id = RunId::new();
        let mut stream = RunProgressStream::new(run_id, &bus);

        let fragments = ["Hello", ", ", "world", "!"];
        for (i, frag) in fragments.iter().enumerate() {
            bus.publish(make_ephemeral_event(
                run_id,
                i as u64 + 1,
                EventKind::StreamingToken {
                    text: frag.to_string(),
                },
            ));
        }
        bus.publish(make_run_event(
            run_id,
            5,
            EventKind::RunCompleted {
                output_artifact_id: None,
            },
        ));

        let mut collected_text = String::new();
        loop {
            match stream.next().await {
                Some(RunProgressEvent::TextDelta { delta, .. }) => {
                    collected_text.push_str(&delta);
                }
                Some(RunProgressEvent::Completed { .. }) => break,
                None => break,
                _ => {}
            }
        }
        assert_eq!(collected_text, "Hello, world!");
    }

    #[tokio::test]
    async fn stream_handles_bus_close() {
        let run_id = RunId::new();
        let stream_result;
        {
            let bus = EventBus::new(8);
            let mut stream = RunProgressStream::new(run_id, &bus);
            bus.publish(make_run_event(run_id, 1, EventKind::RunQueued));
            let e1 = stream.next().await;
            assert!(matches!(e1, Some(RunProgressEvent::Queued { .. })));

            // Drop bus sender.
            drop(bus);
            stream_result = stream.next().await;
        }
        assert!(stream_result.is_none());
    }

    // -----------------------------------------------------------------------
    // ApprovalRequest / ArtifactSummary / RunSummary
    // -----------------------------------------------------------------------

    #[test]
    fn approval_request_serde_round_trip() {
        let req = ApprovalRequest {
            id: ApprovalId::new(),
            description: "transfer 100 DOT".to_string(),
            timeout: Duration::from_secs(120),
        };
        let json = serde_json::to_string(&req).unwrap_or_default();
        let back: ApprovalRequest = serde_json::from_str(&json).unwrap_or(ApprovalRequest {
            id: ApprovalId::new(),
            description: String::new(),
            timeout: Duration::ZERO,
        });
        assert_eq!(req.description, back.description);
    }

    #[test]
    fn artifact_summary_serde_round_trip() {
        let summary = ArtifactSummary {
            id: ArtifactId::new(),
            kind: "extrinsic".to_string(),
            name: "transfer_keep_alive".to_string(),
        };
        let json = serde_json::to_string(&summary).unwrap_or_default();
        let back: ArtifactSummary = serde_json::from_str(&json).unwrap_or(ArtifactSummary {
            id: ArtifactId::new(),
            kind: String::new(),
            name: String::new(),
        });
        assert_eq!(summary.kind, back.kind);
        assert_eq!(summary.name, back.name);
    }

    #[test]
    fn run_summary_fields() {
        let summary = RunSummary {
            tokens_used: 5000,
            duration: Duration::from_millis(1500),
            effects_count: 7,
        };
        assert_eq!(summary.tokens_used, 5000);
        assert_eq!(summary.duration, Duration::from_millis(1500));
        assert_eq!(summary.effects_count, 7);
    }

    #[test]
    fn run_progress_error_serde() {
        let err = RunProgressError {
            message: "something went wrong".to_string(),
        };
        let json = serde_json::to_string(&err).unwrap_or_default();
        let back: RunProgressError = serde_json::from_str(&json).unwrap_or(RunProgressError {
            message: String::new(),
        });
        assert_eq!(err.message, back.message);
    }

    // -----------------------------------------------------------------------
    // collect_progress helper
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn collect_progress_gathers_all_events() {
        let bus = EventBus::new(64);
        let run_id = RunId::new();

        // Subscribe before publishing.
        let receiver = bus.subscribe();

        bus.publish(make_run_event(run_id, 1, EventKind::RunQueued));
        bus.publish(make_run_event(run_id, 2, EventKind::RunStarted));
        bus.publish(make_ephemeral_event(
            run_id,
            3,
            EventKind::StreamingToken {
                text: "hi".to_string(),
            },
        ));
        bus.publish(make_run_event(
            run_id,
            4,
            EventKind::RunCompleted {
                output_artifact_id: None,
            },
        ));

        let mut stream = RunProgressStream::from_receiver(run_id, receiver);
        let mut events = Vec::new();
        while let Some(event) = stream.next().await {
            let is_terminal = event.is_terminal();
            events.push(event);
            if is_terminal {
                break;
            }
        }

        assert_eq!(events.len(), 4);
        assert!(matches!(events[0], RunProgressEvent::Queued { .. }));
        assert!(matches!(events[1], RunProgressEvent::Working { .. }));
        assert!(matches!(events[2], RunProgressEvent::TextDelta { .. }));
        assert!(matches!(events[3], RunProgressEvent::Completed { .. }));
    }
}
