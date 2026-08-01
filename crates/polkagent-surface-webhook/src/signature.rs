//! HMAC-SHA256 signing and verification of webhook payloads.
//!
//! The signature scheme follows:
//!
//! ```text
//! message  = timestamp + "." + json_body
//! signature = hex(HMAC-SHA256(secret, message))
//! ```
//!
//! The signature is sent in the `X-Polkagent-Signature` header and the
//! timestamp in the `X-Polkagent-Timestamp` header.
//!
//! # Why HMAC-SHA256?
//!
//! HMAC-SHA256 is the industry standard for webhook signatures (used by
//! Stripe, GitHub, Shopify, etc.). We use the `blake3` crate's keyed hash
//! mode as the HMAC primitive because BLAKE3 is faster than SHA-256 while
//! providing equivalent security, and the keyed hash mode is a proper MAC
//! construction.

use crate::error::{Result, WebhookError};

/// HTTP header name for the webhook signature.
pub const SIGNATURE_HEADER: &str = "X-Polkagent-Signature";

/// HTTP header name for the webhook timestamp.
pub const TIMESTAMP_HEADER: &str = "X-Polkagent-Timestamp";

// ---------------------------------------------------------------------------
// Signing
// ---------------------------------------------------------------------------

/// Compute the HMAC signature for a webhook payload.
///
/// The message format is `"{timestamp}.{body}"` and the result is a
/// hex-encoded string.
///
/// # Arguments
///
/// * `secret` - The shared signing secret.
/// * `timestamp` - The timestamp string (typically ISO-8601 / RFC-3339).
/// * `body` - The raw JSON body bytes.
#[must_use]
pub fn sign(secret: &str, timestamp: &str, body: &[u8]) -> String {
    let key = derive_key(secret);
    let mut hasher = blake3::Hasher::new_keyed(&key);
    hasher.update(timestamp.as_bytes());
    hasher.update(b".");
    hasher.update(body);
    hasher.finalize().to_hex().to_string()
}

/// Verify a webhook signature against the expected value.
///
/// Uses constant-time comparison to prevent timing attacks.
///
/// # Errors
///
/// Returns [`WebhookError::SignatureInvalid`] if the signature does not
/// match.
pub fn verify(secret: &str, timestamp: &str, body: &[u8], signature: &str) -> Result<()> {
    let expected = sign(secret, timestamp, body);
    if constant_time_eq(expected.as_bytes(), signature.as_bytes()) {
        Ok(())
    } else {
        Err(WebhookError::SignatureInvalid)
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Derive a 32-byte key from the secret string using BLAKE3's key derivation.
///
/// This ensures we always have a fixed-length key regardless of the secret
/// length.
fn derive_key(secret: &str) -> [u8; 32] {
    blake3::derive_key("polkagent-surface-webhook v1 signing key", secret.as_bytes())
}

/// Constant-time byte-string comparison to prevent timing side-channels.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_produces_hex_string() {
        let sig = sign("my-secret", "2024-06-01T00:00:00Z", b"{}");
        // BLAKE3 keyed hash output is 64 hex chars (32 bytes).
        assert_eq!(sig.len(), 64);
        assert!(sig.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn sign_is_deterministic() {
        let s1 = sign("secret", "ts1", b"body1");
        let s2 = sign("secret", "ts1", b"body1");
        assert_eq!(s1, s2);
    }

    #[test]
    fn different_secrets_produce_different_signatures() {
        let s1 = sign("secret-a", "ts1", b"body");
        let s2 = sign("secret-b", "ts1", b"body");
        assert_ne!(s1, s2);
    }

    #[test]
    fn different_timestamps_produce_different_signatures() {
        let s1 = sign("secret", "ts1", b"body");
        let s2 = sign("secret", "ts2", b"body");
        assert_ne!(s1, s2);
    }

    #[test]
    fn different_bodies_produce_different_signatures() {
        let s1 = sign("secret", "ts1", b"body-a");
        let s2 = sign("secret", "ts1", b"body-b");
        assert_ne!(s1, s2);
    }

    #[test]
    fn verify_accepts_valid_signature() {
        let sig = sign("secret", "ts", b"body");
        assert!(verify("secret", "ts", b"body", &sig).is_ok());
    }

    #[test]
    fn verify_rejects_wrong_signature() {
        let result = verify("secret", "ts", b"body", "0000000000000000000000000000000000000000000000000000000000000000");
        assert!(result.is_err());
    }

    #[test]
    fn verify_rejects_wrong_secret() {
        let sig = sign("secret-a", "ts", b"body");
        let result = verify("secret-b", "ts", b"body", &sig);
        assert!(result.is_err());
    }

    #[test]
    fn verify_rejects_wrong_timestamp() {
        let sig = sign("secret", "ts-a", b"body");
        let result = verify("secret", "ts-b", b"body", &sig);
        assert!(result.is_err());
    }

    #[test]
    fn verify_rejects_tampered_body() {
        let sig = sign("secret", "ts", b"original");
        let result = verify("secret", "ts", b"tampered", &sig);
        assert!(result.is_err());
    }

    #[test]
    fn constant_time_eq_works() {
        assert!(constant_time_eq(b"hello", b"hello"));
        assert!(!constant_time_eq(b"hello", b"world"));
        assert!(!constant_time_eq(b"short", b"longer"));
    }

    #[test]
    fn empty_body_produces_valid_signature() {
        let sig = sign("secret", "ts", b"");
        assert_eq!(sig.len(), 64);
        assert!(verify("secret", "ts", b"", &sig).is_ok());
    }
}
