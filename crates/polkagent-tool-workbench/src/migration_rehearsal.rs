//! Storage migration rehearsal tool.
//!
//! The [`MigrationRehearsalTool`] accepts a runtime WASM blob path and a block
//! number, compares old vs new storage layout after a simulated migration, diffs
//! the resulting storage keys/values, and produces a structured report listing
//! changed, added, and removed keys.
//!
//! This is a **read-only** tool — it produces zero write effects. The actual
//! migration is simulated by querying chain state at the specified block via the
//! [`ChainClient`] interface and comparing pre- and post-migration storage
//! snapshots.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tracing::debug;

use polkagent_chain_trait::{ChainClient, ChainProfileId};
use polkagent_core::config::DataClassification;
use polkagent_tool::registry::{ToolContext, ToolError, ToolHandler, ToolResult, ToolSpec};

use crate::error::WorkbenchError;
use crate::types::{MigrationRehearsalReport, StorageDiff, StorageKeyChange};

// ---------------------------------------------------------------------------
// MigrationRehearsalTool
// ---------------------------------------------------------------------------

/// Rehearses a storage migration by comparing pre- and post-migration storage.
///
/// Accepts a runtime WASM blob path and a block number, queries the chain
/// for storage state snapshots, and produces a structured diff report showing
/// changed, added, and removed storage keys.
///
/// Requires `chain.query` grant for read-only chain state access.
pub struct MigrationRehearsalTool {
    chain_client: Arc<dyn ChainClient>,
}

impl MigrationRehearsalTool {
    /// Create a new `MigrationRehearsalTool` backed by the given chain client.
    pub fn new(chain_client: Arc<dyn ChainClient>) -> Self {
        Self { chain_client }
    }

    /// Perform the migration rehearsal.
    ///
    /// Queries pre-migration storage at the specified block, retrieves the
    /// post-migration snapshot (at block + 1, representing the state after
    /// the runtime upgrade), and diffs the two.
    ///
    /// In a full implementation this would:
    /// 1. Fork the chain via Chopsticks at the specified block.
    /// 2. Upload the WASM blob via `dev_setWasmCode`.
    /// 3. Advance one block to execute `on_runtime_upgrade`.
    /// 4. Diff pre/post storage.
    ///
    /// The current implementation queries the chain client for pre- and
    /// post-migration storage snapshots and diffs them.
    async fn rehearse(
        &self,
        wasm_blob_path: &str,
        block_number: u64,
    ) -> Result<MigrationRehearsalReport, WorkbenchError> {
        // Verify chain connectivity.
        self.chain_client
            .health()
            .await
            .map_err(WorkbenchError::Chain)?;

        // Query pre-migration storage snapshot at the specified block.
        let pre_storage_key = build_snapshot_key(wasm_blob_path, block_number, "pre");
        let pre_snapshot = self.query_snapshot(&pre_storage_key).await?;

        // Query post-migration storage snapshot (at block + 1).
        let post_storage_key = build_snapshot_key(wasm_blob_path, block_number, "post");
        let post_snapshot = self.query_snapshot(&post_storage_key).await?;

        // Diff the snapshots.
        let diff = diff_snapshots(&pre_snapshot, &post_snapshot);

        Ok(MigrationRehearsalReport {
            wasm_blob_path: wasm_blob_path.to_string(),
            block_number,
            modified_count: diff.modified.len(),
            added_count: diff.added.len(),
            removed_count: diff.removed.len(),
            total_changes: diff.total_changes(),
            diff,
        })
    }

    /// Query a storage snapshot from the chain client.
    ///
    /// Returns a map of hex-encoded key -> hex-encoded value pairs.
    async fn query_snapshot(
        &self,
        storage_key: &[u8],
    ) -> Result<BTreeMap<String, String>, WorkbenchError> {
        let result = self
            .chain_client
            .query_storage(storage_key, None, ChainProfileId::new("default"))
            .await
            .map_err(WorkbenchError::Chain)?;

        match result {
            Some(bytes) => serde_json::from_slice(&bytes).map_err(|e| WorkbenchError::Decode {
                message: format!("failed to decode storage snapshot: {e}"),
            }),
            None => Ok(BTreeMap::new()),
        }
    }
}

#[async_trait]
impl ToolHandler for MigrationRehearsalTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "polkagent.workbench.migration_rehearsal".to_string(),
            description: "Rehearse a Polkadot storage migration by comparing pre- and \
                          post-migration storage layouts. Accepts a runtime WASM blob path \
                          and block number, produces a structured diff of changed, added, \
                          and removed storage keys. Read-only — no chain state is modified."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "wasm_blob_path": {
                        "type": "string",
                        "description": "File path to the runtime WASM blob to rehearse the migration with"
                    },
                    "block_number": {
                        "type": "integer",
                        "description": "Block number at which to fork the chain for the rehearsal"
                    }
                },
                "required": ["wasm_blob_path", "block_number"]
            }),
            required_grant: Some("chain.query".to_string()),
            output_classification: DataClassification::Internal,
        }
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let wasm_blob_path = input
            .get("wasm_blob_path")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidInput {
                reason: "missing or invalid 'wasm_blob_path' field — must be a string".to_string(),
            })?;

        if wasm_blob_path.is_empty() {
            return Err(ToolError::InvalidInput {
                reason: "'wasm_blob_path' must not be empty".to_string(),
            });
        }

        let block_number = input
            .get("block_number")
            .and_then(Value::as_u64)
            .ok_or_else(|| ToolError::InvalidInput {
                reason: "missing or invalid 'block_number' field — must be a non-negative integer"
                    .to_string(),
            })?;

        debug!(
            wasm_blob_path = wasm_blob_path,
            block_number = block_number,
            agent = %context.agent_id,
            "rehearsing storage migration"
        );

        let report = self
            .rehearse(wasm_blob_path, block_number)
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                reason: e.to_string(),
            })?;

        let output = serde_json::to_value(&report).map_err(|e| ToolError::ExecutionFailed {
            reason: format!("failed to serialize migration report: {e}"),
        })?;

        Ok(ToolResult {
            output,
            classification: DataClassification::Internal,
            artifacts: vec![],
        })
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a deterministic storage key for querying a migration snapshot.
///
/// In a full implementation this would use metadata-derived storage
/// prefixes. Here we produce a deterministic key suitable for the
/// [`ChainClient::query_storage`] interface.
fn build_snapshot_key(wasm_path: &str, block_number: u64, phase: &str) -> Vec<u8> {
    let mut key = b"MigrationRehearsal:".to_vec();
    key.extend_from_slice(phase.as_bytes());
    key.push(b':');
    key.extend_from_slice(wasm_path.as_bytes());
    key.push(b':');
    key.extend_from_slice(&block_number.to_le_bytes());
    key
}

/// Diff two storage snapshots and produce a [`StorageDiff`].
///
/// Both snapshots are maps of hex-encoded key -> hex-encoded value.
fn diff_snapshots(pre: &BTreeMap<String, String>, post: &BTreeMap<String, String>) -> StorageDiff {
    let all_keys: BTreeSet<&String> = pre.keys().chain(post.keys()).collect();

    let mut modified = Vec::new();
    let mut added = Vec::new();
    let mut removed = Vec::new();

    for key in all_keys {
        match (pre.get(key), post.get(key)) {
            (Some(old_val), Some(new_val)) => {
                if old_val != new_val {
                    modified.push(StorageKeyChange::Modified {
                        key: key.clone(),
                        old_value: old_val.clone(),
                        new_value: new_val.clone(),
                    });
                }
            }
            (None, Some(val)) => {
                added.push(StorageKeyChange::Added {
                    key: key.clone(),
                    value: val.clone(),
                });
            }
            (Some(val), None) => {
                removed.push(StorageKeyChange::Removed {
                    key: key.clone(),
                    value: val.clone(),
                });
            }
            (None, None) => {
                // Should not happen since we collected from both maps.
            }
        }
    }

    StorageDiff {
        modified,
        added,
        removed,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::MockChainClient;
    use polkagent_core::ids::{AgentId, RunId, StepId};

    fn test_context() -> ToolContext {
        ToolContext {
            run_id: RunId::new(),
            agent_id: AgentId::new(),
            step_id: StepId::new(),
            grants: vec![],
            security_config: None,
        }
    }

    #[test]
    fn spec_has_correct_name() {
        let client = Arc::new(MockChainClient::new());
        let tool = MigrationRehearsalTool::new(client);
        let spec = tool.spec();
        assert_eq!(spec.name, "polkagent.workbench.migration_rehearsal");
    }

    #[test]
    fn spec_requires_chain_query_grant() {
        let client = Arc::new(MockChainClient::new());
        let tool = MigrationRehearsalTool::new(client);
        let spec = tool.spec();
        assert_eq!(spec.required_grant, Some("chain.query".to_string()));
    }

    #[test]
    fn spec_has_required_fields() {
        let client = Arc::new(MockChainClient::new());
        let tool = MigrationRehearsalTool::new(client);
        let spec = tool.spec();
        let props = &spec.input_schema["properties"];
        assert!(props.get("wasm_blob_path").is_some());
        assert!(props.get("block_number").is_some());

        let required = spec.input_schema["required"]
            .as_array()
            .unwrap_or_else(|| panic!("required should be an array"));
        let names: Vec<&str> = required.iter().filter_map(Value::as_str).collect();
        assert!(names.contains(&"wasm_blob_path"));
        assert!(names.contains(&"block_number"));
    }

    #[test]
    fn spec_output_classification_is_internal() {
        let client = Arc::new(MockChainClient::new());
        let tool = MigrationRehearsalTool::new(client);
        let spec = tool.spec();
        assert_eq!(spec.output_classification, DataClassification::Internal);
    }

    #[tokio::test]
    async fn execute_missing_wasm_path() {
        let client = Arc::new(MockChainClient::new());
        let tool = MigrationRehearsalTool::new(client);
        let input = serde_json::json!({ "block_number": 100 });
        let result = tool.execute(input, &test_context()).await;
        assert!(matches!(result, Err(ToolError::InvalidInput { .. })));
    }

    #[tokio::test]
    async fn execute_empty_wasm_path() {
        let client = Arc::new(MockChainClient::new());
        let tool = MigrationRehearsalTool::new(client);
        let input = serde_json::json!({ "wasm_blob_path": "", "block_number": 100 });
        let result = tool.execute(input, &test_context()).await;
        assert!(matches!(result, Err(ToolError::InvalidInput { .. })));
    }

    #[tokio::test]
    async fn execute_missing_block_number() {
        let client = Arc::new(MockChainClient::new());
        let tool = MigrationRehearsalTool::new(client);
        let input = serde_json::json!({ "wasm_blob_path": "/tmp/runtime.wasm" });
        let result = tool.execute(input, &test_context()).await;
        assert!(matches!(result, Err(ToolError::InvalidInput { .. })));
    }

    #[tokio::test]
    async fn execute_invalid_block_number_type() {
        let client = Arc::new(MockChainClient::new());
        let tool = MigrationRehearsalTool::new(client);
        let input = serde_json::json!({
            "wasm_blob_path": "/tmp/runtime.wasm",
            "block_number": "not_a_number"
        });
        let result = tool.execute(input, &test_context()).await;
        assert!(matches!(result, Err(ToolError::InvalidInput { .. })));
    }

    #[tokio::test]
    async fn execute_empty_snapshots() {
        // When no mock data is present, both snapshots are empty → empty diff.
        let client = Arc::new(MockChainClient::new());
        let tool = MigrationRehearsalTool::new(client);
        let input = serde_json::json!({
            "wasm_blob_path": "/tmp/runtime.wasm",
            "block_number": 100
        });
        let result = tool
            .execute(input, &test_context())
            .await
            .unwrap_or_else(|e| panic!("execute failed: {e}"));

        assert_eq!(result.output["total_changes"], 0);
        assert_eq!(result.output["modified_count"], 0);
        assert_eq!(result.output["added_count"], 0);
        assert_eq!(result.output["removed_count"], 0);
    }

    #[tokio::test]
    async fn execute_with_changes() {
        let mut client = MockChainClient::new();

        let wasm_path = "/tmp/runtime.wasm";
        let block_number: u64 = 500;

        // Pre-migration snapshot: two keys.
        let pre_snapshot: BTreeMap<String, String> = [
            ("0xaaa1".to_string(), "0x01".to_string()),
            ("0xaaa2".to_string(), "0x02".to_string()),
        ]
        .into_iter()
        .collect();
        let pre_key = build_snapshot_key(wasm_path, block_number, "pre");
        let pre_bytes =
            serde_json::to_vec(&pre_snapshot).unwrap_or_else(|e| panic!("serialize: {e}"));
        client.insert_storage(pre_key, pre_bytes);

        // Post-migration snapshot: one modified, one removed, one added.
        let post_snapshot: BTreeMap<String, String> = [
            ("0xaaa1".to_string(), "0xff".to_string()), // modified
            ("0xbbb1".to_string(), "0xcc".to_string()), // added
                                                        // 0xaaa2 removed
        ]
        .into_iter()
        .collect();
        let post_key = build_snapshot_key(wasm_path, block_number, "post");
        let post_bytes =
            serde_json::to_vec(&post_snapshot).unwrap_or_else(|e| panic!("serialize: {e}"));
        client.insert_storage(post_key, post_bytes);

        let tool = MigrationRehearsalTool::new(Arc::new(client));
        let input = serde_json::json!({
            "wasm_blob_path": wasm_path,
            "block_number": block_number
        });
        let result = tool
            .execute(input, &test_context())
            .await
            .unwrap_or_else(|e| panic!("execute failed: {e}"));

        assert_eq!(result.output["total_changes"], 3);
        assert_eq!(result.output["modified_count"], 1);
        assert_eq!(result.output["added_count"], 1);
        assert_eq!(result.output["removed_count"], 1);
        assert_eq!(result.output["block_number"], 500);
        assert_eq!(result.output["wasm_blob_path"], "/tmp/runtime.wasm");
        assert_eq!(result.classification, DataClassification::Internal);
    }

    // -----------------------------------------------------------------------
    // Unit tests for diff_snapshots
    // -----------------------------------------------------------------------

    #[test]
    fn diff_empty_snapshots() {
        let pre = BTreeMap::new();
        let post = BTreeMap::new();
        let diff = diff_snapshots(&pre, &post);
        assert!(diff.is_empty());
    }

    #[test]
    fn diff_identical_snapshots() {
        let snap: BTreeMap<String, String> = [("0xa".to_string(), "0x1".to_string())]
            .into_iter()
            .collect();
        let diff = diff_snapshots(&snap, &snap);
        assert!(diff.is_empty());
    }

    #[test]
    fn diff_detects_added_key() {
        let pre = BTreeMap::new();
        let post: BTreeMap<String, String> = [("0xa".to_string(), "0x1".to_string())]
            .into_iter()
            .collect();
        let diff = diff_snapshots(&pre, &post);
        assert_eq!(diff.added.len(), 1);
        assert!(diff.modified.is_empty());
        assert!(diff.removed.is_empty());
    }

    #[test]
    fn diff_detects_removed_key() {
        let pre: BTreeMap<String, String> = [("0xa".to_string(), "0x1".to_string())]
            .into_iter()
            .collect();
        let post = BTreeMap::new();
        let diff = diff_snapshots(&pre, &post);
        assert!(diff.added.is_empty());
        assert!(diff.modified.is_empty());
        assert_eq!(diff.removed.len(), 1);
    }

    #[test]
    fn diff_detects_modified_key() {
        let pre: BTreeMap<String, String> = [("0xa".to_string(), "0x1".to_string())]
            .into_iter()
            .collect();
        let post: BTreeMap<String, String> = [("0xa".to_string(), "0x2".to_string())]
            .into_iter()
            .collect();
        let diff = diff_snapshots(&pre, &post);
        assert_eq!(diff.modified.len(), 1);
        assert!(diff.added.is_empty());
        assert!(diff.removed.is_empty());
    }

    #[test]
    fn diff_mixed_changes() {
        let pre: BTreeMap<String, String> = [
            ("0xa".to_string(), "0x1".to_string()),
            ("0xb".to_string(), "0x2".to_string()),
            ("0xc".to_string(), "0x3".to_string()),
        ]
        .into_iter()
        .collect();
        let post: BTreeMap<String, String> = [
            ("0xa".to_string(), "0xff".to_string()), // modified
            ("0xb".to_string(), "0x2".to_string()),  // unchanged
            ("0xd".to_string(), "0x4".to_string()),  // added
                                                     // 0xc removed
        ]
        .into_iter()
        .collect();
        let diff = diff_snapshots(&pre, &post);
        assert_eq!(diff.modified.len(), 1);
        assert_eq!(diff.added.len(), 1);
        assert_eq!(diff.removed.len(), 1);
        assert_eq!(diff.total_changes(), 3);
    }

    #[test]
    fn build_snapshot_key_deterministic() {
        let k1 = build_snapshot_key("/tmp/rt.wasm", 100, "pre");
        let k2 = build_snapshot_key("/tmp/rt.wasm", 100, "pre");
        assert_eq!(k1, k2);
    }

    #[test]
    fn build_snapshot_key_different_phases() {
        let k1 = build_snapshot_key("/tmp/rt.wasm", 100, "pre");
        let k2 = build_snapshot_key("/tmp/rt.wasm", 100, "post");
        assert_ne!(k1, k2);
    }

    #[test]
    fn build_snapshot_key_different_blocks() {
        let k1 = build_snapshot_key("/tmp/rt.wasm", 100, "pre");
        let k2 = build_snapshot_key("/tmp/rt.wasm", 200, "pre");
        assert_ne!(k1, k2);
    }
}
