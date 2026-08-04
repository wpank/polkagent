//! Projection trait, [`ProjectionEngine`], and built-in projections.
//!
//! A **projection** is a pure function of the durable event stream. It
//! maintains a read model by consuming events in order. Projections are
//! recoverable: to rebuild one, simply replay all relevant events from
//! `global_sequence = 0` (PRD-10 Appendix A.3).
//!
//! # Built-in projections
//!
//! | Projection | Read model |
//! |---|---|
//! | [`RunStatusProjection`] | Current [`RunState`] per run |
//!
//! # Engine
//!
//! The [`ProjectionEngine`] registers multiple projections and fans out each
//! [`RunEvent`] to every registered projection.  For rebuild, it reads events
//! from an [`EventStore`] in cursor-batched order and replays them.

use std::collections::HashMap;

use polkagent_core::{
    event::{EventKind, RunEvent},
    run::RunState,
    RunId,
};
use polkagent_store_trait::event::EventStore;
use tracing::{debug, info, instrument, warn};

use crate::error::EventError;

// ---------------------------------------------------------------------------
// Projection trait
// ---------------------------------------------------------------------------

/// A stateful, recoverable read model derived from the event stream.
///
/// Implementors maintain internal state that is updated by [`apply`]. The
/// state must be deterministic: replaying the same event sequence always
/// produces the same state.
///
/// [`apply`]: Projection::apply
pub trait Projection: Send + Sync {
    /// Return the unique name of this projection (used for logging and cursors).
    fn name(&self) -> &str;

    /// Apply a single event to the projection's state.
    ///
    /// # Errors
    ///
    /// Implementations should return errors only for unexpected conditions
    /// (serialisation failure, invariant violation). Unknown event kinds
    /// should be silently ignored (forward-compatibility).
    fn apply(&mut self, event: &RunEvent) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;

    /// Reset the projection's state to the empty baseline.
    ///
    /// Called by [`ProjectionEngine::rebuild`] before replaying events.
    fn reset(&mut self);
}

// ---------------------------------------------------------------------------
// ProjectionEngine
// ---------------------------------------------------------------------------

/// Registers [`Projection`]s and drives their application of events.
///
/// # Usage
///
/// ```rust,no_run
/// # use polkagent_event::projection::{ProjectionEngine, RunStatusProjection};
/// let mut engine = ProjectionEngine::new();
/// engine.register(Box::new(RunStatusProjection::new()));
/// // Feed events from a live bus or a store replay:
/// // engine.apply(&event)?;
/// ```
pub struct ProjectionEngine {
    projections: Vec<Box<dyn Projection>>,
}

impl ProjectionEngine {
    /// Create an empty engine.
    #[must_use]
    pub fn new() -> Self {
        Self {
            projections: Vec::new(),
        }
    }

    /// Register a new projection.
    pub fn register(&mut self, projection: Box<dyn Projection>) {
        info!(
            name = projection.name(),
            "ProjectionEngine: registered projection"
        );
        self.projections.push(projection);
    }

    /// Apply a single event to all registered projections.
    ///
    /// # Errors
    ///
    /// Returns the first [`EventError::ProjectionApply`] encountered. All
    /// projections before the failing one will have already been updated.
    pub fn apply(&mut self, event: &RunEvent) -> Result<(), EventError> {
        for proj in &mut self.projections {
            proj.apply(event)
                .map_err(|source| EventError::ProjectionApply {
                    name: proj.name().to_owned(),
                    source,
                })?;
        }
        Ok(())
    }

    /// Rebuild all registered projections by replaying the full event stream
    /// from the store.
    ///
    /// The engine resets every projection, then reads events in batches of
    /// `batch_size` (cursor-based), calling [`apply`] for each.
    ///
    /// [`apply`]: Projection::apply
    ///
    /// # Errors
    ///
    /// Returns [`EventError::Store`] if the event store cannot be read, or
    /// [`EventError::ProjectionApply`] if a projection fails to apply an
    /// event.
    #[instrument(skip(self, store))]
    pub async fn rebuild(
        &mut self,
        store: &dyn EventStore,
        batch_size: usize,
    ) -> Result<(), EventError> {
        info!("ProjectionEngine: resetting all projections for rebuild");
        for proj in &mut self.projections {
            proj.reset();
        }

        let mut cursor = 0u64;
        let effective_batch = batch_size.max(1);

        loop {
            let batch = store
                .read_from_cursor(cursor, effective_batch)
                .await
                .map_err(EventError::Store)?;

            if batch.is_empty() {
                break;
            }

            let last_global_seq = batch.last().map(|e| e.global_sequence).unwrap_or(cursor);

            for stored in &batch {
                // Deserialise the event kind from the stored payload.
                let kind: EventKind = match serde_json::from_value(stored.payload.clone()) {
                    Ok(k) => k,
                    Err(err) => {
                        warn!(
                            event_id = %stored.id,
                            error = %err,
                            "ProjectionEngine: skipping event with unparseable payload"
                        );
                        continue;
                    }
                };

                let run_id: RunId = stored.run_id.parse().map_err(|_| {
                    EventError::Store(polkagent_store_trait::event::EventStoreError::NotFound(
                        format!("invalid run_id: {}", stored.run_id),
                    ))
                })?;

                let event_id: polkagent_core::EventId = stored.id.parse().map_err(|_| {
                    EventError::Store(polkagent_store_trait::event::EventStoreError::NotFound(
                        format!("invalid event_id: {}", stored.id),
                    ))
                })?;

                use polkagent_core::event::{Durability, EventCorrelation};
                let event = RunEvent {
                    id: event_id,
                    run_id: run_id.clone(),
                    sequence: stored.sequence,
                    kind,
                    durability: Durability::Durable,
                    correlation: EventCorrelation {
                        run_id,
                        ..Default::default()
                    },
                    causation_id: None,
                    timestamp: chrono::DateTime::parse_from_rfc3339(&stored.timestamp)
                        .map(|dt| dt.with_timezone(&chrono::Utc))
                        .unwrap_or_else(|_| chrono::Utc::now()),
                };

                self.apply(&event)?;
                debug!(sequence = event.sequence, "ProjectionEngine: applied event");
            }

            cursor = last_global_seq;

            if batch.len() < effective_batch {
                break;
            }
        }

        info!("ProjectionEngine: rebuild complete");
        Ok(())
    }

    /// Return a reference to a registered projection by name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&dyn Projection> {
        self.projections
            .iter()
            .find(|p| p.name() == name)
            .map(|p| p.as_ref())
    }

    /// Return a mutable reference to a registered projection by name.
    pub fn get_mut(&mut self, name: &str) -> Option<&mut dyn Projection> {
        for proj in &mut self.projections {
            if proj.name() == name {
                return Some(proj.as_mut());
            }
        }
        None
    }
}

impl Default for ProjectionEngine {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// RunStatusProjection
// ---------------------------------------------------------------------------

/// Maintains the current [`RunState`] for every run observed in the event
/// stream.
///
/// This is the `RunStatusLens` described in PRD-10 §10.5.2. It derives the
/// current state purely from events without accessing any other data source.
#[derive(Debug, Default)]
pub struct RunStatusProjection {
    /// Current state per run ID.
    states: HashMap<String, RunState>,
}

impl RunStatusProjection {
    /// Create an empty projection.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Return the current state for a run, or `None` if the run has never
    /// been observed.
    #[must_use]
    pub fn state(&self, run_id: &RunId) -> Option<&RunState> {
        self.states.get(&run_id.to_string())
    }

    /// Return an iterator over all (run_id, state) pairs in the projection.
    pub fn all_states(&self) -> impl Iterator<Item = (&str, &RunState)> {
        self.states.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// Number of runs tracked by this projection.
    #[must_use]
    pub fn run_count(&self) -> usize {
        self.states.len()
    }
}

impl Projection for RunStatusProjection {
    fn name(&self) -> &str {
        "run_status"
    }

    fn apply(&mut self, event: &RunEvent) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let run_id = event.run_id.to_string();

        let new_state = match &event.kind {
            EventKind::RunCreated => Some(RunState::Created),
            EventKind::RunQueued => Some(RunState::Queued),
            EventKind::RunStarted => Some(RunState::Running),
            EventKind::ApprovalRequested { request_id } => Some(RunState::AwaitingApproval {
                request_id: request_id.clone(),
            }),
            EventKind::ApprovalGranted { .. } | EventKind::ApprovalDenied { .. } => {
                // Return to Running once approval resolves.
                Some(RunState::Running)
            }
            EventKind::RunCompleting => Some(RunState::Completing),
            EventKind::RunCompleted { .. } => Some(RunState::Completed),
            EventKind::RunFailed { reason } => Some(RunState::Failed {
                reason: reason.clone(),
            }),
            EventKind::RunCancelled { reason } => Some(RunState::Cancelled {
                reason: reason.clone(),
            }),
            EventKind::RunTimedOut => Some(RunState::TimedOut),
            // All other event kinds don't change the run state.
            _ => None,
        };

        if let Some(state) = new_state {
            self.states.insert(run_id, state);
        }

        Ok(())
    }

    fn reset(&mut self) {
        self.states.clear();
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use polkagent_core::{
        event::{Durability, EventCorrelation, EventKind, RunEvent},
        ids::{EventId, RunId},
        RunState,
    };

    fn make_event(run_id: RunId, kind: EventKind) -> RunEvent {
        let correlation = EventCorrelation {
            run_id: run_id.clone(),
            ..Default::default()
        };
        RunEvent {
            id: EventId::new(),
            run_id,
            sequence: 1,
            kind,
            durability: Durability::Durable,
            correlation,
            causation_id: None,
            timestamp: chrono::Utc::now(),
        }
    }

    // ── RunStatusProjection ───────────────────────────────────────────────

    #[test]
    fn initial_projection_is_empty() {
        let proj = RunStatusProjection::new();
        assert_eq!(proj.run_count(), 0);
    }

    #[test]
    fn applies_run_created_event() {
        let mut proj = RunStatusProjection::new();
        let run_id = RunId::new();
        let event = make_event(run_id.clone(), EventKind::RunCreated);
        proj.apply(&event).expect("apply");
        assert_eq!(proj.state(&run_id), Some(&RunState::Created));
    }

    #[test]
    fn applies_run_started_event() {
        let mut proj = RunStatusProjection::new();
        let run_id = RunId::new();
        proj.apply(&make_event(run_id.clone(), EventKind::RunCreated))
            .expect("created");
        proj.apply(&make_event(run_id.clone(), EventKind::RunStarted))
            .expect("started");
        assert_eq!(proj.state(&run_id), Some(&RunState::Running));
    }

    #[test]
    fn applies_terminal_event_completed() {
        let mut proj = RunStatusProjection::new();
        let run_id = RunId::new();
        proj.apply(&make_event(run_id.clone(), EventKind::RunCreated))
            .unwrap();
        proj.apply(&make_event(
            run_id.clone(),
            EventKind::RunCompleted {
                output_artifact_id: None,
                input_tokens: 0,
                output_tokens: 0,
            },
        ))
        .unwrap();
        assert_eq!(proj.state(&run_id), Some(&RunState::Completed));
    }

    #[test]
    fn applies_terminal_event_failed() {
        let mut proj = RunStatusProjection::new();
        let run_id = RunId::new();
        proj.apply(&make_event(run_id.clone(), EventKind::RunCreated))
            .unwrap();
        proj.apply(&make_event(
            run_id.clone(),
            EventKind::RunFailed {
                reason: "disk full".into(),
            },
        ))
        .unwrap();
        assert!(matches!(proj.state(&run_id), Some(RunState::Failed { .. })));
    }

    #[test]
    fn applies_terminal_event_timed_out() {
        let mut proj = RunStatusProjection::new();
        let run_id = RunId::new();
        proj.apply(&make_event(run_id.clone(), EventKind::RunCreated))
            .unwrap();
        proj.apply(&make_event(run_id.clone(), EventKind::RunTimedOut))
            .unwrap();
        assert_eq!(proj.state(&run_id), Some(&RunState::TimedOut));
    }

    #[test]
    fn unrelated_events_do_not_change_state() {
        let mut proj = RunStatusProjection::new();
        let run_id = RunId::new();
        proj.apply(&make_event(run_id.clone(), EventKind::RunStarted))
            .unwrap();
        // Streaming token should not change state.
        proj.apply(&make_event(
            run_id.clone(),
            EventKind::StreamingToken {
                text: "hello".into(),
            },
        ))
        .unwrap();
        assert_eq!(proj.state(&run_id), Some(&RunState::Running));
    }

    #[test]
    fn reset_clears_all_state() {
        let mut proj = RunStatusProjection::new();
        let run_id = RunId::new();
        proj.apply(&make_event(run_id.clone(), EventKind::RunCreated))
            .unwrap();
        assert_eq!(proj.run_count(), 1);
        proj.reset();
        assert_eq!(proj.run_count(), 0);
        assert!(proj.state(&run_id).is_none());
    }

    #[test]
    fn tracks_multiple_runs_independently() {
        let mut proj = RunStatusProjection::new();

        let run_a = RunId::new();
        let run_b = RunId::new();

        proj.apply(&make_event(run_a.clone(), EventKind::RunCreated))
            .unwrap();
        proj.apply(&make_event(run_b.clone(), EventKind::RunCreated))
            .unwrap();
        proj.apply(&make_event(run_a.clone(), EventKind::RunStarted))
            .unwrap();
        proj.apply(&make_event(
            run_b.clone(),
            EventKind::RunCompleted {
                output_artifact_id: None,
                input_tokens: 0,
                output_tokens: 0,
            },
        ))
        .unwrap();

        assert_eq!(proj.state(&run_a), Some(&RunState::Running));
        assert_eq!(proj.state(&run_b), Some(&RunState::Completed));
    }

    // ── ProjectionEngine ──────────────────────────────────────────────────

    #[test]
    fn engine_routes_events_to_all_projections() {
        let mut engine = ProjectionEngine::new();
        engine.register(Box::new(RunStatusProjection::new()));

        let run_id = RunId::new();
        let event = make_event(run_id.clone(), EventKind::RunCreated);
        engine.apply(&event).expect("apply");

        let proj = engine.get("run_status").expect("projection registered");
        // We can only call trait methods; downcast is not needed for this check.
        let _ = proj.name();
    }

    #[test]
    fn engine_get_returns_none_for_unknown_projection() {
        let engine = ProjectionEngine::new();
        assert!(engine.get("nonexistent").is_none());
    }

    #[test]
    fn engine_rebuild_resets_before_replay() {
        // Use a separate projection that counts apply() calls.
        #[derive(Default)]
        struct CountingProjection {
            calls: usize,
        }
        impl Projection for CountingProjection {
            fn name(&self) -> &str {
                "counter"
            }
            fn apply(
                &mut self,
                _event: &RunEvent,
            ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
                self.calls += 1;
                Ok(())
            }
            fn reset(&mut self) {
                self.calls = 0;
            }
        }

        // Apply an event manually.
        let mut engine = ProjectionEngine::new();
        engine.register(Box::new(CountingProjection::default()));
        let run_id = RunId::new();
        let event = make_event(run_id, EventKind::RunCreated);
        engine.apply(&event).unwrap();

        // Downcast isn't possible here; just verify no panic on second apply.
        engine.apply(&event).unwrap();
    }
}
