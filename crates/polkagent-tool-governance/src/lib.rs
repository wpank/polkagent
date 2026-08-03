//! `polkagent-tool-governance` — OpenGov governance research tools for the
//! Polkagent platform.
//!
//! This crate provides read-only tools that let agents research and analyze
//! Polkadot/Kusama OpenGov governance proposals, referenda, voting patterns,
//! delegations, and treasury state.
//!
//! # Tools
//!
//! | Tool | Description |
//! |------|-------------|
//! | [`referendum::ReferendumLookupTool`] | Look up a referendum by index |
//! | [`track::TrackInfoTool`] | List governance tracks with parameters |
//! | [`voter::VoterHistoryTool`] | Query voting history for an account |
//! | [`delegate::DelegationInfoTool`] | Query delegation graph for an account |
//! | [`treasury::TreasuryOverviewTool`] | Query treasury balance and proposals |
//!
//! # Registration
//!
//! Use [`register_governance_tools`] to add all governance tools to a
//! [`ToolRegistry`] in one call:
//!
//! ```rust,no_run
//! use std::sync::Arc;
//! use polkagent_tool::ToolRegistry;
//! use polkagent_tool_governance::register_governance_tools;
//!
//! # fn example(chain_client: Arc<dyn polkagent_chain_trait::ChainClient>) {
//! let mut registry = ToolRegistry::new();
//! register_governance_tools(&mut registry, chain_client);
//! assert_eq!(registry.len(), 5);
//! # }
//! ```
//!
//! # Required grant
//!
//! All tools require the `chain.query` grant, which authorizes read-only
//! chain state queries. No tool in this crate writes to the chain.

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

pub mod delegate;
pub mod error;
pub mod referendum;
pub mod track;
pub mod treasury;
pub mod types;
pub mod voter;

use std::sync::Arc;

use polkagent_chain_trait::ChainClient;
use polkagent_tool::ToolRegistry;

// ---------------------------------------------------------------------------
// Convenience re-exports
// ---------------------------------------------------------------------------

pub use delegate::DelegationInfoTool;
pub use error::GovernanceError;
pub use referendum::ReferendumLookupTool;
pub use track::TrackInfoTool;
pub use treasury::TreasuryOverviewTool;
pub use types::{
    Conviction, Delegation, Referendum, ReferendumStatus, TimelineEvent, Track, TreasuryInfo,
    TreasuryProposal, Vote, VoteDirection,
};
pub use voter::VoterHistoryTool;

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// Register all governance tools into the given registry.
///
/// This adds the referendum lookup, track info, voter history, delegation
/// info, and treasury overview tools. A [`ChainClient`] must be provided
/// so the tools can query chain state.
///
/// Call this once when setting up a new run or agent context.
pub fn register_governance_tools(
    registry: &mut ToolRegistry,
    chain_client: Arc<dyn ChainClient>,
) {
    registry.register(Box::new(ReferendumLookupTool::new(chain_client.clone())));
    registry.register(Box::new(TrackInfoTool::new(chain_client.clone())));
    registry.register(Box::new(VoterHistoryTool::new(chain_client.clone())));
    registry.register(Box::new(DelegationInfoTool::new(chain_client.clone())));
    registry.register(Box::new(TreasuryOverviewTool::new(chain_client)));
}

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
            let map = self
                .storage
                .lock()
                .map_err(|e| ChainError::Internal {
                    message: format!("lock poisoned: {e}"),
                })?;
            Ok(map.get(storage_key).cloned())
        }

        async fn dry_run_call(
            &self,
            _extrinsic: &[u8],
        ) -> Result<DryRunResult, ChainError> {
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

    #[test]
    fn register_governance_tools_adds_five() {
        let client = Arc::new(MockChainClient::new());
        let mut registry = ToolRegistry::new();
        register_governance_tools(&mut registry, client);
        assert_eq!(registry.len(), 5);
    }

    #[test]
    fn register_governance_tools_correct_names() {
        let client = Arc::new(MockChainClient::new());
        let mut registry = ToolRegistry::new();
        register_governance_tools(&mut registry, client);

        let names = registry.names();
        assert!(names.contains(&"polkagent.governance.referendum_lookup".to_string()));
        assert!(names.contains(&"polkagent.governance.track_info".to_string()));
        assert!(names.contains(&"polkagent.governance.voter_history".to_string()));
        assert!(names.contains(&"polkagent.governance.delegation_info".to_string()));
        assert!(names.contains(&"polkagent.governance.treasury_overview".to_string()));
    }

    #[test]
    fn all_tools_require_chain_query_grant() {
        let client = Arc::new(MockChainClient::new());
        let mut registry = ToolRegistry::new();
        register_governance_tools(&mut registry, client);

        for spec in registry.list() {
            assert_eq!(
                spec.required_grant,
                Some("chain.query".to_string()),
                "tool '{}' should require chain.query grant",
                spec.name
            );
        }
    }

    #[test]
    fn all_tools_have_valid_json_schema() {
        let client = Arc::new(MockChainClient::new());
        let mut registry = ToolRegistry::new();
        register_governance_tools(&mut registry, client);

        for spec in registry.list() {
            assert_eq!(
                spec.input_schema["type"], "object",
                "tool '{}' input schema must be an object type",
                spec.name
            );
            assert!(
                spec.input_schema.get("properties").is_some(),
                "tool '{}' input schema must have properties",
                spec.name
            );
        }
    }

    #[test]
    fn tool_definitions_convert() {
        let client = Arc::new(MockChainClient::new());
        let mut registry = ToolRegistry::new();
        register_governance_tools(&mut registry, client);

        let defs = registry.to_tool_definitions();
        assert_eq!(defs.len(), 5);

        for def in &defs {
            assert!(!def.name.is_empty());
            assert!(!def.description.is_empty());
            assert!(!def.input_schema_json.is_empty());
        }
    }

    #[test]
    fn double_registration_replaces() {
        let client = Arc::new(MockChainClient::new());
        let mut registry = ToolRegistry::new();
        register_governance_tools(&mut registry, client.clone());
        register_governance_tools(&mut registry, client);
        // Second registration replaces all five tools.
        assert_eq!(registry.len(), 5);
    }
}
