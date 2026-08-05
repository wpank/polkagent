//! Metadata pinning store.
//!
//! [`PinStore`] manages [`PinnedMetadata`] records — declarations that a
//! particular metadata hash is known-good for a given chain. The drift
//! detector uses these pins as the baseline for comparison.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use polkagent_core::now;

use crate::error::MetadataError;
use crate::types::{ChainId, MetadataDrift, MetadataHash, PinnedMetadata};

/// In-memory store of pinned metadata hashes.
///
/// Thread-safe; can be shared across async tasks.
#[derive(Debug, Clone)]
pub struct PinStore {
    inner: Arc<RwLock<HashMap<ChainId, Vec<PinnedMetadata>>>>,
}

impl PinStore {
    /// Create an empty pin store.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Pin a metadata hash as known-good for a chain.
    ///
    /// If the same `(chain_id, hash)` is already pinned, this is a no-op.
    pub fn pin(&self, chain_id: ChainId, hash: MetadataHash, label: impl Into<String>) {
        let mut inner = self
            .inner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let pins = inner.entry(chain_id.clone()).or_default();

        // Do not duplicate.
        if pins.iter().any(|p| p.hash == hash) {
            return;
        }

        pins.push(PinnedMetadata {
            chain_id,
            hash,
            pinned_at: now(),
            label: label.into(),
            trusted: true,
        });
    }

    /// Remove a pin for a specific chain and hash.
    ///
    /// Returns `Ok(())` if the pin was found and removed, or
    /// `Err(MetadataError::PinNotFound)` if no such pin exists.
    pub fn unpin(&self, chain_id: &ChainId, hash: &MetadataHash) -> Result<(), MetadataError> {
        let mut inner = self
            .inner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let pins = inner
            .get_mut(chain_id)
            .ok_or_else(|| MetadataError::PinNotFound {
                chain_id: chain_id.clone(),
                hash: hash.clone(),
            })?;

        let before = pins.len();
        pins.retain(|p| &p.hash != hash);
        if pins.len() == before {
            return Err(MetadataError::PinNotFound {
                chain_id: chain_id.clone(),
                hash: hash.clone(),
            });
        }

        // Clean up empty chain entries.
        if pins.is_empty() {
            inner.remove(chain_id);
        }

        Ok(())
    }

    /// Check whether a specific hash is pinned for a chain.
    #[must_use]
    pub fn is_pinned(&self, chain_id: &ChainId, hash: &MetadataHash) -> bool {
        let inner = self
            .inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inner
            .get(chain_id)
            .is_some_and(|pins| pins.iter().any(|p| &p.hash == hash))
    }

    /// Get all pinned metadata records for a chain.
    #[must_use]
    pub fn get_pinned(&self, chain_id: &ChainId) -> Vec<PinnedMetadata> {
        let inner = self
            .inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inner.get(chain_id).cloned().unwrap_or_default()
    }

    /// Verify that a current metadata hash matches at least one pin for the chain.
    ///
    /// If no pins exist for the chain, this returns `Ok(())` (nothing to
    /// compare against). If pins exist but none match, returns a
    /// [`MetadataDrift`] describing the mismatch.
    pub fn verify_against_pin(
        &self,
        chain_id: &ChainId,
        current_hash: &MetadataHash,
    ) -> Result<(), MetadataDrift> {
        let inner = self
            .inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(pins) = inner.get(chain_id) else {
            // No pins for this chain — nothing to verify against.
            return Ok(());
        };

        if pins.iter().any(|p| &p.hash == current_hash) {
            return Ok(());
        }

        // Drift detected: use the first pin as the reference.
        let first_pin = &pins[0];
        Err(MetadataDrift {
            chain_id: chain_id.clone(),
            pinned_hash: first_pin.hash.clone(),
            current_hash: current_hash.clone(),
            detected_at: now(),
            affected_pallets: vec!["(full pallet diff requires SCALE decoding)".into()],
        })
    }

    /// Return the total number of pins across all chains.
    #[must_use]
    pub fn total_pins(&self) -> usize {
        let inner = self
            .inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inner.values().map(Vec::len).sum()
    }
}

impl Default for PinStore {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn chain(name: &str) -> ChainId {
        ChainId::new(name)
    }

    fn hash(data: &[u8]) -> MetadataHash {
        MetadataHash::from_bytes(data)
    }

    #[test]
    fn pin_and_is_pinned() {
        let store = PinStore::new();
        let cid = chain("polkadot");
        let h = hash(b"metadata_v1");

        assert!(!store.is_pinned(&cid, &h));
        store.pin(cid.clone(), h.clone(), "v1.0");
        assert!(store.is_pinned(&cid, &h));
    }

    #[test]
    fn pin_is_idempotent() {
        let store = PinStore::new();
        let cid = chain("polkadot");
        let h = hash(b"data");

        store.pin(cid.clone(), h.clone(), "first");
        store.pin(cid.clone(), h.clone(), "second");
        assert_eq!(store.total_pins(), 1);
    }

    #[test]
    fn unpin_removes_pin() {
        let store = PinStore::new();
        let cid = chain("polkadot");
        let h = hash(b"data");

        store.pin(cid.clone(), h.clone(), "label");
        assert!(store.is_pinned(&cid, &h));

        store.unpin(&cid, &h).expect("unpin should succeed");
        assert!(!store.is_pinned(&cid, &h));
    }

    #[test]
    fn unpin_returns_error_for_missing() {
        let store = PinStore::new();
        let result = store.unpin(&chain("polkadot"), &hash(b"nonexistent"));
        assert!(result.is_err());
    }

    #[test]
    fn get_pinned_returns_all_for_chain() {
        let store = PinStore::new();
        let cid = chain("polkadot");

        store.pin(cid.clone(), hash(b"v1"), "v1");
        store.pin(cid.clone(), hash(b"v2"), "v2");
        store.pin(chain("kusama"), hash(b"k1"), "k1");

        let polkadot_pins = store.get_pinned(&cid);
        assert_eq!(polkadot_pins.len(), 2);
    }

    #[test]
    fn get_pinned_returns_empty_for_unknown_chain() {
        let store = PinStore::new();
        let result = store.get_pinned(&chain("unknown"));
        assert!(result.is_empty());
    }

    #[test]
    fn verify_against_pin_ok_when_matching() {
        let store = PinStore::new();
        let cid = chain("polkadot");
        let h = hash(b"trusted_metadata");

        store.pin(cid.clone(), h.clone(), "trusted");
        let result = store.verify_against_pin(&cid, &h);
        assert!(result.is_ok());
    }

    #[test]
    fn verify_against_pin_ok_when_no_pins() {
        let store = PinStore::new();
        let result = store.verify_against_pin(&chain("polkadot"), &hash(b"anything"));
        assert!(result.is_ok());
    }

    #[test]
    fn verify_against_pin_drift_when_mismatch() {
        let store = PinStore::new();
        let cid = chain("polkadot");
        let pinned = hash(b"old_metadata");
        let current = hash(b"new_metadata");

        store.pin(cid.clone(), pinned.clone(), "old");

        let result = store.verify_against_pin(&cid, &current);
        assert!(result.is_err());

        let drift = result.unwrap_err();
        assert_eq!(drift.chain_id, cid);
        assert_eq!(drift.pinned_hash, pinned);
        assert_eq!(drift.current_hash, current);
        assert!(!drift.affected_pallets.is_empty());
    }

    #[test]
    fn verify_against_pin_ok_with_multiple_pins() {
        let store = PinStore::new();
        let cid = chain("polkadot");
        let h1 = hash(b"v1");
        let h2 = hash(b"v2");

        store.pin(cid.clone(), h1, "v1");
        store.pin(cid.clone(), h2.clone(), "v2");

        // Matching the second pin should be OK.
        let result = store.verify_against_pin(&cid, &h2);
        assert!(result.is_ok());
    }

    #[test]
    fn total_pins_counts_all_chains() {
        let store = PinStore::new();
        store.pin(chain("polkadot"), hash(b"p1"), "p1");
        store.pin(chain("polkadot"), hash(b"p2"), "p2");
        store.pin(chain("kusama"), hash(b"k1"), "k1");
        assert_eq!(store.total_pins(), 3);
    }

    #[test]
    fn pinned_metadata_has_correct_fields() {
        let store = PinStore::new();
        let cid = chain("polkadot");
        let h = hash(b"data");

        store.pin(cid.clone(), h.clone(), "my-label");
        let pins = store.get_pinned(&cid);
        assert_eq!(pins.len(), 1);
        assert_eq!(pins[0].label, "my-label");
        assert!(pins[0].trusted);
        assert_eq!(pins[0].chain_id, cid);
        assert_eq!(pins[0].hash, h);
    }

    #[test]
    fn thread_safety() {
        use std::sync::Arc;
        use std::thread;

        let store = Arc::new(PinStore::new());
        let mut handles = vec![];

        for i in 0..10 {
            let store = Arc::clone(&store);
            handles.push(thread::spawn(move || {
                let data = format!("pin_{i}");
                store.pin(
                    chain("polkadot"),
                    hash(data.as_bytes()),
                    format!("label_{i}"),
                );
            }));
        }

        for h in handles {
            h.join().expect("thread panicked");
        }

        assert_eq!(store.total_pins(), 10);
    }
}
