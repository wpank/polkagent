//! Idempotency key generation and deduplication for the effect pipeline.
//!
//! Every [`EffectIntent`](crate::types::EffectIntent) carries a stable
//! [`IdempotencyKey`] derived deterministically from the run, sequence, kind,
//! and input parameters. This key enables two safety properties:
//!
//! 1. **Internal deduplication** — if `propose` is called twice with the same
//!    logical inputs, the pipeline returns the existing intent rather than
//!    creating a duplicate.
//! 2. **External idempotency** — workers pass the key to external systems
//!    that support idempotency headers (model APIs, payment rails), ensuring
//!    that retries of the same intent produce the same external result.
//!
//! ## Key generation
//!
//! The key is the BLAKE3 hash of the concatenation of:
//!
//! ```text
//! run_id_bytes (16) || turn_sequence_le (8) || step_index_le (8)
//!     || effect_kind_discriminant_bytes || 0x00 (separator)
//!     || params_hash_bytes (32)
//! ```
//!
//! The separator byte prevents length-extension collisions between the
//! discriminant and the params hash.
//!
//! ## Deduplication rules (PRD-03 §7.2)
//!
//! | Scenario | Behaviour |
//! |---|---|
//! | Same key, `Pending` intent exists | Return `DuplicatePending` error with existing ID |
//! | Same key, `Resolved` intent exists | Return `DuplicateResolved` error with existing ID |
//! | Same key, `Claimed` intent exists | Return `DuplicateClaimed` error with existing ID |
//! | Different key | New intent; no deduplication |

use std::sync::Arc;

use polkagent_core::{EffectId, RunId};
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::error::PipelineError;
use crate::types::EffectKind;
use polkagent_store_trait::EffectStore;

// ---------------------------------------------------------------------------
// IdempotencyKey
// ---------------------------------------------------------------------------

/// A stable, content-addressed key used to deduplicate effect intents and
/// pass idempotency tokens to external systems.
///
/// The key is opaque outside this module. Callers obtain it via
/// [`IdempotencyKey::generate`] and store it on the intent. Display and
/// serialisation expose the hex-encoded BLAKE3 digest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct IdempotencyKey(
    /// 32-byte BLAKE3 digest encoded as hex in the JSON representation.
    #[serde(with = "hex_bytes")]
    [u8; 32],
);

impl IdempotencyKey {
    /// Generate a stable idempotency key for an effect intent.
    ///
    /// # Parameters
    ///
    /// - `run_id` — the run that owns the intent.
    /// - `turn_sequence` — the turn number within the run (1-based).
    /// - `step_index` — the step index within the turn (0-based).
    /// - `effect_kind` — the type of external I/O to perform.
    /// - `params_hash` — a caller-computed 32-byte digest of the
    ///   kind-specific parameters (e.g., BLAKE3 of the canonically
    ///   serialised input payload).
    ///
    /// # Stability
    ///
    /// Given the same inputs, this function always returns the same key. The
    /// hash function (BLAKE3) and the byte layout are part of the public
    /// contract. Any change would break deduplication across restarts and must
    /// be treated as a breaking API change.
    #[must_use]
    pub fn generate(
        run_id: RunId,
        turn_sequence: u64,
        step_index: u64,
        effect_kind: EffectKind,
        params_hash: [u8; 32],
    ) -> Self {
        let mut hasher = blake3::Hasher::new();
        hasher.update(run_id.as_bytes());
        hasher.update(&turn_sequence.to_le_bytes());
        hasher.update(&step_index.to_le_bytes());
        hasher.update(effect_kind.discriminant_str().as_bytes());
        // Separator byte: prevents length-extension collisions between the
        // variable-length discriminant and the fixed-length params hash.
        hasher.update(&[0x00u8]);
        hasher.update(&params_hash);
        let digest = hasher.finalize();
        Self(*digest.as_bytes())
    }

    /// Compute a params hash from an arbitrary byte slice using BLAKE3.
    ///
    /// Callers that have already serialised their kind-specific parameters to
    /// bytes can use this helper to obtain the `params_hash` argument for
    /// [`generate`](IdempotencyKey::generate).
    #[must_use]
    pub fn hash_params(params_bytes: &[u8]) -> [u8; 32] {
        let digest = blake3::hash(params_bytes);
        *digest.as_bytes()
    }

    /// Return the raw 32-byte digest.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Return the idempotency key as a hex string, suitable for use as an
    /// HTTP idempotency header value.
    #[must_use]
    pub fn to_hex(&self) -> String {
        hex_encode(&self.0)
    }
}

impl std::fmt::Display for IdempotencyKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_hex())
    }
}

// ---------------------------------------------------------------------------
// Deduplication
// ---------------------------------------------------------------------------

/// Result of a deduplication check.
#[derive(Debug)]
pub enum DeduplicationResult {
    /// No existing intent with this key; the caller should create a new one.
    New,
    /// An intent with this key already exists in the given state.
    /// The caller should handle it per PRD-03 §7.2.
    Exists {
        intent_id: EffectId,
        state: ExistingIntentState,
    },
}

/// The state of an existing intent found during deduplication.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExistingIntentState {
    Pending,
    Claimed,
    Resolved,
}

/// Check for an existing intent with `key` in the store.
///
/// This is called by [`crate::pipeline::EffectPipeline::propose`] before
/// persisting a new intent, implementing the deduplication rules from
/// PRD-03 §7.2.
///
/// Returns [`PipelineError::DuplicatePending`],
/// [`PipelineError::DuplicateClaimed`], or
/// [`PipelineError::DuplicateResolved`] if a matching intent is found.
pub async fn check_duplicate(
    store: &Arc<dyn EffectStore>,
    key: &IdempotencyKey,
    run_id: RunId,
) -> Result<(), PipelineError> {
    let key_hex = key.to_hex();

    // Use the indexed lookup when available (O(1) in SQL-backed stores).
    // The default trait implementation falls back to get_by_run + linear scan.
    let maybe_stored = store
        .get_by_idempotency_key(&key_hex, run_id)
        .await
        .map_err(PipelineError::Store)?;

    if let Some(stored) = maybe_stored {
        let intent_id = stored.id;
        let state_tag = stored.state.as_str();
        debug!(
            intent_id = %intent_id,
            state = state_tag,
            "deduplication: found existing intent with matching key"
        );
        return match state_tag {
            "pending" => Err(PipelineError::DuplicatePending(intent_id)),
            "claimed" | "executing" => Err(PipelineError::DuplicateClaimed(intent_id)),
            "resolved" => Err(PipelineError::DuplicateResolved(intent_id)),
            "permanently_failed" | "failed" => Err(PipelineError::DuplicateResolved(intent_id)),
            other => Err(PipelineError::Internal(format!(
                "deduplication: intent {} has unexpected state '{}'; \
                 treating as duplicate to prevent double-execution",
                intent_id, other,
            ))),
        };
    }

    Ok(())
}

/// Parse an [`IdempotencyKey`] from a hex string.
///
/// Returns `None` if the string is not valid 64-character hex.
pub fn parse_from_hex(hex_str: &str) -> Option<IdempotencyKey> {
    let bytes = hex_decode(hex_str).ok()?;
    if bytes.len() != 32 {
        return None;
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes);
    Some(IdempotencyKey(arr))
}

/// Extract an idempotency key from a stored intent payload JSON blob.
///
/// Falls back to reading the `"idempotency_key"` field inside the JSON
/// payload (used only in tests / legacy paths; the canonical path uses the
/// `StoredIntent.idempotency_key` field directly).
#[allow(dead_code)]
pub(crate) fn extract_idempotency_key_from_payload(
    payload: &serde_json::Value,
) -> Option<IdempotencyKey> {
    let hex_str = payload.get("idempotency_key")?.as_str()?;
    parse_from_hex(hex_str)
}

// ---------------------------------------------------------------------------
// Hex helpers (internal, to avoid a dep on `hex` crate)
// ---------------------------------------------------------------------------

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0xf) as usize] as char);
    }
    out
}

fn hex_decode(s: &str) -> Result<Vec<u8>, ()> {
    if !s.len().is_multiple_of(2) {
        return Err(());
    }
    let mut out = Vec::with_capacity(s.len() / 2);
    let s = s.as_bytes();
    for pair in s.chunks(2) {
        let hi = hex_nibble(pair[0])?;
        let lo = hex_nibble(pair[1])?;
        out.push((hi << 4) | lo);
    }
    Ok(out)
}

fn hex_nibble(b: u8) -> Result<u8, ()> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        _ => Err(()),
    }
}

/// Custom serde module for the fixed-size byte array stored as hex.
mod hex_bytes {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8; 32], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&super::hex_encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<[u8; 32], D::Error> {
        let hex_str = String::deserialize(d)?;
        let bytes = super::hex_decode(&hex_str)
            .map_err(|()| serde::de::Error::custom("invalid hex string for IdempotencyKey"))?;
        if bytes.len() != 32 {
            return Err(serde::de::Error::custom(
                "IdempotencyKey must be exactly 32 bytes (64 hex chars)",
            ));
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        Ok(arr)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use polkagent_core::RunId;

    fn sample_run_id() -> RunId {
        RunId::new()
    }

    fn sample_params_hash() -> [u8; 32] {
        IdempotencyKey::hash_params(b"some-canonical-input")
    }

    #[test]
    fn same_inputs_produce_same_key() {
        let run_id = sample_run_id();
        let params_hash = sample_params_hash();

        let k1 = IdempotencyKey::generate(run_id, 1, 0, EffectKind::ModelCall, params_hash);
        let k2 = IdempotencyKey::generate(run_id, 1, 0, EffectKind::ModelCall, params_hash);

        assert_eq!(k1, k2, "same inputs must produce same key");
    }

    #[test]
    fn different_run_ids_produce_different_keys() {
        let run_a = RunId::new();
        let run_b = RunId::new();
        let params_hash = sample_params_hash();

        let k_a = IdempotencyKey::generate(run_a, 1, 0, EffectKind::ModelCall, params_hash);
        let k_b = IdempotencyKey::generate(run_b, 1, 0, EffectKind::ModelCall, params_hash);

        assert_ne!(k_a, k_b, "different run IDs must produce different keys");
    }

    #[test]
    fn different_turn_sequences_produce_different_keys() {
        let run_id = sample_run_id();
        let params_hash = sample_params_hash();

        let k1 = IdempotencyKey::generate(run_id, 1, 0, EffectKind::ChainRead, params_hash);
        let k2 = IdempotencyKey::generate(run_id, 2, 0, EffectKind::ChainRead, params_hash);

        assert_ne!(k1, k2);
    }

    #[test]
    fn different_step_indices_produce_different_keys() {
        let run_id = sample_run_id();
        let params_hash = sample_params_hash();

        let k0 = IdempotencyKey::generate(run_id, 1, 0, EffectKind::ToolCall, params_hash);
        let k1 = IdempotencyKey::generate(run_id, 1, 1, EffectKind::ToolCall, params_hash);

        assert_ne!(k0, k1);
    }

    #[test]
    fn different_effect_kinds_produce_different_keys() {
        let run_id = sample_run_id();
        let params_hash = sample_params_hash();

        let k_model = IdempotencyKey::generate(run_id, 1, 0, EffectKind::ModelCall, params_hash);
        let k_chain = IdempotencyKey::generate(run_id, 1, 0, EffectKind::ChainRead, params_hash);

        assert_ne!(k_model, k_chain);
    }

    #[test]
    fn different_params_produce_different_keys() {
        let run_id = sample_run_id();

        let hash_a = IdempotencyKey::hash_params(b"input-a");
        let hash_b = IdempotencyKey::hash_params(b"input-b");

        let k_a = IdempotencyKey::generate(run_id, 1, 0, EffectKind::Simulation, hash_a);
        let k_b = IdempotencyKey::generate(run_id, 1, 0, EffectKind::Simulation, hash_b);

        assert_ne!(k_a, k_b);
    }

    #[test]
    fn key_as_bytes_is_32_bytes() {
        let key = IdempotencyKey::generate(sample_run_id(), 1, 0, EffectKind::Broadcast, [0u8; 32]);
        assert_eq!(key.as_bytes().len(), 32);
    }

    #[test]
    fn key_to_hex_is_64_chars() {
        let key = IdempotencyKey::generate(sample_run_id(), 1, 0, EffectKind::Delivery, [0u8; 32]);
        assert_eq!(key.to_hex().len(), 64);
    }

    #[test]
    fn key_display_matches_to_hex() {
        let key = IdempotencyKey::generate(
            sample_run_id(),
            1,
            0,
            EffectKind::HarnessOperation,
            [0u8; 32],
        );
        assert_eq!(key.to_string(), key.to_hex());
    }

    #[test]
    fn key_serde_round_trip() {
        let key =
            IdempotencyKey::generate(sample_run_id(), 3, 2, EffectKind::ToolCall, [0xabu8; 32]);
        let json = serde_json::to_string(&key).expect("serialize");
        let back: IdempotencyKey = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(key, back);
    }

    #[test]
    fn hash_params_is_deterministic() {
        let h1 = IdempotencyKey::hash_params(b"deterministic-input");
        let h2 = IdempotencyKey::hash_params(b"deterministic-input");
        assert_eq!(h1, h2);
    }

    #[test]
    fn hash_params_different_inputs_differ() {
        let h1 = IdempotencyKey::hash_params(b"input-x");
        let h2 = IdempotencyKey::hash_params(b"input-y");
        assert_ne!(h1, h2);
    }

    #[test]
    fn extract_idempotency_key_round_trip() {
        let key = IdempotencyKey::generate(sample_run_id(), 1, 0, EffectKind::ModelCall, [0u8; 32]);
        let payload = serde_json::json!({ "idempotency_key": key.to_hex() });
        let extracted = extract_idempotency_key_from_payload(&payload);
        assert_eq!(extracted, Some(key));
    }

    #[test]
    fn extract_idempotency_key_missing_returns_none() {
        let payload = serde_json::json!({ "something_else": "value" });
        assert!(extract_idempotency_key_from_payload(&payload).is_none());
    }

    #[test]
    fn hex_encode_decode_round_trip() {
        let input: [u8; 32] = {
            let mut arr = [0u8; 32];
            for (i, b) in arr.iter_mut().enumerate() {
                *b = i as u8;
            }
            arr
        };
        let encoded = hex_encode(&input);
        let decoded = hex_decode(&encoded).unwrap();
        assert_eq!(&decoded[..], &input[..]);
    }
}
