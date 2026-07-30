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
        prop::collection::vec(Just(()).prop_map(|()| EffectId::new()), 0..5)
            .prop_map(|ids| RunState::WaitingEffect {
                pending_intent_ids: ids,
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

    /// Accumulation never panics or overflows — it uses saturating
    /// arithmetic, so the result is always <= u32::MAX.
    #[test]
    fn token_usage_accumulate_no_overflow(
        a in arb_token_usage(),
        b in arb_token_usage(),
    ) {
        let mut result = a;
        result.accumulate(&b);

        prop_assert!(result.input_tokens       <= u32::MAX);
        prop_assert!(result.output_tokens      <= u32::MAX);
        prop_assert!(result.total_tokens       <= u32::MAX);
        prop_assert!(result.cache_read_tokens  <= u32::MAX);
        prop_assert!(result.cache_write_tokens <= u32::MAX);
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
