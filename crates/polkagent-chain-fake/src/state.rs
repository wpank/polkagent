//! Internal mutable state for [`FakeChainClient`].
//!
//! Kept in a separate module so that the locking discipline is visible in one
//! place and cannot be accidentally broken by other modules.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use parking_lot::Mutex;

use crate::builder::FakeChainClientBuilder;

// ---------------------------------------------------------------------------
// Call record
// ---------------------------------------------------------------------------

/// A single recorded invocation of a [`ChainClient`] trait method.
///
/// Collected by [`FakeChainClient::calls`] for assertion in tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallRecord {
    FetchMetadata { chain_profile: String },
    Simulate { extrinsic_len: usize, block_number: u64 },
    SubmitExtrinsic { extrinsic_len: usize, chain_profile: String },
    WatchFinality { tx_hash: String, chain_profile: String, timeout_ms: u64 },
    DecodeCall { call_len: usize },
    QueryStorage { key: Vec<u8>, chain_profile: String },
    DryRunCall { extrinsic_len: usize },
    XcmQueryAcceptablePaymentAssets { version: u8 },
    XcmQueryDeliveryFee { dest: String, message_len: usize },
    IsTrustedTeleporter { dest: String, asset: String },
    IsReserveTransferSupported { dest: String, asset: String },
    Health,
}

// ---------------------------------------------------------------------------
// FakeChainState
// ---------------------------------------------------------------------------

/// The shared, lock-protected inner state of a [`FakeChainClient`].
#[allow(dead_code)]
pub struct FakeChainState {
    // --- configuration ---
    pub network_name: String,
    pub genesis_hash: [u8; 32],
    pub spec_version: u32,
    pub impl_version: u32,
    pub metadata_hash: [u8; 32],
    pub block_number: AtomicU64,
    pub finalized_block_number: AtomicU64,
    pub latency_ms: u64,
    pub disconnected: bool,

    // --- pre-seeded data ---
    pub storage_values: Mutex<HashMap<Vec<u8>, Vec<u8>>>,
    pub extrinsic_results: Mutex<HashMap<[u8; 32], bool>>,
    pub balances: Mutex<HashMap<[u8; 32], (u128, u128)>>,

    // --- fault injection ---
    pub fail_remaining: AtomicUsize,

    // --- call tracking ---
    pub calls: Mutex<Vec<CallRecord>>,
}

#[allow(dead_code)]
impl FakeChainState {
    pub fn from_builder(b: FakeChainClientBuilder) -> Self {
        Self {
            network_name: b.network_name,
            genesis_hash: b.genesis_hash,
            spec_version: b.spec_version,
            impl_version: b.impl_version,
            metadata_hash: b.metadata_hash,
            block_number: AtomicU64::new(b.block_number),
            finalized_block_number: AtomicU64::new(b.finalized_block_number),
            latency_ms: b.latency_ms,
            disconnected: b.disconnected,
            storage_values: Mutex::new(b.storage_values),
            extrinsic_results: Mutex::new(b.extrinsic_results),
            balances: Mutex::new(b.balances),
            fail_remaining: AtomicUsize::new(b.fail_next_n),
            calls: Mutex::new(Vec::new()),
        }
    }

    /// Record a call and return `true` if this call should be faulted.
    pub fn record_and_check_fault(&self, call: CallRecord) -> bool {
        self.calls.lock().push(call);
        if self.disconnected {
            return true;
        }
        // Atomically decrement and fault if remaining > 0.
        let prev = self.fail_remaining.fetch_update(
            Ordering::SeqCst,
            Ordering::SeqCst,
            |n| if n > 0 { Some(n - 1) } else { None },
        );
        prev.is_ok()
    }

    pub fn current_block_number(&self) -> u64 {
        self.block_number.load(Ordering::SeqCst)
    }

    pub fn finalized_block_number(&self) -> u64 {
        self.finalized_block_number.load(Ordering::SeqCst)
    }

    pub fn genesis_hex(&self) -> String {
        format!("0x{}", hex_encode(&self.genesis_hash))
    }

    pub fn metadata_digest_hex(&self) -> String {
        format!("0x{}", hex_encode(&self.metadata_hash))
    }
}

pub fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
