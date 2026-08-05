//! Builder for [`FakeChainClient`].
//!
//! [`FakeChainClientBuilder`] provides a fluent API for constructing a
//! [`FakeChainClient`] with pre-seeded responses, fault-injection settings,
//! and simulated network latency.

use std::collections::HashMap;

use crate::state::FakeChainState;
use crate::FakeChainClient;

// ---------------------------------------------------------------------------
// Well-known network defaults
// ---------------------------------------------------------------------------

/// Polkadot mainnet genesis hash (leading bytes of the real hash for test use).
pub const POLKADOT_GENESIS: [u8; 32] = [
    0x91, 0xb1, 0x71, 0xbb, 0x15, 0x8e, 0x2d, 0x38, 0x48, 0xfa, 0x23, 0xa9, 0x7b, 0x64, 0x48, 0x77,
    0x58, 0x08, 0x55, 0x00, 0x7d, 0x04, 0x47, 0x87, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
];

/// Kusama genesis hash (leading bytes of the real hash for test use).
pub const KUSAMA_GENESIS: [u8; 32] = [
    0xb0, 0xa8, 0xd4, 0x93, 0x28, 0x5c, 0x2d, 0xf7, 0x32, 0x90, 0xdf, 0xb7, 0xe6, 0x1f, 0x87, 0x0f,
    0x17, 0xb4, 0x18, 0x01, 0x19, 0x7a, 0x14, 0x9c, 0xa9, 0x36, 0x54, 0x99, 0x9e, 0xbc, 0xae, 0x88,
];

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

/// Fluent builder for constructing a [`FakeChainClient`].
///
/// # Quick start
///
/// ```rust
/// use polkagent_chain_fake::FakeChainClientBuilder;
///
/// let client = FakeChainClientBuilder::new("polkadot")
///     .with_block_number(1_000_000)
///     .with_runtime_version(1_000_004, 0)
///     .build();
/// ```
#[derive(Debug)]
pub struct FakeChainClientBuilder {
    pub(crate) network_name: String,
    pub(crate) genesis_hash: [u8; 32],
    pub(crate) spec_version: u32,
    pub(crate) impl_version: u32,
    pub(crate) metadata_hash: [u8; 32],
    pub(crate) block_number: u64,
    pub(crate) finalized_block_number: u64,
    pub(crate) storage_values: HashMap<Vec<u8>, Vec<u8>>,
    /// Map from tx hash bytes → success flag for `submit_extrinsic`.
    pub(crate) extrinsic_results: HashMap<[u8; 32], bool>,
    /// Map from account id bytes → (free, reserved).
    pub(crate) balances: HashMap<[u8; 32], (u128, u128)>,
    pub(crate) fail_next_n: usize,
    pub(crate) latency_ms: u64,
    pub(crate) disconnected: bool,
}

impl FakeChainClientBuilder {
    /// Create a builder for the given network name.
    ///
    /// The network name populates [`polkagent_chain_trait::ChainProfile::name`] fields in returned
    /// data. It is also used to pick sensible defaults when `.build()` is
    /// called via the pre-defined network constructors.
    #[must_use]
    pub fn new(network: &str) -> Self {
        Self {
            network_name: network.to_string(),
            genesis_hash: default_genesis_for_network(network),
            spec_version: 1_000_000,
            impl_version: 0,
            metadata_hash: [0x42u8; 32],
            block_number: 1_000_000,
            finalized_block_number: 999_990,
            storage_values: HashMap::new(),
            extrinsic_results: HashMap::new(),
            balances: HashMap::new(),
            fail_next_n: 0,
            latency_ms: 0,
            disconnected: false,
        }
    }

    /// Use the Polkadot mainnet defaults.
    #[must_use]
    pub fn polkadot() -> Self {
        Self::new("polkadot")
            .with_genesis_hash(POLKADOT_GENESIS)
            .with_runtime_version(1_003_000, 0)
    }

    /// Use the Kusama defaults.
    #[must_use]
    pub fn kusama() -> Self {
        Self::new("kusama")
            .with_genesis_hash(KUSAMA_GENESIS)
            .with_runtime_version(1_003_000, 0)
    }

    /// Set the genesis hash that will be reported by the fake client.
    #[must_use]
    pub fn with_genesis_hash(mut self, hash: [u8; 32]) -> Self {
        self.genesis_hash = hash;
        self
    }

    /// Set the runtime spec/impl versions reported by the fake client.
    #[must_use]
    pub fn with_runtime_version(mut self, spec_version: u32, impl_version: u32) -> Self {
        self.spec_version = spec_version;
        self.impl_version = impl_version;
        self
    }

    /// Set the metadata digest that will be embedded in [`polkagent_chain_trait::PinnedMetadata`].
    #[must_use]
    pub fn with_metadata_hash(mut self, hash: [u8; 32]) -> Self {
        self.metadata_hash = hash;
        self
    }

    /// Set the current best-block number returned by the client.
    #[must_use]
    pub fn with_block_number(mut self, n: u64) -> Self {
        self.block_number = n;
        self
    }

    /// Seed a raw storage key → value pair.
    ///
    /// Calling [`polkagent_chain_trait::ChainClient::query_storage`] with this key will return
    /// `Some(value)`.
    #[must_use]
    pub fn with_storage_value(mut self, key: Vec<u8>, value: Vec<u8>) -> Self {
        self.storage_values.insert(key, value);
        self
    }

    /// Seed an extrinsic result for a specific tx hash.
    ///
    /// When [`polkagent_chain_trait::ChainClient::watch_finality`] is called for a tx whose first
    /// 32-byte hash matches `hash`, the fake returns either `Finalized` or
    /// `Failed` depending on `success`.
    #[must_use]
    pub fn with_extrinsic_result(mut self, hash: [u8; 32], success: bool) -> Self {
        self.extrinsic_results.insert(hash, success);
        self
    }

    /// Seed a balance entry for an account.
    ///
    /// The balance is stored as a raw storage value in a format that mirrors
    /// the Substrate `System::Account` storage key. Tests may query it via
    /// [`polkagent_chain_trait::ChainClient::query_storage`] using the account bytes as the key.
    #[must_use]
    pub fn with_balance(mut self, account: [u8; 32], free: u128, reserved: u128) -> Self {
        self.balances.insert(account, (free, reserved));
        // Also expose the balance as a storage value keyed on the account id.
        let mut balance_bytes = Vec::with_capacity(32);
        balance_bytes.extend_from_slice(&free.to_le_bytes());
        balance_bytes.extend_from_slice(&reserved.to_le_bytes());
        self.storage_values.insert(account.to_vec(), balance_bytes);
        self
    }

    /// Cause the next `n` calls to any trait method to return an error.
    ///
    /// After `n` failures the client returns to normal operation.
    #[must_use]
    pub fn fail_next_n(mut self, n: usize) -> Self {
        self.fail_next_n = n;
        self
    }

    /// Add artificial latency to every call.
    ///
    /// Each call will sleep for `ms` milliseconds before returning. This is
    /// useful for testing timeout logic.
    #[must_use]
    pub fn with_latency(mut self, ms: u64) -> Self {
        self.latency_ms = ms;
        self
    }

    /// Simulate a disconnected client.
    ///
    /// All calls return [`polkagent_chain_trait::ChainError::Rpc`] with `retryable: true`, as if the
    /// node were unreachable.
    #[must_use]
    pub fn disconnected(mut self) -> Self {
        self.disconnected = true;
        self
    }

    /// Consume the builder and construct a [`FakeChainClient`].
    #[must_use]
    pub fn build(self) -> FakeChainClient {
        FakeChainClient::from_state(FakeChainState::from_builder(self))
    }
}

/// Pick a sensible genesis hash default based on a well-known network name.
fn default_genesis_for_network(name: &str) -> [u8; 32] {
    match name {
        "polkadot" => POLKADOT_GENESIS,
        "kusama" => KUSAMA_GENESIS,
        _ => [0xFFu8; 32],
    }
}
