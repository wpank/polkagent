//! Deterministic mock backend for JAM client testing.
//!
//! Provides a [`MockJamBackend`] that returns pre-configured blocks without
//! requiring a live JAM testnet node. Used for unit and integration tests.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::error::JamError;
use crate::types::{JamBlock, JamBlockHeader, JamServiceId};

/// A mock JAM backend that serves pre-loaded blocks by slot number.
#[derive(Debug, Clone)]
pub struct MockJamBackend {
    blocks: Arc<Mutex<HashMap<u64, JamBlock>>>,
    latest_slot: Arc<Mutex<u64>>,
}

impl MockJamBackend {
    /// Create an empty mock backend.
    #[must_use]
    pub fn new() -> Self {
        Self {
            blocks: Arc::new(Mutex::new(HashMap::new())),
            latest_slot: Arc::new(Mutex::new(0)),
        }
    }

    /// Create a mock backend pre-populated with a genesis block and `n`
    /// additional blocks.
    #[must_use]
    pub fn with_blocks(n: u64) -> Self {
        let backend = Self::new();
        let mut parent_hash =
            "0x0000000000000000000000000000000000000000000000000000000000000000".to_string();

        for slot in 0..=n {
            let hash = format!("0x{slot:064x}");
            let block = JamBlock {
                hash: hash.clone(),
                header: JamBlockHeader {
                    slot,
                    parent_hash: parent_hash.clone(),
                    state_root: format!("0x{:064x}", slot.wrapping_mul(7)),
                    extrinsic_root: format!("0x{:064x}", slot.wrapping_mul(13)),
                },
                work_report_service_ids: if slot > 0 {
                    vec![JamServiceId((slot % 10) as u32)]
                } else {
                    vec![]
                },
            };
            backend.insert_block(block);
            parent_hash = hash;
        }
        backend
    }

    /// Insert a block into the mock backend.
    pub fn insert_block(&self, block: JamBlock) {
        let slot = block.header.slot;
        let mut blocks = self
            .blocks
            .lock()
            .unwrap_or_else(|e| panic!("lock poisoned: {e}"));
        blocks.insert(slot, block);

        let mut latest = self
            .latest_slot
            .lock()
            .unwrap_or_else(|e| panic!("lock poisoned: {e}"));
        if slot > *latest {
            *latest = slot;
        }
    }

    /// Query a block by slot number.
    pub fn get_block(&self, slot: u64) -> Result<JamBlock, JamError> {
        let blocks = self
            .blocks
            .lock()
            .unwrap_or_else(|e| panic!("lock poisoned: {e}"));
        blocks
            .get(&slot)
            .cloned()
            .ok_or(JamError::BlockNotFound { slot })
    }

    /// Get the latest block.
    pub fn get_latest_block(&self) -> Result<JamBlock, JamError> {
        let latest = *self
            .latest_slot
            .lock()
            .unwrap_or_else(|e| panic!("lock poisoned: {e}"));
        self.get_block(latest)
    }

    /// Get the latest slot number.
    #[must_use]
    pub fn latest_slot(&self) -> u64 {
        *self
            .latest_slot
            .lock()
            .unwrap_or_else(|e| panic!("lock poisoned: {e}"))
    }

    /// Return the total number of stored blocks.
    #[must_use]
    pub fn block_count(&self) -> usize {
        self.blocks
            .lock()
            .unwrap_or_else(|e| panic!("lock poisoned: {e}"))
            .len()
    }
}

impl Default for MockJamBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
// Mock backend tests intentionally panic at fixture and rejection boundaries so
// broken deterministic chain state remains easy to diagnose.
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "JAM mock backend assertions intentionally panic with focused diagnostics"
)]
mod tests {
    use super::*;

    #[test]
    fn empty_backend_returns_block_not_found() {
        let backend = MockJamBackend::new();
        let err = backend.get_block(0).unwrap_err();
        assert!(matches!(err, JamError::BlockNotFound { slot: 0 }));
    }

    #[test]
    fn with_blocks_creates_chain() {
        let backend = MockJamBackend::with_blocks(5);
        assert_eq!(backend.block_count(), 6); // slots 0..=5
        assert_eq!(backend.latest_slot(), 5);

        // Genesis has no work reports.
        let genesis = backend.get_block(0).expect("genesis");
        assert!(genesis.work_report_service_ids.is_empty());
        assert_eq!(genesis.header.slot, 0);

        // Block 3 has a work report.
        let block3 = backend.get_block(3).expect("block 3");
        assert_eq!(block3.header.slot, 3);
        assert_eq!(block3.work_report_service_ids.len(), 1);
    }

    #[test]
    fn parent_hash_chain_is_consistent() {
        let backend = MockJamBackend::with_blocks(3);
        let b0 = backend.get_block(0).expect("block 0");
        let b1 = backend.get_block(1).expect("block 1");
        let b2 = backend.get_block(2).expect("block 2");
        let b3 = backend.get_block(3).expect("block 3");

        assert_eq!(b1.header.parent_hash, b0.hash);
        assert_eq!(b2.header.parent_hash, b1.hash);
        assert_eq!(b3.header.parent_hash, b2.hash);
    }

    #[test]
    fn insert_block_updates_latest() {
        let backend = MockJamBackend::new();
        assert_eq!(backend.latest_slot(), 0);

        backend.insert_block(JamBlock {
            hash: "0xaaa".into(),
            header: JamBlockHeader {
                slot: 10,
                parent_hash: "0x00".into(),
                state_root: "0x00".into(),
                extrinsic_root: "0x00".into(),
            },
            work_report_service_ids: vec![],
        });
        assert_eq!(backend.latest_slot(), 10);
    }

    #[test]
    fn get_latest_block_returns_highest_slot() {
        let backend = MockJamBackend::with_blocks(4);
        let latest = backend.get_latest_block().expect("latest");
        assert_eq!(latest.header.slot, 4);
    }

    #[test]
    fn default_creates_empty_backend() {
        let backend = MockJamBackend::default();
        assert_eq!(backend.block_count(), 0);
    }
}
