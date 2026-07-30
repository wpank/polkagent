//! Property-based tests for `polkagent-effect`.
//!
//! Uses `proptest` to validate algebraic invariants of [`IdempotencyKey`],
//! [`EffectKind`], and [`EffectPriority`] that must hold for *all* inputs,
//! not just hand-picked samples.

use polkagent_core::RunId;
use polkagent_effect::idempotency::{self, IdempotencyKey};
use polkagent_effect::types::{EffectKind, EffectPriority};
use proptest::prelude::*;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Strategies
// ---------------------------------------------------------------------------

/// Arbitrary `RunId` from 16 random bytes.
fn arb_run_id() -> impl Strategy<Value = RunId> {
    prop::array::uniform16(any::<u8>()).prop_map(|bytes| RunId::from_uuid(Uuid::from_bytes(bytes)))
}

/// Arbitrary `EffectKind` — one of the nine variants.
fn arb_effect_kind() -> impl Strategy<Value = EffectKind> {
    prop_oneof![
        Just(EffectKind::ModelCall),
        Just(EffectKind::ToolCall),
        Just(EffectKind::SignatureRequest),
        Just(EffectKind::Broadcast),
        Just(EffectKind::FinalityWatch),
        Just(EffectKind::Delivery),
        Just(EffectKind::ChainRead),
        Just(EffectKind::Simulation),
        Just(EffectKind::HarnessOperation),
    ]
}

/// Arbitrary 32-byte params hash.
fn arb_params_hash() -> impl Strategy<Value = [u8; 32]> {
    prop::array::uniform32(any::<u8>())
}

/// Arbitrary `EffectPriority`.
fn arb_priority() -> impl Strategy<Value = EffectPriority> {
    prop_oneof![
        Just(EffectPriority::Low),
        Just(EffectPriority::Normal),
        Just(EffectPriority::High),
        Just(EffectPriority::Critical),
    ]
}

// ---------------------------------------------------------------------------
// 1. IdempotencyKey properties
// ---------------------------------------------------------------------------

proptest! {
    /// Same inputs always produce the same key (deterministic).
    #[test]
    fn idempotency_key_is_deterministic(
        run_id in arb_run_id(),
        turn_seq in any::<u64>(),
        step_idx in any::<u64>(),
        kind in arb_effect_kind(),
        params_hash in arb_params_hash(),
    ) {
        let k1 = IdempotencyKey::generate(run_id, turn_seq, step_idx, kind, params_hash);
        let k2 = IdempotencyKey::generate(run_id, turn_seq, step_idx, kind, params_hash);
        prop_assert_eq!(k1, k2, "same inputs must produce same key");
    }

    /// Different run_ids produce different keys (with overwhelming probability;
    /// a collision here would be a BLAKE3 collision on distinct 16-byte prefixes).
    #[test]
    fn different_run_ids_produce_different_keys(
        run_a in arb_run_id(),
        run_b in arb_run_id(),
        turn_seq in any::<u64>(),
        step_idx in any::<u64>(),
        kind in arb_effect_kind(),
        params_hash in arb_params_hash(),
    ) {
        prop_assume!(run_a != run_b);
        let ka = IdempotencyKey::generate(run_a, turn_seq, step_idx, kind, params_hash);
        let kb = IdempotencyKey::generate(run_b, turn_seq, step_idx, kind, params_hash);
        prop_assert_ne!(ka, kb, "different run_ids must produce different keys");
    }

    /// Different turn sequences produce different keys.
    #[test]
    fn different_turn_sequences_produce_different_keys(
        run_id in arb_run_id(),
        turn_a in any::<u64>(),
        turn_b in any::<u64>(),
        step_idx in any::<u64>(),
        kind in arb_effect_kind(),
        params_hash in arb_params_hash(),
    ) {
        prop_assume!(turn_a != turn_b);
        let ka = IdempotencyKey::generate(run_id, turn_a, step_idx, kind, params_hash);
        let kb = IdempotencyKey::generate(run_id, turn_b, step_idx, kind, params_hash);
        prop_assert_ne!(ka, kb, "different turn sequences must produce different keys");
    }

    /// Different step indices produce different keys.
    #[test]
    fn different_step_indices_produce_different_keys(
        run_id in arb_run_id(),
        turn_seq in any::<u64>(),
        step_a in any::<u64>(),
        step_b in any::<u64>(),
        kind in arb_effect_kind(),
        params_hash in arb_params_hash(),
    ) {
        prop_assume!(step_a != step_b);
        let ka = IdempotencyKey::generate(run_id, turn_seq, step_a, kind, params_hash);
        let kb = IdempotencyKey::generate(run_id, turn_seq, step_b, kind, params_hash);
        prop_assert_ne!(ka, kb, "different step indices must produce different keys");
    }

    /// Different effect kinds produce different keys.
    #[test]
    fn different_effect_kinds_produce_different_keys(
        run_id in arb_run_id(),
        turn_seq in any::<u64>(),
        step_idx in any::<u64>(),
        kind_a in arb_effect_kind(),
        kind_b in arb_effect_kind(),
        params_hash in arb_params_hash(),
    ) {
        prop_assume!(kind_a != kind_b);
        let ka = IdempotencyKey::generate(run_id, turn_seq, step_idx, kind_a, params_hash);
        let kb = IdempotencyKey::generate(run_id, turn_seq, step_idx, kind_b, params_hash);
        prop_assert_ne!(ka, kb, "different effect kinds must produce different keys");
    }

    /// IdempotencyKey round-trips through hex string: `to_hex` then `parse_from_hex`.
    #[test]
    fn idempotency_key_hex_round_trip(
        run_id in arb_run_id(),
        turn_seq in any::<u64>(),
        step_idx in any::<u64>(),
        kind in arb_effect_kind(),
        params_hash in arb_params_hash(),
    ) {
        let key = IdempotencyKey::generate(run_id, turn_seq, step_idx, kind, params_hash);
        let hex_str = key.to_hex();

        // Hex string must be exactly 64 characters (32 bytes).
        prop_assert_eq!(hex_str.len(), 64, "hex encoding must be 64 chars");

        // Round-trip through parse.
        let parsed = idempotency::parse_from_hex(&hex_str);
        prop_assert!(parsed.is_some(), "parse_from_hex must succeed for valid hex");
        prop_assert_eq!(parsed.expect("checked above"), key, "round-trip must preserve key");
    }
}

// ---------------------------------------------------------------------------
// 2. EffectKind properties
// ---------------------------------------------------------------------------

proptest! {
    /// Every EffectKind has a non-empty discriminant string.
    #[test]
    fn effect_kind_discriminant_is_non_empty(kind in arb_effect_kind()) {
        let disc = kind.discriminant_str();
        prop_assert!(!disc.is_empty(), "discriminant_str must be non-empty");
    }

    /// Every EffectKind has a positive default lease duration.
    #[test]
    fn effect_kind_lease_duration_is_positive(kind in arb_effect_kind()) {
        let dur = kind.default_lease_duration();
        prop_assert!(dur.as_secs() > 0, "default lease duration must be positive");
    }

    /// Every EffectKind has a valid retry class (the match is exhaustive, so
    /// this mainly asserts that the method does not panic).
    #[test]
    fn effect_kind_retry_class_does_not_panic(kind in arb_effect_kind()) {
        let _class = kind.default_retry_class();
        // If we get here the match was exhaustive and did not panic.
    }
}

// ---------------------------------------------------------------------------
// 3. EffectPriority properties
// ---------------------------------------------------------------------------

proptest! {
    /// Priority ordering is total: for any two priorities, exactly one of
    /// `a < b`, `a == b`, or `a > b` holds.
    #[test]
    fn priority_ordering_is_total(
        a in arb_priority(),
        b in arb_priority(),
    ) {
        let cmp = a.cmp(&b);
        prop_assert!(
            matches!(cmp, std::cmp::Ordering::Less | std::cmp::Ordering::Equal | std::cmp::Ordering::Greater),
            "cmp must return a valid Ordering"
        );
        // Verify consistency with PartialOrd.
        prop_assert_eq!(a.partial_cmp(&b), Some(cmp), "PartialOrd must agree with Ord");
    }

    /// The fixed ordering Low < Normal < High < Critical holds.
    #[test]
    fn priority_low_lt_normal_lt_high_lt_critical(_dummy in 0..1u8) {
        prop_assert!(EffectPriority::Low < EffectPriority::Normal);
        prop_assert!(EffectPriority::Normal < EffectPriority::High);
        prop_assert!(EffectPriority::High < EffectPriority::Critical);
        // Transitivity sanity check.
        prop_assert!(EffectPriority::Low < EffectPriority::Critical);
    }
}
