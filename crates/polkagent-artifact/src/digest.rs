//! BLAKE3-based and SHA-256 content digest utilities for artifact integrity.
//!
//! These utilities operate on [`polkagent_core::artifact::BlobRef`], the
//! canonical content-address type used throughout the artifact subsystem.
//! `BlobRef` stores the BLAKE3 digest as a lowercase hex string, which is
//! human-readable in logs and JSON.
//!
//! # Dual-digest design
//!
//! BLAKE3 (`blake3_hex`) is the fast internal digest used as the primary
//! storage key and integrity check. SHA-256 (`sha256_hex`) is the canonical
//! external digest for interoperability with Sigstore, IPFS, and on-chain
//! anchoring systems. Both digests are computed at artifact creation time.
//!
//! # Relationship to `polkagent_core`
//!
//! [`polkagent_core::artifact::BlobRef`] defines the storage format including
//! the optional `sha256_hex` field.  This module provides the computation and
//! verification helpers that the service layer uses.

use sha2::Digest as Sha2Digest;

use polkagent_core::artifact::BlobRef;

// ---------------------------------------------------------------------------
// Type aliases
// ---------------------------------------------------------------------------

/// A BLAKE3 digest represented as a lowercase 64-character hex string.
pub type Blake3Digest = String;

/// A SHA-256 digest represented as a lowercase 64-character hex string.
pub type Sha256Digest = String;

// ---------------------------------------------------------------------------
// Core functions
// ---------------------------------------------------------------------------

/// Compute a BLAKE3 digest over `data` and return a [`BlobRef`].
///
/// This is the canonical entry point for digest computation in the artifact
/// subsystem.  The returned `BlobRef` carries the hex digest and the byte
/// size of `data`.  The `sha256_hex` field is **not** populated here; use
/// [`compute_dual_digest`] to populate both digests simultaneously.
///
/// ```
/// use polkagent_artifact::digest::compute_digest;
///
/// let digest = compute_digest(b"hello");
/// assert_eq!(digest.size_bytes, 5);
/// assert_eq!(digest.blake3_hex.len(), 64);
/// assert!(digest.sha256_hex.is_none());
/// ```
#[must_use]
pub fn compute_digest(data: &[u8]) -> BlobRef {
    BlobRef::from_bytes(data)
}

/// Compute a SHA-256 digest over `data` and return a lowercase hex string.
///
/// SHA-256 is the canonical external digest for interoperability with
/// Sigstore, IPFS CIDv0, and on-chain anchoring.
///
/// ```
/// use polkagent_artifact::digest::compute_sha256_digest;
///
/// let hex = compute_sha256_digest(b"hello");
/// assert_eq!(hex.len(), 64);
/// assert!(hex.chars().all(|c| c.is_ascii_hexdigit()));
/// ```
#[must_use]
pub fn compute_sha256_digest(data: &[u8]) -> Sha256Digest {
    let hash = sha2::Sha256::digest(data);
    format!("{hash:x}")
}

/// Compute both a BLAKE3 digest and a SHA-256 digest over `data`.
///
/// Returns `(blake3_hex, sha256_hex)` as a tuple of lowercase hex strings.
/// Use this when you need both digests and want to avoid hashing the data
/// twice unnecessarily.
///
/// ```
/// use polkagent_artifact::digest::compute_dual_digest;
///
/// let (blake3, sha256) = compute_dual_digest(b"polkagent");
/// assert_eq!(blake3.len(), 64);
/// assert_eq!(sha256.len(), 64);
/// ```
#[must_use]
pub fn compute_dual_digest(data: &[u8]) -> (Blake3Digest, Sha256Digest) {
    let blake3_hex = blake3::hash(data).to_hex().to_string();
    let sha256_hex = compute_sha256_digest(data);
    (blake3_hex, sha256_hex)
}

/// Return `true` if `data` hashes to the BLAKE3 digest stored in `blob_ref`.
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

/// Return `true` if `data` hashes to the SHA-256 digest stored in `artifact`.
///
/// Returns `false` if `artifact.blob_ref.sha256_hex` is `None` (i.e. no
/// SHA-256 digest was recorded at creation time) or if the hash does not
/// match.
///
/// ```
/// use polkagent_artifact::digest::{compute_sha256_digest, verify_sha256};
/// use polkagent_core::artifact::BlobRef;
///
/// let data = b"canonical external digest";
/// let sha256 = compute_sha256_digest(data);
/// let blob_ref = BlobRef {
///     blake3_hex: "a".repeat(64),
///     sha256_hex: Some(sha256),
///     size_bytes: data.len() as u64,
/// };
/// assert!(verify_sha256(&blob_ref, data));
/// assert!(!verify_sha256(&blob_ref, b"wrong data"));
/// ```
#[must_use]
pub fn verify_sha256(blob_ref: &BlobRef, data: &[u8]) -> bool {
    match &blob_ref.sha256_hex {
        None => false,
        Some(stored) => {
            let computed = compute_sha256_digest(data);
            computed == *stored
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- compute_digest (BLAKE3) ---

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
    fn compute_digest_sha256_field_is_none() {
        let d = compute_digest(b"no sha256 expected");
        assert!(d.sha256_hex.is_none(), "sha256_hex must not be set by compute_digest");
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

    // --- compute_sha256_digest ---

    #[test]
    fn sha256_digest_known_value() {
        // SHA-256("hello") = 2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824
        let hex = compute_sha256_digest(b"hello");
        assert_eq!(hex, "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824");
    }

    #[test]
    fn sha256_digest_is_64_hex_chars() {
        let hex = compute_sha256_digest(b"polkagent sha256");
        assert_eq!(hex.len(), 64);
        assert!(hex.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn sha256_digest_is_deterministic() {
        let data = b"deterministic sha256 input";
        let a = compute_sha256_digest(data);
        let b = compute_sha256_digest(data);
        assert_eq!(a, b);
    }

    #[test]
    fn sha256_digest_differs_for_different_inputs() {
        let a = compute_sha256_digest(b"input a");
        let b = compute_sha256_digest(b"input b");
        assert_ne!(a, b);
    }

    #[test]
    fn sha256_digest_empty_data_known_value() {
        // SHA-256("") = e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
        let hex = compute_sha256_digest(b"");
        assert_eq!(hex, "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    }

    // --- compute_dual_digest ---

    #[test]
    fn dual_digest_returns_correct_lengths() {
        let (blake3, sha256) = compute_dual_digest(b"dual digest test");
        assert_eq!(blake3.len(), 64);
        assert_eq!(sha256.len(), 64);
    }

    #[test]
    fn dual_digest_blake3_matches_standalone() {
        let data = b"cross-check blake3";
        let (blake3, _) = compute_dual_digest(data);
        let standalone = compute_digest(data);
        assert_eq!(blake3, standalone.blake3_hex);
    }

    #[test]
    fn dual_digest_sha256_matches_standalone() {
        let data = b"cross-check sha256";
        let (_, sha256) = compute_dual_digest(data);
        let standalone = compute_sha256_digest(data);
        assert_eq!(sha256, standalone);
    }

    #[test]
    fn dual_digest_is_deterministic() {
        let data = b"reproducible dual";
        let (b1, s1) = compute_dual_digest(data);
        let (b2, s2) = compute_dual_digest(data);
        assert_eq!(b1, b2);
        assert_eq!(s1, s2);
    }

    #[test]
    fn dual_digests_are_different_algorithms() {
        // BLAKE3 and SHA-256 should produce different output for the same input.
        let data = b"algorithm comparison";
        let (blake3, sha256) = compute_dual_digest(data);
        assert_ne!(blake3, sha256, "BLAKE3 and SHA-256 must not produce identical output");
    }

    // --- verify_sha256 ---

    #[test]
    fn verify_sha256_returns_true_for_matching_data() {
        let data = b"verify sha256 ok";
        let sha256 = compute_sha256_digest(data);
        let blob_ref = BlobRef {
            blake3_hex: "a".repeat(64),
            sha256_hex: Some(sha256),
            size_bytes: data.len() as u64,
        };
        assert!(verify_sha256(&blob_ref, data));
    }

    #[test]
    fn verify_sha256_returns_false_for_tampered_data() {
        let data = b"original data";
        let sha256 = compute_sha256_digest(data);
        let blob_ref = BlobRef {
            blake3_hex: "b".repeat(64),
            sha256_hex: Some(sha256),
            size_bytes: data.len() as u64,
        };
        assert!(!verify_sha256(&blob_ref, b"tampered data"));
    }

    #[test]
    fn verify_sha256_returns_false_when_no_sha256_stored() {
        let data = b"no sha256 field";
        let blob_ref = BlobRef {
            blake3_hex: compute_digest(data).blake3_hex,
            sha256_hex: None,
            size_bytes: data.len() as u64,
        };
        assert!(!verify_sha256(&blob_ref, data));
    }

    #[test]
    fn verify_sha256_empty_data() {
        let data = b"";
        let sha256 = compute_sha256_digest(data);
        let blob_ref = BlobRef {
            blake3_hex: "c".repeat(64),
            sha256_hex: Some(sha256),
            size_bytes: 0,
        };
        assert!(verify_sha256(&blob_ref, data));
        assert!(!verify_sha256(&blob_ref, b"not empty"));
    }
}
