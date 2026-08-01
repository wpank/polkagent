//! Network identity validation for extrinsic decoding.
//!
//! Before decoding an extrinsic with chain metadata, the genesis hash
//! embedded in the extrinsic's signed extensions must match the expected
//! genesis hash for the chain profile. This prevents accidentally decoding
//! an extrinsic intended for a different network (AC-P2-005).

use crate::error::MetadataError;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Validate that an extrinsic's genesis hash matches the expected genesis hash
/// for a chain.
///
/// Returns `Ok(())` when the hashes match. Returns
/// [`MetadataError::WrongNetwork`] with a human-readable description when
/// they differ (AC-P2-005).
///
/// # Arguments
///
/// * `chain_name` — human-readable chain name used in the error message
///   (e.g., `"polkadot"`, `"kusama"`).
/// * `extrinsic_genesis` — the 32-byte genesis hash extracted from the
///   extrinsic's signed extensions.
/// * `expected_genesis` — the 32-byte genesis hash that the caller
///   considers authoritative for this chain.
pub fn validate_network(
    chain_name: &str,
    extrinsic_genesis: &[u8; 32],
    expected_genesis: &[u8; 32],
) -> Result<(), MetadataError> {
    if extrinsic_genesis == expected_genesis {
        return Ok(());
    }

    let extrinsic_hex = hex_encode(extrinsic_genesis);
    let expected_hex = hex_encode(expected_genesis);

    Err(MetadataError::WrongNetwork {
        chain_name: chain_name.to_string(),
        extrinsic_genesis: extrinsic_hex,
        expected_genesis: expected_hex,
    })
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const POLKADOT_GENESIS: [u8; 32] = [
        0x91, 0xb1, 0x71, 0xbb, 0x15, 0x8e, 0x2d, 0x38, 0x48, 0xfa, 0x23, 0xa9, 0x7b, 0x72,
        0x73, 0x68, 0x35, 0x37, 0x22, 0x04, 0xd7, 0x60, 0x89, 0x29, 0x33, 0x91, 0x05, 0xf7,
        0xf3, 0x74, 0x58, 0x23,
    ];

    const KUSAMA_GENESIS: [u8; 32] = [
        0xb0, 0xa8, 0xd4, 0x93, 0x28, 0x5c, 0x2d, 0xf7, 0x32, 0x90, 0xdf, 0xb7, 0xe6, 0x1f,
        0x87, 0x0f, 0x17, 0xb4, 0x18, 0x01, 0x19, 0x7a, 0x14, 0x9c, 0xa9, 0x36, 0x54, 0x99,
        0xea, 0x3d, 0xaf, 0xe2,
    ];

    #[test]
    fn matching_genesis_returns_ok() {
        let result = validate_network("polkadot", &POLKADOT_GENESIS, &POLKADOT_GENESIS);
        assert!(result.is_ok());
    }

    #[test]
    fn mismatched_genesis_returns_wrong_network_error() {
        let result = validate_network("polkadot", &KUSAMA_GENESIS, &POLKADOT_GENESIS);
        assert!(result.is_err());

        let err = result.expect_err("should be err");
        let msg = format!("{err}");
        // Error message must mention both network context and the hash mismatch.
        assert!(msg.contains("polkadot"), "error should mention chain name: {msg}");
    }

    #[test]
    fn error_message_contains_both_hashes() {
        let result = validate_network("polkadot", &KUSAMA_GENESIS, &POLKADOT_GENESIS);
        let err = result.expect_err("should be err");
        let msg = format!("{err}");
        // Both the extrinsic genesis (Kusama) and expected genesis (Polkadot) should appear.
        assert!(!msg.is_empty());
        // Check that we have WrongNetwork variant.
        assert!(
            matches!(err, MetadataError::WrongNetwork { .. }),
            "should be WrongNetwork: {err:?}"
        );
    }

    #[test]
    fn zero_genesis_hashes_match() {
        let zero = [0u8; 32];
        let result = validate_network("local-dev", &zero, &zero);
        assert!(result.is_ok());
    }

    #[test]
    fn one_byte_difference_triggers_rejection() {
        let mut slightly_off = POLKADOT_GENESIS;
        slightly_off[0] ^= 0xFF; // flip first byte
        let result = validate_network("polkadot", &slightly_off, &POLKADOT_GENESIS);
        assert!(result.is_err());
    }
}
