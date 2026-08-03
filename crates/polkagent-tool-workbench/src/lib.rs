//! `polkagent-tool-workbench` — Product-engineering workbench tools for
//! Polkagent agents.
//!
//! This crate provides read-only tools for builder workflows defined in
//! PRD-05, starting with storage migration rehearsal (workflow A1).
//!
//! # Tools
//!
//! | Tool | Description |
//! |------|-------------|
//! | [`migration_rehearsal::MigrationRehearsalTool`] | Rehearse a storage migration and produce a structured diff |
//!
//! # Registration
//!
//! Use [`register_workbench_tools`] to add all workbench tools to a
//! [`ToolRegistry`] in one call:
//!
//! ```rust,no_run
//! use std::sync::Arc;
//! use polkagent_tool::ToolRegistry;
//! use polkagent_tool_workbench::register_workbench_tools;
//!
//! # fn example(chain_client: Arc<dyn polkagent_chain_trait::ChainClient>) {
//! let mut registry = ToolRegistry::new();
//! register_workbench_tools(&mut registry, chain_client);
//! assert_eq!(registry.len(), 1);
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

pub mod error;
pub mod migration_rehearsal;
pub mod types;

use std::sync::Arc;

use polkagent_chain_trait::ChainClient;
use polkagent_tool::ToolRegistry;

// ---------------------------------------------------------------------------
// Convenience re-exports
// ---------------------------------------------------------------------------

pub use error::WorkbenchError;
pub use migration_rehearsal::MigrationRehearsalTool;
pub use types::{MigrationRehearsalReport, StorageDiff, StorageKeyChange};

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// Register all workbench tools into the given registry.
///
/// This adds the migration rehearsal tool. A [`ChainClient`] must be
/// provided so the tools can query chain state.
///
/// Call this once when setting up a new run or agent context.
pub fn register_workbench_tools(
    registry: &mut ToolRegistry,
    chain_client: Arc<dyn ChainClient>,
) {
    registry.register(Box::new(MigrationRehearsalTool::new(chain_client)));
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
    fn register_workbench_tools_adds_one() {
        let client = Arc::new(MockChainClient::new());
        let mut registry = ToolRegistry::new();
        register_workbench_tools(&mut registry, client);
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn register_workbench_tools_correct_name() {
        let client = Arc::new(MockChainClient::new());
        let mut registry = ToolRegistry::new();
        register_workbench_tools(&mut registry, client);

        let names = registry.names();
        assert!(names.contains(&"polkagent.workbench.migration_rehearsal".to_string()));
    }

    #[test]
    fn all_tools_require_chain_query_grant() {
        let client = Arc::new(MockChainClient::new());
        let mut registry = ToolRegistry::new();
        register_workbench_tools(&mut registry, client);

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
        register_workbench_tools(&mut registry, client);

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
        register_workbench_tools(&mut registry, client);

        let defs = registry.to_tool_definitions();
        assert_eq!(defs.len(), 1);

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
        register_workbench_tools(&mut registry, client.clone());
        register_workbench_tools(&mut registry, client);
        assert_eq!(registry.len(), 1);
    }
}
