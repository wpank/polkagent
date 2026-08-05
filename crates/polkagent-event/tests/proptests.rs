//! Property-based tests for `polkagent-event`.
//!
//! These tests use `proptest` to verify invariants of `EventType` string
//! round-trips, default durability consistency, and `EventBus` delivery.

// Property fixtures use expect to stop at the exact serialization, runtime,
// or delivery invariant that failed for the generated case.
#![allow(
    clippy::expect_used,
    reason = "property-test assertions intentionally panic with focused diagnostics"
)]

use proptest::prelude::*;

use polkagent_core::DurabilityClass;
use polkagent_event::bus::EventBus;
use polkagent_event::types::EventType;

// =========================================================================
// Helpers
// =========================================================================

/// All `EventType` variants as a static list.
const ALL_EVENT_TYPES: &[EventType] = &[
    EventType::RunCreated,
    EventType::RunQueued,
    EventType::RunStarted,
    EventType::RunCompleting,
    EventType::RunCompleted,
    EventType::RunFailed,
    EventType::RunCancelled,
    EventType::RunTimedOut,
    EventType::RunResumed,
    EventType::RunRetryQueued,
    EventType::TurnStarted,
    EventType::TurnCompleted,
    EventType::TextDelta,
    EventType::ToolCallStarted,
    EventType::ToolCallCompleted,
    EventType::ThinkingStart,
    EventType::ThinkingDelta,
    EventType::ThinkingEnd,
    EventType::ProgressUpdate,
    EventType::EffectIntentCreated,
    EventType::EffectAttemptStarted,
    EventType::EffectOutcomeRecorded,
    EventType::EffectRetryScheduled,
    EventType::EffectCancelled,
    EventType::EffectLeaseExpired,
    EventType::ApprovalRequested,
    EventType::ApprovalGranted,
    EventType::ApprovalDenied,
    EventType::ApprovalExpired,
    EventType::MandateApplied,
    EventType::ErrorOccurred,
    EventType::PanicRecovered,
    EventType::IntegrityViolation,
    EventType::PolicyDenial,
    EventType::ClassificationEscalation,
    EventType::SystemStarted,
    EventType::SystemShutdown,
    EventType::BackupCompleted,
    EventType::RetentionEnforced,
    EventType::SchemasMigrated,
    EventType::ConfigurationChanged,
];

/// Strategy for arbitrary `EventType`.
fn arb_event_type() -> impl Strategy<Value = EventType> {
    prop::sample::select(ALL_EVENT_TYPES)
}

/// Create a `RunEvent` for testing the event bus.
fn make_event(
    run_id: polkagent_core::ids::RunId,
    sequence: u64,
) -> polkagent_core::event::RunEvent {
    use polkagent_core::event::{EventCorrelation, EventKind, RunEvent};
    use polkagent_core::ids::EventId;

    RunEvent::new_durable(
        EventId::new(),
        run_id,
        sequence,
        EventKind::RunCreated,
        EventCorrelation {
            run_id,
            ..Default::default()
        },
    )
}

// =========================================================================
// 1. EventType::as_str round-trips correctly
// =========================================================================

proptest! {
    /// `as_str` produces a non-empty snake_case string, and serde JSON
    /// round-trip preserves the value.
    #[test]
    fn event_type_as_str_round_trips(et in arb_event_type()) {
        let s = et.as_str();

        // as_str must return a non-empty string.
        prop_assert!(!s.is_empty(), "as_str() must be non-empty");

        // The string must be snake_case (lowercase, underscores only).
        prop_assert!(
            s.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
            "as_str() must be snake_case, got: {}",
            s
        );

        // Display must match as_str.
        prop_assert_eq!(
            et.to_string(), s,
            "Display and as_str must agree"
        );

        // serde JSON round-trip.
        let json = serde_json::to_string(&et).expect("serialize");
        let back: EventType = serde_json::from_str(&json).expect("deserialize");
        prop_assert_eq!(et, back, "serde round-trip must preserve EventType");
    }
}

// =========================================================================
// 2. All EventTypes have a valid default durability
// =========================================================================

proptest! {
    /// Every EventType variant returns a valid DurabilityClass from
    /// `default_durability()`. The returned value must be one of the
    /// three known variants.
    #[test]
    fn all_event_types_have_valid_durability(et in arb_event_type()) {
        let durability = et.default_durability();
        let is_valid = matches!(
            durability,
            DurabilityClass::Durable | DurabilityClass::Diagnostic | DurabilityClass::Ephemeral
        );
        prop_assert!(
            is_valid,
            "default_durability() must return a valid DurabilityClass for {:?}",
            et
        );
    }

    /// Streaming / ephemeral events have Ephemeral durability, never Durable.
    #[test]
    fn streaming_events_are_ephemeral(et in arb_event_type()) {
        let is_streaming = matches!(
            et,
            EventType::TextDelta
            | EventType::ThinkingStart
            | EventType::ThinkingDelta
            | EventType::ThinkingEnd
            | EventType::ProgressUpdate
        );
        if is_streaming {
            prop_assert_eq!(
                et.default_durability(),
                DurabilityClass::Ephemeral,
                "{:?} must be Ephemeral",
                et
            );
        }
    }

    /// Terminal lifecycle events must be Durable (they are critical state
    /// transitions that must survive crash).
    #[test]
    fn terminal_events_are_durable(et in arb_event_type()) {
        if et.is_terminal() {
            prop_assert_eq!(
                et.default_durability(),
                DurabilityClass::Durable,
                "terminal event {:?} must be Durable",
                et
            );
        }
    }
}

// =========================================================================
// 3. EventBus delivers to all subscribers
// =========================================================================

proptest! {
    /// Publishing an event to a bus with `n` subscribers delivers the event
    /// to all `n` of them.
    #[test]
    fn event_bus_delivers_to_all_subscribers(subscriber_count in 1..8usize) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime");

        rt.block_on(async {
            let bus = EventBus::new(64);
            let mut receivers = Vec::new();
            for _ in 0..subscriber_count {
                receivers.push(bus.subscribe());
            }

            let run_id = polkagent_core::ids::RunId::new();
            let event = make_event(run_id, 1);
            let event_id = event.id;

            let delivered = bus.publish(event);

            // All subscribers should receive the event.
            for rx in &mut receivers {
                let received = rx.recv().await.expect("should receive");
                assert_eq!(received.id, event_id);
            }
            assert_eq!(delivered, subscriber_count);
        });
    }

    /// Publishing N events delivers them in order to a single subscriber.
    #[test]
    fn event_bus_preserves_order(event_count in 1..20usize) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime");

        rt.block_on(async {
            let bus = EventBus::new(64);
            let mut rx = bus.subscribe();

            let run_id = polkagent_core::ids::RunId::new();
            for seq in 1..=event_count as u64 {
                bus.publish(make_event(run_id, seq));
            }

            let mut last_seq = 0u64;
            for _ in 0..event_count {
                let event = rx.recv().await.expect("should receive");
                assert!(
                    event.sequence > last_seq,
                    "events must arrive in order"
                );
                last_seq = event.sequence;
            }
        });
    }
}
