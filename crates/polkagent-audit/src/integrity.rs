//! Integrity hash chain for tamper detection.
//!
//! Each audit entry's [`integrity_hash`](crate::entry::AuditEntry::integrity_hash)
//! is computed as `BLAKE3(prev_hash || entry_json)`. This creates a hash chain
//! similar to a blockchain: tampering with or deleting any entry breaks the
//! chain and is detectable by [`verify_chain`].

use crate::entry::AuditEntry;
use crate::error::{AuditError, AuditResult};

/// The genesis hash used as the "previous hash" for the very first entry.
pub const GENESIS_HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// Compute the integrity hash for an entry given the previous hash in the
/// chain.
///
/// The hash is `BLAKE3(prev_hash_bytes || canonical_entry_json_bytes)`. The
/// entry is serialized with its `integrity_hash` field set to the empty
/// string so that the hash does not include itself.
pub fn compute_hash(prev_hash: &str, entry: &AuditEntry) -> AuditResult<String> {
    // Serialize the entry with integrity_hash blanked out so we get a stable
    // canonical form that does not depend on the hash value itself.
    let mut canonical = entry.clone();
    canonical.integrity_hash = String::new();
    let entry_json = serde_json::to_string(&canonical)?;

    let mut hasher = blake3::Hasher::new();
    hasher.update(prev_hash.as_bytes());
    hasher.update(entry_json.as_bytes());
    Ok(hasher.finalize().to_hex().to_string())
}

/// Verify the integrity of a chain of audit entries.
///
/// Returns `Ok(())` if every entry's `integrity_hash` matches the expected
/// `BLAKE3(prev_hash || entry_json)`. Returns an [`AuditError::IntegrityViolation`]
/// on the first mismatch.
pub fn verify_chain(entries: &[AuditEntry]) -> AuditResult<()> {
    let mut prev_hash = GENESIS_HASH.to_string();

    for (i, entry) in entries.iter().enumerate() {
        let expected = compute_hash(&prev_hash, entry)?;
        if entry.integrity_hash != expected {
            return Err(AuditError::IntegrityViolation {
                message: format!(
                    "hash mismatch at index {i} (id={}): expected {expected}, got {}",
                    entry.id, entry.integrity_hash
                ),
            });
        }
        prev_hash.clone_from(&entry.integrity_hash);
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
// Test fixtures use explicit panic boundaries to identify broken invariants.
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "unit-test audit assertions intentionally panic with focused diagnostics"
)]
mod tests {
    use super::*;
    use crate::action::AuditAction;
    use crate::actor::ActorInfo;
    use crate::entry::{ActionOutcome, AuditEntry, ResourceInfo};

    fn make_entry(action: AuditAction) -> AuditEntry {
        AuditEntry::new(
            ActorInfo::agent("test-agent"),
            action,
            ResourceInfo::new("test", "res-1"),
            ActionOutcome::Success,
            serde_json::Value::Null,
        )
    }

    #[test]
    fn compute_hash_is_deterministic() {
        let entry = make_entry(AuditAction::RunStarted);
        let h1 = compute_hash(GENESIS_HASH, &entry).expect("hash");
        let h2 = compute_hash(GENESIS_HASH, &entry).expect("hash");
        assert_eq!(h1, h2);
    }

    #[test]
    fn compute_hash_changes_with_prev_hash() {
        let entry = make_entry(AuditAction::RunStarted);
        let h1 = compute_hash(GENESIS_HASH, &entry).expect("hash");
        let h2 = compute_hash("aaaa", &entry).expect("hash");
        assert_ne!(h1, h2);
    }

    #[test]
    fn compute_hash_changes_with_entry_content() {
        let e1 = make_entry(AuditAction::RunStarted);
        let e2 = make_entry(AuditAction::RunCompleted);
        let h1 = compute_hash(GENESIS_HASH, &e1).expect("hash");
        let h2 = compute_hash(GENESIS_HASH, &e2).expect("hash");
        assert_ne!(h1, h2);
    }

    #[test]
    fn verify_chain_empty_is_ok() {
        assert!(verify_chain(&[]).is_ok());
    }

    #[test]
    fn verify_chain_single_valid_entry() {
        let mut entry = make_entry(AuditAction::RunStarted);
        entry.integrity_hash = compute_hash(GENESIS_HASH, &entry).expect("hash");
        assert!(verify_chain(&[entry]).is_ok());
    }

    #[test]
    fn verify_chain_multiple_valid_entries() {
        let mut e1 = make_entry(AuditAction::RunStarted);
        e1.integrity_hash = compute_hash(GENESIS_HASH, &e1).expect("hash");

        let mut e2 = make_entry(AuditAction::ToolInvoked);
        e2.integrity_hash = compute_hash(&e1.integrity_hash, &e2).expect("hash");

        let mut e3 = make_entry(AuditAction::RunCompleted);
        e3.integrity_hash = compute_hash(&e2.integrity_hash, &e3).expect("hash");

        assert!(verify_chain(&[e1, e2, e3]).is_ok());
    }

    #[test]
    fn verify_chain_detects_tampered_entry() {
        let mut e1 = make_entry(AuditAction::RunStarted);
        e1.integrity_hash = compute_hash(GENESIS_HASH, &e1).expect("hash");

        let mut e2 = make_entry(AuditAction::ToolInvoked);
        e2.integrity_hash = compute_hash(&e1.integrity_hash, &e2).expect("hash");

        // Tamper with e1 after hashing.
        e1.outcome = ActionOutcome::Failure;

        let result = verify_chain(&[e1, e2]);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("hash mismatch at index 0"));
    }

    #[test]
    fn verify_chain_detects_deleted_entry() {
        let mut e1 = make_entry(AuditAction::RunStarted);
        e1.integrity_hash = compute_hash(GENESIS_HASH, &e1).expect("hash");

        let mut e2 = make_entry(AuditAction::ToolInvoked);
        e2.integrity_hash = compute_hash(&e1.integrity_hash, &e2).expect("hash");

        let mut e3 = make_entry(AuditAction::RunCompleted);
        e3.integrity_hash = compute_hash(&e2.integrity_hash, &e3).expect("hash");

        // Delete the middle entry.
        let result = verify_chain(&[e1, e3]);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("hash mismatch at index 1"));
    }

    #[test]
    fn hash_length_is_64_hex_chars() {
        let entry = make_entry(AuditAction::RunStarted);
        let h = compute_hash(GENESIS_HASH, &entry).expect("hash");
        assert_eq!(h.len(), 64);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
