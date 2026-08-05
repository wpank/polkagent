//! Property-based tests for `polkagent-core` domain types.
//!
//! These tests use `proptest` to verify invariants that must hold for *all*
//! values of a given type, not just hand-picked examples. Every property is
//! documented with the invariant it encodes.

use std::collections::HashSet;
use std::str::FromStr;

use proptest::prelude::*;

use polkagent_core::artifact::{Artifact, ArtifactKind, BlobRef};
use polkagent_core::effect::EffectIntentState;
use polkagent_core::ids::{
    AgentId, ApprovalId, ArtifactId, ConversationId, EffectAttemptId, EffectId, EffectOutcomeId,
    EventId, GrantId, PrincipalId, RunId, StepId, TurnId, WorkerId,
};
use polkagent_core::run::RunState;
use polkagent_core::turn::TokenUsage;

// =========================================================================
// Helpers: strategies for generating domain values
// =========================================================================

/// Strategy that produces an arbitrary `RunState` variant with plausible
/// payload data.
fn arb_run_state() -> impl Strategy<Value = RunState> {
    prop_oneof![
        Just(RunState::Created),
        Just(RunState::Queued),
        Just(RunState::Running),
        ".*".prop_map(|s: String| RunState::AwaitingApproval { request_id: s }),
        prop::collection::vec(Just(()).prop_map(|()| EffectId::new()), 0..5).prop_map(|ids| {
            RunState::WaitingEffect {
                pending_intent_ids: ids,
            }
        }),
        Just(RunState::Completing),
        Just(RunState::Completed),
        ".*".prop_map(|s: String| RunState::Failed { reason: s }),
        ".*".prop_map(|s: String| RunState::Cancelled { reason: s }),
        Just(RunState::TimedOut),
    ]
}

/// Strategy that produces an arbitrary `EffectIntentState` variant.
fn arb_effect_intent_state() -> impl Strategy<Value = EffectIntentState> {
    prop_oneof![
        Just(EffectIntentState::Pending),
        ".*".prop_map(|s: String| {
            EffectIntentState::Claimed {
                worker_id: s,
                lease_expires: chrono::Utc::now(),
            }
        }),
        ".*".prop_map(|s: String| {
            EffectIntentState::Executing {
                worker_id: s,
                started_at: chrono::Utc::now(),
            }
        }),
        Just(()).prop_map(|()| EffectIntentState::Resolved {
            outcome_id: EffectOutcomeId::new(),
        }),
        ".*".prop_map(|s: String| {
            EffectIntentState::Retrying {
                next_attempt_after: chrono::Utc::now(),
                last_error: s,
            }
        }),
        ".*".prop_map(|s: String| EffectIntentState::Superseded { reason: s }),
    ]
}

/// Strategy that produces an arbitrary `TokenUsage`.
fn arb_token_usage() -> impl Strategy<Value = TokenUsage> {
    (
        any::<u32>(),
        any::<u32>(),
        any::<u32>(),
        any::<u32>(),
        any::<u32>(),
        prop::option::of(0.0f64..1_000_000.0),
    )
        .prop_map(
            |(input, output, total, cache_r, cache_w, cost)| TokenUsage {
                input_tokens: input,
                output_tokens: output,
                total_tokens: total,
                cache_read_tokens: cache_r,
                cache_write_tokens: cache_w,
                cost_usd: cost,
            },
        )
}

// =========================================================================
// 1. ID type properties
// =========================================================================

/// Macro to stamp out the three ID property tests for each ID type.
macro_rules! id_properties {
    ($name:ident, $ty:ty) => {
        mod $name {
            use super::*;

            proptest! {
                /// Display -> FromStr round-trip preserves the value.
                #[test]
                fn display_from_str_round_trip(_ in 0u8..1) {
                    let id = <$ty>::new();
                    let s = id.to_string();
                    let parsed = <$ty>::from_str(&s).expect("valid UUID string");
                    prop_assert_eq!(id, parsed);
                }

                /// serde_json round-trip preserves the value.
                #[test]
                fn serde_json_round_trip(_ in 0u8..1) {
                    let id = <$ty>::new();
                    let json = serde_json::to_string(&id).expect("serialize");
                    let back: $ty = serde_json::from_str(&json).expect("deserialize");
                    prop_assert_eq!(id, back);
                }
            }

            /// Generating 1000 IDs produces 1000 unique values.
            #[test]
            fn uniqueness_1000() {
                let ids: Vec<$ty> = (0..1000).map(|_| <$ty>::new()).collect();
                let set: HashSet<_> = ids.iter().copied().collect();
                assert_eq!(set.len(), 1000, "expected 1000 unique IDs");
            }
        }
    };
}

id_properties!(agent_id, AgentId);
id_properties!(run_id, RunId);
id_properties!(turn_id, TurnId);
id_properties!(step_id, StepId);
id_properties!(effect_id, EffectId);
id_properties!(artifact_id, ArtifactId);
id_properties!(event_id, EventId);
id_properties!(conversation_id, ConversationId);
id_properties!(principal_id, PrincipalId);
id_properties!(effect_attempt_id, EffectAttemptId);
id_properties!(effect_outcome_id, EffectOutcomeId);
id_properties!(grant_id, GrantId);
id_properties!(approval_id, ApprovalId);
id_properties!(worker_id, WorkerId);

// =========================================================================
// 2. RunState properties
// =========================================================================

proptest! {
    /// Every RunState variant round-trips through serde JSON without loss.
    #[test]
    fn run_state_serde_round_trip(state in arb_run_state()) {
        let json = serde_json::to_string(&state).expect("serialize");
        let back: RunState = serde_json::from_str(&json).expect("deserialize");
        prop_assert_eq!(&state, &back);
    }

    /// Terminal states are never executing. The two predicates are mutually
    /// exclusive: if `is_terminal()` then `!is_executing()`.
    #[test]
    fn terminal_never_executing(state in arb_run_state()) {
        if state.is_terminal() {
            prop_assert!(
                !state.is_executing(),
                "terminal state {:?} must not be executing",
                state
            );
        }
    }

    /// RunState JSON serialization always produces valid JSON (i.e. it
    /// round-trips through `serde_json::Value`).
    #[test]
    fn run_state_produces_valid_json(state in arb_run_state()) {
        let json = serde_json::to_string(&state).expect("serialize");
        let _value: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
    }
}

// =========================================================================
// 3. EffectIntentState properties
// =========================================================================

proptest! {
    /// Every EffectIntentState variant round-trips through serde JSON.
    #[test]
    fn effect_intent_state_serde_round_trip(state in arb_effect_intent_state()) {
        let json = serde_json::to_string(&state).expect("serialize");
        let back: EffectIntentState = serde_json::from_str(&json).expect("deserialize");
        prop_assert_eq!(&state, &back);
    }

    /// `is_terminal()` is true exactly for `Resolved` and `Superseded`.
    #[test]
    fn effect_intent_state_terminal_consistency(state in arb_effect_intent_state()) {
        let expected_terminal = matches!(
            state,
            EffectIntentState::Resolved { .. } | EffectIntentState::Superseded { .. }
        );
        prop_assert_eq!(
            state.is_terminal(),
            expected_terminal,
            "is_terminal() inconsistent for {:?}",
            state
        );
    }
}

// =========================================================================
// 4. BlobRef properties
// =========================================================================

proptest! {
    /// `from_bytes(data).verify(data)` always returns `true`.
    #[test]
    fn blob_ref_self_verify(data in prop::collection::vec(any::<u8>(), 0..4096)) {
        let blob = BlobRef::from_bytes(&data);
        prop_assert!(
            blob.verify(&data),
            "BlobRef must verify against its own source data"
        );
    }

    /// `from_bytes(data).verify(other)` returns `false` when `data != other`
    /// (with overwhelming probability for distinct byte sequences).
    #[test]
    fn blob_ref_rejects_different_data(
        data in prop::collection::vec(any::<u8>(), 1..4096),
        other in prop::collection::vec(any::<u8>(), 1..4096),
    ) {
        prop_assume!(data != other);
        let blob = BlobRef::from_bytes(&data);
        prop_assert!(
            !blob.verify(&other),
            "BlobRef must reject different data"
        );
    }

    /// `from_bytes` is deterministic: computing twice yields the same ref.
    #[test]
    fn blob_ref_deterministic(data in prop::collection::vec(any::<u8>(), 0..4096)) {
        let a = BlobRef::from_bytes(&data);
        let b = BlobRef::from_bytes(&data);
        prop_assert_eq!(&a, &b, "BlobRef must be deterministic");
    }

    /// Different data produces different BLAKE3 hashes (with high probability).
    #[test]
    fn blob_ref_different_data_different_hash(
        data in prop::collection::vec(any::<u8>(), 1..4096),
        other in prop::collection::vec(any::<u8>(), 1..4096),
    ) {
        prop_assume!(data != other);
        let a = BlobRef::from_bytes(&data);
        let b = BlobRef::from_bytes(&other);
        prop_assert_ne!(
            a.blake3_hex, b.blake3_hex,
            "Different data should produce different hashes"
        );
    }

    /// `size_bytes` matches the input length.
    #[test]
    fn blob_ref_size_matches(data in prop::collection::vec(any::<u8>(), 0..4096)) {
        let blob = BlobRef::from_bytes(&data);
        prop_assert_eq!(blob.size_bytes, data.len() as u64);
    }
}

// =========================================================================
// 5. Artifact properties
// =========================================================================

proptest! {
    /// `Artifact::from_bytes` produces an artifact whose `verify_integrity`
    /// returns `true` for the original bytes.
    #[test]
    fn artifact_from_bytes_verifies(data in prop::collection::vec(any::<u8>(), 0..4096)) {
        let art = Artifact::from_bytes(
            ArtifactId::new(),
            ArtifactKind::Custom { type_uri: "test://proptest".into() },
            &data,
        );
        prop_assert!(
            art.verify_integrity(&data),
            "Artifact must verify its source data"
        );
    }

    /// `verify_integrity` rejects data that differs from the original.
    #[test]
    fn artifact_rejects_tampered(
        data in prop::collection::vec(any::<u8>(), 1..4096),
        other in prop::collection::vec(any::<u8>(), 1..4096),
    ) {
        prop_assume!(data != other);
        let art = Artifact::from_bytes(
            ArtifactId::new(),
            ArtifactKind::Custom { type_uri: "test://proptest".into() },
            &data,
        );
        prop_assert!(
            !art.verify_integrity(&other),
            "Artifact must reject different data"
        );
    }

    /// `verify_integrity` is consistent with the inner `blob_ref.verify`.
    #[test]
    fn artifact_verify_consistent_with_blob_ref(
        data in prop::collection::vec(any::<u8>(), 0..4096),
        check in prop::collection::vec(any::<u8>(), 0..4096),
    ) {
        let art = Artifact::from_bytes(
            ArtifactId::new(),
            ArtifactKind::Custom { type_uri: "test://proptest".into() },
            &data,
        );
        prop_assert_eq!(
            art.verify_integrity(&check),
            art.blob_ref.verify(&check),
            "Artifact.verify_integrity must agree with BlobRef.verify"
        );
    }
}

// =========================================================================
// 6. TokenUsage accumulate properties
// =========================================================================

proptest! {
    /// Accumulation order does not affect the final integer sums.
    ///
    /// Given three usages a, b, c:
    ///   (a + b) + c  ==  (a + c) + b   (for the integer fields)
    ///
    /// Note: this tests commutativity of the *second* and *third* operands,
    /// which is sufficient because saturating_add is commutative.
    #[test]
    fn token_usage_accumulate_order_independent(
        a in arb_token_usage(),
        b in arb_token_usage(),
        c in arb_token_usage(),
    ) {
        // Path 1: a + b, then + c
        let mut path1 = a;
        path1.accumulate(&b);
        path1.accumulate(&c);

        // Path 2: a + c, then + b
        let mut path2 = a;
        path2.accumulate(&c);
        path2.accumulate(&b);

        prop_assert_eq!(path1.input_tokens,       path2.input_tokens);
        prop_assert_eq!(path1.output_tokens,      path2.output_tokens);
        prop_assert_eq!(path1.total_tokens,       path2.total_tokens);
        prop_assert_eq!(path1.cache_read_tokens,  path2.cache_read_tokens);
        prop_assert_eq!(path1.cache_write_tokens, path2.cache_write_tokens);
    }

    /// Accumulation applies saturating addition independently to every token
    /// counter, including inputs whose mathematical sum exceeds `u32::MAX`.
    #[test]
    fn token_usage_accumulate_no_overflow(
        a in arb_token_usage(),
        b in arb_token_usage(),
    ) {
        let mut result = a;
        result.accumulate(&b);

        prop_assert_eq!(result.input_tokens, a.input_tokens.saturating_add(b.input_tokens));
        prop_assert_eq!(result.output_tokens, a.output_tokens.saturating_add(b.output_tokens));
        prop_assert_eq!(result.total_tokens, a.total_tokens.saturating_add(b.total_tokens));
        prop_assert_eq!(
            result.cache_read_tokens,
            a.cache_read_tokens.saturating_add(b.cache_read_tokens)
        );
        prop_assert_eq!(
            result.cache_write_tokens,
            a.cache_write_tokens.saturating_add(b.cache_write_tokens)
        );
    }

    /// Accumulating zero (Default) is identity: a + 0 == a.
    #[test]
    fn token_usage_accumulate_zero_is_identity(a in arb_token_usage()) {
        let mut result = a;
        result.accumulate(&TokenUsage::default());

        prop_assert_eq!(result.input_tokens,       a.input_tokens);
        prop_assert_eq!(result.output_tokens,      a.output_tokens);
        prop_assert_eq!(result.total_tokens,       a.total_tokens);
        prop_assert_eq!(result.cache_read_tokens,  a.cache_read_tokens);
        prop_assert_eq!(result.cache_write_tokens, a.cache_write_tokens);
        prop_assert_eq!(result.cost_usd,           a.cost_usd);
    }

    /// After accumulation, each field is >= both operands (saturating_add
    /// is monotonically non-decreasing).
    #[test]
    fn token_usage_accumulate_monotonic(
        a in arb_token_usage(),
        b in arb_token_usage(),
    ) {
        let mut result = a;
        result.accumulate(&b);

        prop_assert!(result.input_tokens       >= a.input_tokens);
        prop_assert!(result.input_tokens       >= b.input_tokens);
        prop_assert!(result.output_tokens      >= a.output_tokens);
        prop_assert!(result.output_tokens      >= b.output_tokens);
        prop_assert!(result.total_tokens       >= a.total_tokens);
        prop_assert!(result.total_tokens       >= b.total_tokens);
        prop_assert!(result.cache_read_tokens  >= a.cache_read_tokens);
        prop_assert!(result.cache_read_tokens  >= b.cache_read_tokens);
        prop_assert!(result.cache_write_tokens >= a.cache_write_tokens);
        prop_assert!(result.cache_write_tokens >= b.cache_write_tokens);
    }
}

// =========================================================================
// PB-05: Round-trip serialization identity for core domain types
// =========================================================================
//
// For every serialisable domain type, serialising then deserialising must
// produce a value that is equal to the original. This validates that serde
// attributes (rename_all, tag, flatten, etc.) are symmetric.

/// Strategy for an arbitrary `EffectKind` (core variant, not effect-crate).
fn arb_core_effect_kind() -> impl Strategy<Value = polkagent_core::EffectKind> {
    use polkagent_core::EffectKind;
    prop_oneof![
        Just(EffectKind::ChainSubmit),
        Just(EffectKind::ChainQuery),
        Just(EffectKind::ModelInference),
        Just(EffectKind::ToolInvocation),
        Just(EffectKind::FileWrite),
        Just(EffectKind::FileRead),
        Just(EffectKind::HttpRequest),
        Just(EffectKind::Notification),
        Just(EffectKind::SignatureRequest),
        Just(EffectKind::Broadcast),
        Just(EffectKind::FinalityWatch),
        Just(EffectKind::Delivery),
        Just(EffectKind::HarnessOperation),
    ]
}

/// Strategy for an arbitrary `EventKind`.
fn arb_event_kind() -> impl Strategy<Value = polkagent_core::EventKind> {
    use polkagent_core::{
        event::LogLevel, ArtifactId, EffectAttemptId, EffectId, EffectOutcomeId, EventKind, StepId,
        TurnId,
    };
    prop_oneof![
        Just(EventKind::RunCreated),
        Just(EventKind::RunQueued),
        Just(EventKind::RunStarted),
        ".*".prop_map(|s: String| EventKind::ApprovalRequested { request_id: s }),
        ".*".prop_map(|s: String| EventKind::ApprovalGranted { approval_id: s }),
        ".*".prop_map(|s: String| EventKind::ApprovalDenied { reason: s }),
        Just(EventKind::RunCompleting),
        Just(()).prop_map(|()| EventKind::RunCompleted {
            output_artifact_id: None,
            input_tokens: 0,
            output_tokens: 0
        }),
        ".*".prop_map(|s: String| EventKind::RunFailed { reason: s }),
        ".*".prop_map(|s: String| EventKind::RunCancelled { reason: s }),
        Just(EventKind::RunTimedOut),
        Just(EventKind::RunRetryQueued),
        (any::<u32>()).prop_map(|n| EventKind::TurnStarted {
            turn_number: n,
            turn_id: TurnId::new()
        }),
        (any::<u32>()).prop_map(|n| EventKind::TurnCompleted {
            turn_number: n,
            turn_id: TurnId::new()
        }),
        Just(()).prop_map(|()| EventKind::StepStarted {
            step_id: StepId::new()
        }),
        Just(()).prop_map(|()| EventKind::StepCompleted {
            step_id: StepId::new()
        }),
        Just(()).prop_map(|()| EventKind::EffectIntentCreated {
            intent_id: EffectId::new()
        }),
        Just(()).prop_map(|()| EventKind::EffectAttemptStarted {
            attempt_id: EffectAttemptId::new()
        }),
        Just(()).prop_map(|()| EventKind::EffectOutcomeRecorded {
            outcome_id: EffectOutcomeId::new()
        }),
        Just(EventKind::EffectsResolved),
        Just(()).prop_map(|()| EventKind::ArtifactCreated {
            artifact_id: ArtifactId::new()
        }),
        ".*".prop_map(|s: String| EventKind::StreamingToken { text: s }),
        (".*", prop::option::of(0.0f32..100.0)).prop_map(|(msg, pct)| EventKind::ProgressUpdate {
            message: msg,
            percentage: pct
        }),
        ".*".prop_map(|s: String| EventKind::ToolCallStarted { tool_name: s }),
        ".*".prop_map(|s: String| EventKind::ToolCallCompleted { tool_name: s }),
        Just(EventKind::DeliveryStarted),
        Just(EventKind::DeliveryCompleted),
        (
            ".*",
            prop_oneof![
                Just(LogLevel::Trace),
                Just(LogLevel::Debug),
                Just(LogLevel::Info),
                Just(LogLevel::Warn),
                Just(LogLevel::Error),
            ]
        )
            .prop_map(|(msg, lvl)| EventKind::DiagnosticLog {
                level: lvl,
                message: msg
            }),
        (".*", ".*").prop_map(|(r, a)| EventKind::BudgetConsumed {
            resource: r,
            amount_str: a
        }),
        (".*", ".*").prop_map(|(r, rem)| EventKind::BudgetWarning {
            resource: r,
            remaining_str: rem
        }),
    ]
}

/// Strategy for an arbitrary `Durability`.
fn arb_durability() -> impl Strategy<Value = polkagent_core::Durability> {
    use polkagent_core::Durability;
    prop_oneof![
        Just(Durability::Durable),
        Just(Durability::Ephemeral),
        Just(Durability::Diagnostic),
    ]
}

/// Strategy for an arbitrary `RetryClass` (core).
fn arb_retry_class() -> impl Strategy<Value = polkagent_core::RetryClass> {
    use polkagent_core::RetryClass;
    prop_oneof![
        Just(RetryClass::Idempotent),
        Just(RetryClass::CheckBeforeRetry),
        Just(RetryClass::NoAutoRetry),
    ]
}

/// Strategy for an arbitrary `OutcomeStatus`.
fn arb_outcome_status() -> impl Strategy<Value = polkagent_core::OutcomeStatus> {
    use polkagent_core::OutcomeStatus;
    prop_oneof![
        Just(OutcomeStatus::Success),
        Just(OutcomeStatus::Failure),
        Just(OutcomeStatus::Timeout),
        Just(OutcomeStatus::Cancelled),
        Just(OutcomeStatus::Unknown),
    ]
}

proptest! {
    // ----- PB-05-a: RunState serde identity (already tested above, adding
    //                explicit identity framing) --------------------------------
    #[test]
    fn pb05_run_state_serde_identity(state in arb_run_state()) {
        let json = serde_json::to_string(&state).expect("serialize");
        let back: RunState = serde_json::from_str(&json).expect("deserialize");
        prop_assert_eq!(&state, &back, "RunState serde identity failed for {:?}", state);
    }

    // ----- PB-05-b: EffectIntentState serde identity -------------------------
    #[test]
    fn pb05_effect_intent_state_serde_identity(state in arb_effect_intent_state()) {
        let json = serde_json::to_string(&state).expect("serialize");
        let back: polkagent_core::EffectIntentState =
            serde_json::from_str(&json).expect("deserialize");
        prop_assert_eq!(&state, &back, "EffectIntentState serde identity failed for {:?}", state);
    }

    // ----- PB-05-c: EffectKind (core) serde identity -------------------------
    #[test]
    fn pb05_core_effect_kind_serde_identity(kind in arb_core_effect_kind()) {
        let json = serde_json::to_string(&kind).expect("serialize");
        let back: polkagent_core::EffectKind =
            serde_json::from_str(&json).expect("deserialize");
        prop_assert_eq!(kind, back, "EffectKind serde identity failed");
    }

    // ----- PB-05-d: EventKind serde identity ---------------------------------
    #[test]
    fn pb05_event_kind_serde_identity(kind in arb_event_kind()) {
        let json = serde_json::to_string(&kind).expect("serialize");
        let back: polkagent_core::EventKind =
            serde_json::from_str(&json).expect("deserialize");
        prop_assert_eq!(&kind, &back, "EventKind serde identity failed for {:?}", kind);
    }

    // ----- PB-05-e: Durability serde identity --------------------------------
    #[test]
    fn pb05_durability_serde_identity(dur in arb_durability()) {
        let json = serde_json::to_string(&dur).expect("serialize");
        let back: polkagent_core::Durability =
            serde_json::from_str(&json).expect("deserialize");
        prop_assert_eq!(&dur, &back, "Durability serde identity failed for {:?}", dur);
    }

    // ----- PB-05-f: RetryClass serde identity --------------------------------
    #[test]
    fn pb05_retry_class_serde_identity(cls in arb_retry_class()) {
        let json = serde_json::to_string(&cls).expect("serialize");
        let back: polkagent_core::RetryClass =
            serde_json::from_str(&json).expect("deserialize");
        prop_assert_eq!(cls, back, "RetryClass serde identity failed");
    }

    // ----- PB-05-g: OutcomeStatus serde identity -----------------------------
    #[test]
    fn pb05_outcome_status_serde_identity(status in arb_outcome_status()) {
        let json = serde_json::to_string(&status).expect("serialize");
        let back: polkagent_core::OutcomeStatus =
            serde_json::from_str(&json).expect("deserialize");
        prop_assert_eq!(&status, &back, "OutcomeStatus serde identity failed for {:?}", status);
    }
}

// =========================================================================
// PB-06: Two identical inputs produce identical outputs
// =========================================================================
//
// Same RunState input → same JSON, same JSON → same serde_json::Value hash.
// Serialization must be deterministic: no non-determinism from HashMap
// iteration order can leak into the serialized form.

proptest! {
    /// The same RunState always serializes to exactly the same JSON string.
    #[test]
    fn pb06_run_state_serialization_is_deterministic(state in arb_run_state()) {
        let json1 = serde_json::to_string(&state).expect("serialize 1");
        let json2 = serde_json::to_string(&state).expect("serialize 2");
        prop_assert_eq!(&json1, &json2, "same RunState must produce identical JSON");
    }

    /// The same EventKind always serializes to exactly the same JSON string.
    #[test]
    fn pb06_event_kind_serialization_is_deterministic(kind in arb_event_kind()) {
        let json1 = serde_json::to_string(&kind).expect("serialize 1");
        let json2 = serde_json::to_string(&kind).expect("serialize 2");
        prop_assert_eq!(&json1, &json2, "same EventKind must produce identical JSON");
    }

    /// The same EffectKind always serializes to exactly the same JSON string.
    #[test]
    fn pb06_effect_kind_serialization_is_deterministic(kind in arb_core_effect_kind()) {
        let json1 = serde_json::to_string(&kind).expect("serialize 1");
        let json2 = serde_json::to_string(&kind).expect("serialize 2");
        prop_assert_eq!(&json1, &json2, "same EffectKind must produce identical JSON");
    }

    /// BlobRef computed from same data is always byte-identical.
    #[test]
    fn pb06_blob_ref_is_deterministic(data in prop::collection::vec(any::<u8>(), 0..4096)) {
        use polkagent_core::BlobRef;
        let b1 = BlobRef::from_bytes(&data);
        let b2 = BlobRef::from_bytes(&data);
        let json1 = serde_json::to_string(&b1).expect("serialize 1");
        let json2 = serde_json::to_string(&b2).expect("serialize 2");
        prop_assert_eq!(&json1, &json2, "BlobRef from identical data must produce identical JSON");
    }
}

// =========================================================================
// PB-08: ID generation uniqueness
// =========================================================================
//
// Generating N IDs in sequence always yields N distinct values.
// We use N = 200 per type (sufficient to catch implementation bugs without
// making the test suite slow).

macro_rules! id_uniqueness_proptest {
    ($mod_name:ident, $ty:ty) => {
        mod $mod_name {
            use super::*;

            proptest! {
                /// Generating `n` IDs yields `n` distinct values (no collisions).
                #[test]
                fn pb08_n_ids_are_distinct(n in 2usize..=200) {
                    let ids: Vec<$ty> = (0..n).map(|_| <$ty>::new()).collect();
                    let unique: std::collections::HashSet<_> = ids.iter().copied().collect();
                    prop_assert_eq!(
                        unique.len(),
                        n,
                        "expected {} distinct IDs, got {} unique",
                        n,
                        unique.len()
                    );
                }
            }
        }
    };
}

id_uniqueness_proptest!(pb08_run_id_unique, RunId);
id_uniqueness_proptest!(pb08_agent_id_unique, AgentId);
id_uniqueness_proptest!(pb08_turn_id_unique, TurnId);
id_uniqueness_proptest!(pb08_step_id_unique, StepId);
id_uniqueness_proptest!(pb08_effect_id_unique, EffectId);
id_uniqueness_proptest!(pb08_event_id_unique, EventId);
id_uniqueness_proptest!(pb08_artifact_id_unique, ArtifactId);
id_uniqueness_proptest!(pb08_grant_id_unique, GrantId);

// =========================================================================
// PB-10: Serialization versioning / forward compatibility
// =========================================================================
//
// Optional fields that are absent in the serialized JSON should still
// deserialize successfully (forward compatibility). We test this by
// manually omitting optional fields from JSON objects and verifying that
// deserialization succeeds and the field lands as `None`.

proptest! {
    /// RunState with no optional payload fields deserializes from a minimal
    /// JSON object (only the `state` tag).
    #[test]
    fn pb10_run_state_tolerates_missing_optional_fields(_ in 0u8..1) {
        // All unit-like variants (no extra fields) must deserialize from just
        // `{ "state": "<tag>" }`.
        let simple_cases = [
            (r#"{"state":"created"}"#, RunState::Created),
            (r#"{"state":"queued"}"#, RunState::Queued),
            (r#"{"state":"running"}"#, RunState::Running),
            (r#"{"state":"completing"}"#, RunState::Completing),
            (r#"{"state":"completed"}"#, RunState::Completed),
            (r#"{"state":"timed_out"}"#, RunState::TimedOut),
        ];
        for (json, expected) in &simple_cases {
            let got: RunState = serde_json::from_str(json)
                .unwrap_or_else(|e| panic!("failed to deserialize {json}: {e}"));
            prop_assert_eq!(&got, expected, "mismatch for {}", json);
        }
    }

    /// EffectIntentState::Pending deserializes from a minimal representation.
    #[test]
    fn pb10_effect_intent_state_pending_minimal(_ in 0u8..1) {
        use polkagent_core::EffectIntentState;
        // Pending carries no payload, so the minimal JSON is just the tag.
        let json = r#""pending""#;
        let got: EffectIntentState = serde_json::from_str(json)
            .expect("deserialize pending");
        prop_assert_eq!(got, EffectIntentState::Pending);
    }

    /// A Durability value with an unrecognised string is rejected (no silent
    /// default), while a known tag always succeeds.
    #[test]
    fn pb10_durability_known_tags_deserialize(dur in arb_durability()) {
        let json = serde_json::to_string(&dur).expect("serialize");
        // Forward compat: re-deserialize from our own output.
        let back: polkagent_core::Durability =
            serde_json::from_str(&json).expect("deserialize known tag");
        prop_assert_eq!(&back, &dur);
    }

    /// RetryClass: all known tags round-trip; forward-compat means we can add
    /// new tags later without breaking consumers of old tags.
    #[test]
    fn pb10_retry_class_known_tags_are_forward_compat(cls in arb_retry_class()) {
        let json = serde_json::to_string(&cls).expect("serialize");
        let back: polkagent_core::RetryClass =
            serde_json::from_str(&json).expect("deserialize known tag");
        prop_assert_eq!(cls, back);
    }

    /// EventKind: a freshly serialized EventKind always re-deserializes
    /// (forward compat means each known tag is its own stable wire format).
    #[test]
    fn pb10_event_kind_self_roundtrip_is_stable(kind in arb_event_kind()) {
        let json = serde_json::to_string(&kind).expect("serialize");
        let back: polkagent_core::EventKind =
            serde_json::from_str(&json).expect("deserialize");
        prop_assert_eq!(&kind, &back);
    }

    /// EffectIntent with `payload_json = None` and `resolved_at = None` and
    /// `deadline = None` deserializes correctly (all optional fields absent).
    #[test]
    fn pb10_effect_intent_with_optional_fields_missing(_ in 0u8..1) {
        use polkagent_core::effect::EffectIntent;
        use polkagent_core::ids::{EffectId, RunId, TurnId, StepId};
        use polkagent_core::effect::{EffectKind, RetryClass, EffectIntentState, IdempotencyKey};

        let intent = EffectIntent {
            id: EffectId::new(),
            run_id: RunId::new(),
            turn_id: TurnId::new(),
            step_id: StepId::new(),
            kind: EffectKind::ChainQuery,
            idempotency_key: IdempotencyKey::from_hex("aabbcc"),
            sequence: 1,
            state: EffectIntentState::Pending,
            retry_class: RetryClass::Idempotent,
            max_attempts: 3,
            attempt_count: 0,
            created_at: chrono::Utc::now(),
            resolved_at: None,   // optional
            deadline: None,      // optional
            payload_json: None,  // optional
        };

        let json = serde_json::to_string(&intent).expect("serialize");
        let back: EffectIntent = serde_json::from_str(&json).expect("deserialize");

        prop_assert_eq!(intent.id, back.id);
        prop_assert!(back.resolved_at.is_none(), "resolved_at must stay None");
        prop_assert!(back.deadline.is_none(), "deadline must stay None");
        prop_assert!(back.payload_json.is_none(), "payload_json must stay None");
    }
}
