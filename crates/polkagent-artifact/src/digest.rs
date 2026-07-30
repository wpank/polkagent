//! BLAKE3-based content digest utilities for artifact integrity.
//!
//! These utilities operate on [`polkagent_core::artifact::BlobRef`], the
//! canonical content-address type used throughout the artifact subsystem.
//! `BlobRef` stores the digest as a lowercase hex string, which is human-
//! readable in logs and JSON.
//!
//! # Relationship to `polkagent_core`
//!
//! [`polkagent_core::artifact::BlobRef`] defines the storage format.  This
//! module provides the computation and verification helpers that the service
//! layer uses — `BlobRef::from_bytes` in core does the same thing, but
//! `compute_digest` and `verify_digest` here match the service API and
//! provide a single auditable entry point.

use polkagent_core::artifact::BlobRef;

// ---------------------------------------------------------------------------
// Core functions
// ---------------------------------------------------------------------------

/// Compute a BLAKE3 digest over `data` and return a [`BlobRef`].
///
/// This is the canonical entry point for digest computation in the artifact
/// subsystem.  The returned `BlobRef` carries the hex digest and the byte
/// size of `data`.
///
/// ```
/// use polkagent_artifact::digest::compute_digest;
///
/// let digest = compute_digest(b"hello");
/// assert_eq!(digest.size_bytes, 5);
/// assert_eq!(digest.blake3_hex.len(), 64);
/// ```
#[must_use]
pub fn compute_digest(data: &[u8]) -> BlobRef {
    BlobRef::from_bytes(data)
}

/// Return `true` if `data` hashes to the digest stored in `blob_ref`.
///
/// This delegates to [`BlobRef::verify`], which performs the BLAKE3 hash and
/// compares the hex strings.
///
/// ```
/// use polkagent_artifact::digest::{compute_digest, verify_digest};
///
/// let data = b"artifact body";
/// let digest = compute_digest(data);
/// assert!(verify_digest(data, &digest));
/// assert!(!verify_digest(b"different", &digest));
/// ```
#[must_use]
pub fn verify_digest(data: &[u8], blob_ref: &BlobRef) -> bool {
    blob_ref.verify(data)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compute_digest_deterministic() {
        let data = b"hello, polkagent";
        let a = compute_digest(data);
        let b = compute_digest(data);
        assert_eq!(a.blake3_hex, b.blake3_hex, "digest must be deterministic");
    }

    #[test]
    fn compute_digest_differs_for_different_input() {
        let a = compute_digest(b"foo");
        let b = compute_digest(b"bar");
        assert_ne!(a.blake3_hex, b.blake3_hex);
    }

    #[test]
    fn compute_digest_records_size() {
        let data = b"size test data";
        let d = compute_digest(data);
        assert_eq!(d.size_bytes, data.len() as u64);
    }

    #[test]
    fn verify_digest_correct_data_returns_true() {
        let data = b"artifact body content";
        let digest = compute_digest(data);
        assert!(verify_digest(data, &digest));
    }

    #[test]
    fn verify_digest_tampered_data_returns_false() {
        let data = b"artifact body content";
        let digest = compute_digest(data);
        let tampered = b"artifact body TAMPERED";
        assert!(!verify_digest(tampered, &digest));
    }

    #[test]
    fn verify_digest_empty_body() {
        let digest = compute_digest(b"");
        assert!(verify_digest(b"", &digest));
        assert!(!verify_digest(b"x", &digest));
    }

    #[test]
    fn blob_ref_hex_is_64_chars() {
        let digest = compute_digest(b"hex length check");
        assert_eq!(digest.blake3_hex.len(), 64);
        assert!(digest.blake3_hex.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn blob_ref_serde_round_trip() {
        let digest = compute_digest(b"serde round trip");
        let json = serde_json::to_string(&digest).expect("serialize");
        let back: BlobRef = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(digest, back);
    }

    #[test]
    fn idempotent_digest_same_content_same_ref() {
        let content = b"idempotent content body";
        let d1 = compute_digest(content);
        let d2 = compute_digest(content);
        assert_eq!(d1.blake3_hex, d2.blake3_hex, "same content must produce same BlobRef");
    }
}
