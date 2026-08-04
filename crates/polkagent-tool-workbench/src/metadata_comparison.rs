//! Metadata comparison tool (PRD-05 workflow A2).
//!
//! The [`MetadataComparisonTool`] accepts two metadata snapshots — identified
//! by block number or raw-bytes file path — parses them via
//! [`polkagent_codec::parse_metadata`], diffs pallets/calls/events/storage/
//! constants, and produces a structured [`MetadataComparisonReport`] listing
//! every change with before/after detail.
//!
//! This is a **read-only** tool — it produces zero write effects.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tracing::debug;

use polkagent_chain_trait::ChainClient;
use polkagent_codec::parse_metadata;
use polkagent_core::config::DataClassification;
use polkagent_metadata::diff::{diff_metadata, generate_impact_brief, is_breaking};
use polkagent_tool::registry::{ToolContext, ToolError, ToolHandler, ToolResult, ToolSpec};

use crate::error::WorkbenchError;
use crate::types::{ComparisonSummary, MetadataComparisonReport, PalletComparisonDetail};

// ---------------------------------------------------------------------------
// MetadataComparisonTool
// ---------------------------------------------------------------------------

/// Compares two runtime metadata snapshots and produces a structured diff.
///
/// Accepts metadata as either raw-bytes file paths or block numbers (fetched
/// via the chain client). Compares pallets, calls, events, storage items,
/// and constants, listing all changes with before/after type signatures.
///
/// Requires `chain.query` grant for read-only chain state access.
pub struct MetadataComparisonTool {
    chain_client: Arc<dyn ChainClient>,
}

impl MetadataComparisonTool {
    /// Create a new `MetadataComparisonTool` backed by the given chain client.
    pub fn new(chain_client: Arc<dyn ChainClient>) -> Self {
        Self { chain_client }
    }

    async fn load_metadata(&self, source: &MetadataSource) -> Result<Vec<u8>, WorkbenchError> {
        match source {
            MetadataSource::FilePath(path) => {
                tokio::fs::read(path)
                    .await
                    .map_err(|e| WorkbenchError::Decode {
                        message: format!("failed to read metadata file '{path}': {e}"),
                    })
            }
            MetadataSource::BlockNumber(block) => {
                let key = build_metadata_key(*block);
                let result = self
                    .chain_client
                    .query_storage(
                        &key,
                        None,
                        polkagent_chain_trait::ChainProfileId::new("default"),
                    )
                    .await
                    .map_err(WorkbenchError::Chain)?;
                result.ok_or_else(|| WorkbenchError::Decode {
                    message: format!("no metadata found for block {block}"),
                })
            }
        }
    }

    async fn compare(
        &self,
        old_source: &MetadataSource,
        new_source: &MetadataSource,
    ) -> Result<MetadataComparisonReport, WorkbenchError> {
        let old_bytes = self.load_metadata(old_source).await?;
        let new_bytes = self.load_metadata(new_source).await?;

        let old_meta = parse_metadata(&old_bytes).map_err(|e| WorkbenchError::Decode {
            message: format!("failed to parse old metadata: {e}"),
        })?;
        let new_meta = parse_metadata(&new_bytes).map_err(|e| WorkbenchError::Decode {
            message: format!("failed to parse new metadata: {e}"),
        })?;

        let diff = diff_metadata(&old_meta, &new_meta);
        let breaking = is_breaking(&diff);
        let brief = generate_impact_brief(&diff);

        let modified_pallets: Vec<PalletComparisonDetail> = diff
            .modified_pallets
            .iter()
            .map(|pd| PalletComparisonDetail {
                name: pd.name.clone(),
                added_calls: pd.added_calls.clone(),
                removed_calls: pd.removed_calls.clone(),
                changed_call_signatures: pd.changed_call_signatures.clone(),
                added_storage: pd.added_storage.clone(),
                removed_storage: pd.removed_storage.clone(),
                added_constants: pd.added_constants.clone(),
                removed_constants: pd.removed_constants.clone(),
                changed_constants: pd.changed_constants.clone(),
            })
            .collect();

        let summary = ComparisonSummary {
            pallets_added: diff.added_pallets.len(),
            pallets_removed: diff.removed_pallets.len(),
            pallets_modified: diff.modified_pallets.len(),
            calls_added: diff.added_calls.len(),
            calls_removed: diff.removed_calls.len(),
            calls_changed: diff
                .modified_pallets
                .iter()
                .map(|p| p.changed_call_signatures.len())
                .sum(),
            events_added: diff.added_events.len(),
            events_removed: diff.removed_events.len(),
            storage_added: diff.added_storage.len(),
            storage_removed: diff.removed_storage.len(),
            constants_added: diff.added_constants.len(),
            constants_removed: diff.removed_constants.len(),
            constants_changed: diff.changed_constants.len(),
            breaking_change_count: diff.breaking_changes.len(),
        };

        Ok(MetadataComparisonReport {
            old_source: old_source.to_string(),
            new_source: new_source.to_string(),
            is_breaking: breaking,
            added_pallets: diff.added_pallets,
            removed_pallets: diff.removed_pallets,
            modified_pallets,
            summary,
            impact_brief: brief,
        })
    }
}

#[async_trait]
impl ToolHandler for MetadataComparisonTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "polkagent.workbench.metadata_comparison".to_string(),
            description: "Compare two runtime metadata snapshots to produce an upgrade \
                          impact brief. Accepts old and new metadata by block number or \
                          file path. Diffs pallets, calls, events, storage items, and \
                          constants, listing all changes with before/after type signatures. \
                          Read-only — no chain state is modified."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "old_block_number": {
                        "type": "integer",
                        "description": "Block number for the old (baseline) metadata snapshot"
                    },
                    "new_block_number": {
                        "type": "integer",
                        "description": "Block number for the new (upgraded) metadata snapshot"
                    },
                    "old_file_path": {
                        "type": "string",
                        "description": "File path to raw SCALE-encoded old metadata bytes"
                    },
                    "new_file_path": {
                        "type": "string",
                        "description": "File path to raw SCALE-encoded new metadata bytes"
                    }
                }
            }),
            required_grant: Some("chain.query".to_string()),
            output_classification: DataClassification::Internal,
        }
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let old_source = parse_source(&input, "old_block_number", "old_file_path", "old")?;
        let new_source = parse_source(&input, "new_block_number", "new_file_path", "new")?;

        debug!(
            old = %old_source,
            new = %new_source,
            agent = %context.agent_id,
            "comparing metadata snapshots"
        );

        let report = self.compare(&old_source, &new_source).await.map_err(|e| {
            ToolError::ExecutionFailed {
                reason: e.to_string(),
            }
        })?;

        let output = serde_json::to_value(&report).map_err(|e| ToolError::ExecutionFailed {
            reason: format!("failed to serialize comparison report: {e}"),
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

enum MetadataSource {
    FilePath(String),
    BlockNumber(u64),
}

impl std::fmt::Display for MetadataSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::FilePath(p) => write!(f, "file:{p}"),
            Self::BlockNumber(n) => write!(f, "block:{n}"),
        }
    }
}

fn parse_source(
    input: &Value,
    block_key: &str,
    file_key: &str,
    label: &str,
) -> Result<MetadataSource, ToolError> {
    if let Some(block) = input.get(block_key).and_then(Value::as_u64) {
        return Ok(MetadataSource::BlockNumber(block));
    }
    if let Some(path) = input.get(file_key).and_then(Value::as_str) {
        if path.is_empty() {
            return Err(ToolError::InvalidInput {
                reason: format!("'{file_key}' must not be empty"),
            });
        }
        return Ok(MetadataSource::FilePath(path.to_string()));
    }
    Err(ToolError::InvalidInput {
        reason: format!(
            "must provide either '{block_key}' (integer) or '{file_key}' (string) for the {label} metadata"
        ),
    })
}

fn build_metadata_key(block_number: u64) -> Vec<u8> {
    let mut key = b"RuntimeMetadata:".to_vec();
    key.extend_from_slice(&block_number.to_le_bytes());
    key
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
        }
    }

    #[test]
    fn spec_has_correct_name() {
        let client = Arc::new(MockChainClient::new());
        let tool = MetadataComparisonTool::new(client);
        let spec = tool.spec();
        assert_eq!(spec.name, "polkagent.workbench.metadata_comparison");
    }

    #[test]
    fn spec_requires_chain_query_grant() {
        let client = Arc::new(MockChainClient::new());
        let tool = MetadataComparisonTool::new(client);
        let spec = tool.spec();
        assert_eq!(spec.required_grant, Some("chain.query".to_string()));
    }

    #[test]
    fn spec_has_input_properties() {
        let client = Arc::new(MockChainClient::new());
        let tool = MetadataComparisonTool::new(client);
        let spec = tool.spec();
        let props = &spec.input_schema["properties"];
        assert!(props.get("old_block_number").is_some());
        assert!(props.get("new_block_number").is_some());
        assert!(props.get("old_file_path").is_some());
        assert!(props.get("new_file_path").is_some());
    }

    #[test]
    fn spec_output_classification_is_internal() {
        let client = Arc::new(MockChainClient::new());
        let tool = MetadataComparisonTool::new(client);
        let spec = tool.spec();
        assert_eq!(spec.output_classification, DataClassification::Internal);
    }

    #[tokio::test]
    async fn execute_missing_both_sources() {
        let client = Arc::new(MockChainClient::new());
        let tool = MetadataComparisonTool::new(client);
        let input = serde_json::json!({});
        let result = tool.execute(input, &test_context()).await;
        assert!(matches!(result, Err(ToolError::InvalidInput { .. })));
    }

    #[tokio::test]
    async fn execute_missing_new_source() {
        let client = Arc::new(MockChainClient::new());
        let tool = MetadataComparisonTool::new(client);
        let input = serde_json::json!({ "old_block_number": 100 });
        let result = tool.execute(input, &test_context()).await;
        assert!(matches!(result, Err(ToolError::InvalidInput { .. })));
    }

    #[tokio::test]
    async fn execute_empty_file_path_rejected() {
        let client = Arc::new(MockChainClient::new());
        let tool = MetadataComparisonTool::new(client);
        let input = serde_json::json!({
            "old_file_path": "",
            "new_block_number": 200
        });
        let result = tool.execute(input, &test_context()).await;
        assert!(matches!(result, Err(ToolError::InvalidInput { .. })));
    }

    #[test]
    fn parse_source_block_number() {
        let input = serde_json::json!({ "old_block_number": 42 });
        let src = parse_source(&input, "old_block_number", "old_file_path", "old");
        assert!(src.is_ok());
        assert_eq!(
            src.unwrap_or_else(|e| panic!("{e}")).to_string(),
            "block:42"
        );
    }

    #[test]
    fn parse_source_file_path() {
        let input = serde_json::json!({ "old_file_path": "/tmp/meta.bin" });
        let src = parse_source(&input, "old_block_number", "old_file_path", "old");
        assert!(src.is_ok());
        assert_eq!(
            src.unwrap_or_else(|e| panic!("{e}")).to_string(),
            "file:/tmp/meta.bin"
        );
    }

    #[test]
    fn parse_source_prefers_block_over_file() {
        let input = serde_json::json!({
            "old_block_number": 10,
            "old_file_path": "/tmp/meta.bin"
        });
        let src = parse_source(&input, "old_block_number", "old_file_path", "old");
        assert!(src.is_ok());
        assert_eq!(
            src.unwrap_or_else(|e| panic!("{e}")).to_string(),
            "block:10"
        );
    }

    #[test]
    fn parse_source_missing_both_errors() {
        let input = serde_json::json!({});
        let src = parse_source(&input, "old_block_number", "old_file_path", "old");
        assert!(src.is_err());
    }

    #[test]
    fn build_metadata_key_deterministic() {
        let k1 = build_metadata_key(100);
        let k2 = build_metadata_key(100);
        assert_eq!(k1, k2);
    }

    #[test]
    fn build_metadata_key_different_blocks() {
        let k1 = build_metadata_key(100);
        let k2 = build_metadata_key(200);
        assert_ne!(k1, k2);
    }

    #[test]
    fn metadata_source_display() {
        let file = MetadataSource::FilePath("/tmp/meta.bin".to_string());
        assert_eq!(file.to_string(), "file:/tmp/meta.bin");

        let block = MetadataSource::BlockNumber(42);
        assert_eq!(block.to_string(), "block:42");
    }

    #[test]
    fn comparison_report_serializes() {
        let report = MetadataComparisonReport {
            old_source: "block:100".to_string(),
            new_source: "block:200".to_string(),
            is_breaking: true,
            added_pallets: vec!["NewPallet".to_string()],
            removed_pallets: vec!["OldPallet".to_string()],
            modified_pallets: vec![PalletComparisonDetail {
                name: "Balances".to_string(),
                added_calls: vec!["transfer_all".to_string()],
                removed_calls: vec![],
                changed_call_signatures: vec![],
                added_storage: vec![],
                removed_storage: vec![],
                added_constants: vec!["MaxLocks".to_string()],
                removed_constants: vec![],
                changed_constants: vec![],
            }],
            summary: ComparisonSummary {
                pallets_added: 1,
                pallets_removed: 1,
                pallets_modified: 1,
                calls_added: 1,
                calls_removed: 0,
                calls_changed: 0,
                events_added: 0,
                events_removed: 0,
                storage_added: 0,
                storage_removed: 0,
                constants_added: 1,
                constants_removed: 0,
                constants_changed: 0,
                breaking_change_count: 1,
            },
            impact_brief: "BREAKING runtime upgrade detected.".to_string(),
        };
        let json = serde_json::to_value(&report).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(json["is_breaking"], true);
        assert_eq!(json["added_pallets"][0], "NewPallet");
        assert_eq!(json["summary"]["pallets_added"], 1);
        assert_eq!(json["summary"]["constants_added"], 1);
    }
}
