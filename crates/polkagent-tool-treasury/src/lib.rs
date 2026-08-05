//! `polkagent-tool-treasury` — Treasury and portfolio analysis tools for
//! Polkagent agents.
//!
//! This crate provides a suite of read-only chain query tools that let agents
//! analyze DOT/KSM balances, staking positions, crowdloans, transfer history,
//! vesting schedules, and aggregated asset portfolios on Polkadot-SDK chains.
//!
//! # Tools
//!
//! | Tool | Description |
//! |------|-------------|
//! | [`BalanceQueryTool`] | Free/reserved/frozen/total balance for an account |
//! | [`StakingInfoTool`] | Validator/nominator status, bonded amounts, rewards |
//! | [`PortfolioSummaryTool`] | Aggregated view of all asset positions |
//! | [`TransferHistoryTool`] | Recent transfers in/out with timestamps |
//! | [`VestingScheduleTool`] | Vesting schedules, unlocked amount, next unlock |
//!
//! All tools require the `chain.query` grant (read-only chain access) and
//! accept an `account_id` (SS58 string) plus an optional `chain` identifier
//! (defaults to `"polkadot"`).
//!
//! # Registration
//!
//! Use [`register_treasury_tools`] to add all treasury tools to a
//! [`ToolRegistry`] in one call:
//!
//! ```rust,no_run
//! use std::sync::Arc;
//! use polkagent_tool::ToolRegistry;
//! use polkagent_tool_treasury::register_treasury_tools;
//!
//! # fn example(chain_client: Arc<dyn polkagent_chain_trait::ChainClient>) {
//! let mut registry = ToolRegistry::new();
//! register_treasury_tools(&mut registry, chain_client);
//! # }
//! ```

#![forbid(unsafe_code)]
#![warn(
    missing_docs,
    clippy::pedantic,
    clippy::unwrap_used,
    clippy::expect_used
)]
#![allow(
    clippy::module_name_repetitions,
    clippy::must_use_candidate,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc
)]

pub mod balance;
pub mod error;
pub mod portfolio;
pub mod staking;
pub mod transfer;
pub mod types;
pub mod vesting;

// ---------------------------------------------------------------------------
// Convenience re-exports
// ---------------------------------------------------------------------------

pub use balance::BalanceQueryTool;
pub use error::TreasuryToolError;
pub use portfolio::PortfolioSummaryTool;
pub use staking::StakingInfoTool;
pub use transfer::TransferHistoryTool;
pub use types::{
    AccountBalance, PortfolioEntry, PortfolioSource, StakingPosition, StakingRole, Transfer,
    UnlockingChunk, VestingInfo, VestingSchedule,
};
pub use vesting::VestingScheduleTool;

use std::sync::Arc;

use polkagent_chain_trait::ChainClient;
use polkagent_tool::ToolRegistry;

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// Register all treasury and portfolio analysis tools into the given registry.
///
/// Adds:
/// - [`BalanceQueryTool`] — account balance breakdown
/// - [`StakingInfoTool`] — staking position details
/// - [`PortfolioSummaryTool`] — aggregated portfolio view
/// - [`TransferHistoryTool`] — recent transfer events
/// - [`VestingScheduleTool`] — vesting schedule information
///
/// Each tool requires a `chain.query` grant for execution.
pub fn register_treasury_tools(registry: &mut ToolRegistry, chain_client: Arc<dyn ChainClient>) {
    registry.register(Box::new(BalanceQueryTool::new(chain_client.clone())));
    registry.register(Box::new(StakingInfoTool::new(chain_client.clone())));
    registry.register(Box::new(PortfolioSummaryTool::new(chain_client.clone())));
    registry.register(Box::new(TransferHistoryTool::new(chain_client.clone())));
    registry.register(Box::new(VestingScheduleTool::new(chain_client)));
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Test utilities
// ---------------------------------------------------------------------------

/// Test utilities and mock chain client used across tool test modules.
#[cfg(test)]
pub(crate) mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use async_trait::async_trait;

    use polkagent_chain_trait::{
        BlockRef, ChainClient, ChainError, ChainProfileId, DecodedCall, DryRunResult,
        FinalityObservation, GenesisHash, PinnedMetadata, SimulationResult, TxHash,
    };

    use super::*;
    use polkagent_tool::ToolHandler;

    // -----------------------------------------------------------------------
    // MockChainClient
    // -----------------------------------------------------------------------

    /// A mock [`ChainClient`] that stores data in-memory for testing.
    ///
    /// Tests insert storage entries via [`insert_storage`] and the tools
    /// read them back through the standard [`ChainClient::query_storage`]
    /// interface.
    ///
    /// [`insert_storage`]: MockChainClient::insert_storage
    pub(crate) struct MockChainClient {
        storage: Mutex<HashMap<Vec<u8>, Vec<u8>>>,
    }

    impl MockChainClient {
        /// Create a new empty mock chain client.
        pub(crate) fn new() -> Self {
            Self {
                storage: Mutex::new(HashMap::new()),
            }
        }

        /// Insert a storage entry that will be returned by `query_storage`.
        pub(crate) fn insert_storage(&mut self, key: Vec<u8>, value: Vec<u8>) {
            self.storage
                .get_mut()
                .unwrap_or_else(|e| panic!("lock poisoned: {e}"))
                .insert(key, value);
        }
    }

    #[async_trait]
    impl ChainClient for MockChainClient {
        async fn fetch_metadata(
            &self,
            _chain_profile: ChainProfileId,
        ) -> Result<PinnedMetadata, ChainError> {
            Err(ChainError::Internal {
                message: "mock: fetch_metadata not implemented".to_string(),
            })
        }

        async fn simulate(
            &self,
            _signed_extrinsic: &[u8],
            _block_ref: &BlockRef,
            _metadata: &PinnedMetadata,
        ) -> Result<SimulationResult, ChainError> {
            Err(ChainError::Internal {
                message: "mock: simulate not implemented".to_string(),
            })
        }

        async fn submit_extrinsic(
            &self,
            _signed_extrinsic: &[u8],
            _chain_profile: ChainProfileId,
        ) -> Result<TxHash, ChainError> {
            Err(ChainError::Internal {
                message: "mock: submit_extrinsic not implemented".to_string(),
            })
        }

        async fn watch_finality(
            &self,
            _tx_hash: TxHash,
            _chain_profile: ChainProfileId,
            _timeout_ms: u64,
        ) -> Result<FinalityObservation, ChainError> {
            Err(ChainError::Internal {
                message: "mock: watch_finality not implemented".to_string(),
            })
        }

        async fn decode_call(
            &self,
            _call_bytes: &[u8],
            _metadata: &PinnedMetadata,
        ) -> Result<DecodedCall, ChainError> {
            Err(ChainError::Internal {
                message: "mock: decode_call not implemented".to_string(),
            })
        }

        async fn query_storage(
            &self,
            storage_key: &[u8],
            _block_ref: Option<&BlockRef>,
            _chain_profile: ChainProfileId,
        ) -> Result<Option<Vec<u8>>, ChainError> {
            let map = self.storage.lock().map_err(|e| ChainError::Internal {
                message: format!("lock poisoned: {e}"),
            })?;
            Ok(map.get(storage_key).cloned())
        }

        async fn dry_run_call(&self, _extrinsic: &[u8]) -> Result<DryRunResult, ChainError> {
            Err(ChainError::Unsupported {
                operation: "dry_run_call".into(),
            })
        }

        async fn xcm_query_acceptable_payment_assets(
            &self,
            _version: u8,
        ) -> Result<Vec<String>, ChainError> {
            Err(ChainError::Unsupported {
                operation: "xcm_query_acceptable_payment_assets".into(),
            })
        }

        async fn xcm_query_delivery_fee(
            &self,
            _dest: &GenesisHash,
            _message: &[u8],
        ) -> Result<u128, ChainError> {
            Err(ChainError::Unsupported {
                operation: "xcm_query_delivery_fee".into(),
            })
        }

        async fn is_trusted_teleporter(
            &self,
            _dest: &ChainProfileId,
            _asset: &str,
        ) -> Result<bool, ChainError> {
            Err(ChainError::Unsupported {
                operation: "is_trusted_teleporter".into(),
            })
        }

        async fn is_reserve_transfer_supported(
            &self,
            _dest: &ChainProfileId,
            _asset: &str,
        ) -> Result<bool, ChainError> {
            Err(ChainError::Unsupported {
                operation: "is_reserve_transfer_supported".into(),
            })
        }

        async fn health(&self) -> Result<(), ChainError> {
            Ok(())
        }
    }

    // -----------------------------------------------------------------------
    // Registration tests
    // -----------------------------------------------------------------------

    fn mock_client() -> Arc<dyn ChainClient> {
        Arc::new(MockChainClient::new())
    }

    #[test]
    fn register_treasury_tools_adds_five_tools() {
        let mut registry = ToolRegistry::new();
        register_treasury_tools(&mut registry, mock_client());
        assert_eq!(registry.len(), 5);
    }

    #[test]
    fn all_tool_names_are_namespaced() {
        let mut registry = ToolRegistry::new();
        register_treasury_tools(&mut registry, mock_client());

        for spec in registry.list() {
            assert!(
                spec.name.starts_with("polkagent.treasury."),
                "tool name '{}' does not start with 'polkagent.treasury.'",
                spec.name
            );
        }
    }

    #[test]
    fn all_tools_require_chain_query_grant() {
        let mut registry = ToolRegistry::new();
        register_treasury_tools(&mut registry, mock_client());

        for spec in registry.list() {
            assert_eq!(
                spec.required_grant.as_deref(),
                Some("chain.query"),
                "tool '{}' should require chain.query grant",
                spec.name
            );
        }
    }

    #[test]
    fn all_tools_have_account_id_in_schema() {
        let mut registry = ToolRegistry::new();
        register_treasury_tools(&mut registry, mock_client());

        for spec in registry.list() {
            let has_account_id = spec.input_schema["properties"].get("account_id").is_some();
            assert!(
                has_account_id,
                "tool '{}' should have account_id in input schema",
                spec.name
            );
        }
    }

    #[test]
    fn tool_specs_are_individually_accessible() {
        let client = mock_client();

        let balance_spec = BalanceQueryTool::new(client.clone()).spec();
        assert_eq!(balance_spec.name, "polkagent.treasury.balance_query");

        let staking_spec = StakingInfoTool::new(client.clone()).spec();
        assert_eq!(staking_spec.name, "polkagent.treasury.staking_info");

        let portfolio_spec = PortfolioSummaryTool::new(client.clone()).spec();
        assert_eq!(portfolio_spec.name, "polkagent.treasury.portfolio_summary");

        let transfer_spec = TransferHistoryTool::new(client.clone()).spec();
        assert_eq!(transfer_spec.name, "polkagent.treasury.transfer_history");

        let vesting_spec = VestingScheduleTool::new(client).spec();
        assert_eq!(vesting_spec.name, "polkagent.treasury.vesting_schedule");
    }

    #[test]
    fn registry_can_look_up_each_tool() {
        let mut registry = ToolRegistry::new();
        register_treasury_tools(&mut registry, mock_client());

        assert!(registry.get("polkagent.treasury.balance_query").is_some());
        assert!(registry.get("polkagent.treasury.staking_info").is_some());
        assert!(registry
            .get("polkagent.treasury.portfolio_summary")
            .is_some());
        assert!(registry
            .get("polkagent.treasury.transfer_history")
            .is_some());
        assert!(registry
            .get("polkagent.treasury.vesting_schedule")
            .is_some());
    }

    #[test]
    fn output_classification_is_internal() {
        let mut registry = ToolRegistry::new();
        register_treasury_tools(&mut registry, mock_client());

        for spec in registry.list() {
            assert_eq!(
                spec.output_classification,
                polkagent_core::config::DataClassification::Internal,
                "tool '{}' should have Internal output classification",
                spec.name
            );
        }
    }

    #[test]
    fn re_registering_replaces_tools() {
        let client = mock_client();
        let mut registry = ToolRegistry::new();
        register_treasury_tools(&mut registry, client.clone());
        assert_eq!(registry.len(), 5);

        // Register again: should replace, not duplicate.
        register_treasury_tools(&mut registry, client);
        assert_eq!(registry.len(), 5);
    }
}
