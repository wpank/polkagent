//! Referendum lookup tool.
//!
//! The [`ReferendumLookupTool`] queries a referendum by its index and returns
//! detailed information including status, track, tally, proposer, and timeline
//! events.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tracing::debug;

use polkagent_chain_trait::ChainClient;
use polkagent_core::config::DataClassification;
use polkagent_tool::registry::{ToolContext, ToolError, ToolHandler, ToolResult, ToolSpec};

use crate::error::GovernanceError;
use crate::types::Referendum;

// ---------------------------------------------------------------------------
// ReferendumLookupTool
// ---------------------------------------------------------------------------

/// Looks up an OpenGov referendum by its index.
///
/// Returns the referendum's status, track, tally (ayes/nays/support),
/// proposer, and timeline events. Requires `chain.query` grant for
/// read-only chain state access.
pub struct ReferendumLookupTool {
    chain_client: Arc<dyn ChainClient>,
}

impl ReferendumLookupTool {
    /// Create a new `ReferendumLookupTool` backed by the given chain client.
    pub fn new(chain_client: Arc<dyn ChainClient>) -> Self {
        Self { chain_client }
    }

    /// Query a referendum by index using the chain client.
    ///
    /// In a production implementation this would construct the appropriate
    /// storage key for `Referenda::ReferendumInfoFor(index)` and decode
    /// the SCALE-encoded response. The current implementation returns a
    /// placeholder that downstream integrations will replace with real
    /// chain queries.
    async fn lookup_referendum(&self, index: u32) -> Result<Referendum, GovernanceError> {
        // Verify chain connectivity.
        self.chain_client
            .health()
            .await
            .map_err(GovernanceError::Chain)?;

        // Build the storage key for Referenda::ReferendumInfoFor.
        // In production this would use proper SCALE encoding; here we
        // construct a representative key to query via the chain client.
        let storage_key = build_referendum_storage_key(index);

        let result = self
            .chain_client
            .query_storage(
                &storage_key,
                None,
                polkagent_chain_trait::ChainProfileId::new("default"),
            )
            .await
            .map_err(GovernanceError::Chain)?;

        match result {
            Some(bytes) => decode_referendum(index, &bytes),
            None => Err(GovernanceError::ReferendumNotFound { index }),
        }
    }
}

#[async_trait]
impl ToolHandler for ReferendumLookupTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "polkagent.governance.referendum_lookup".to_string(),
            description: "Look up an OpenGov referendum by index. Returns status, track, \
                          tally (ayes/nays/support), proposer, and timeline events."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "index": {
                        "type": "integer",
                        "description": "The referendum index to look up"
                    }
                },
                "required": ["index"]
            }),
            required_grant: Some("chain.query".to_string()),
            output_classification: DataClassification::Public,
        }
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let index =
            input
                .get("index")
                .and_then(Value::as_u64)
                .ok_or_else(|| ToolError::InvalidInput {
                    reason: "missing or invalid 'index' field — must be a non-negative integer"
                        .to_string(),
                })?;

        let index = u32::try_from(index).map_err(|_| ToolError::InvalidInput {
            reason: format!("referendum index {index} exceeds u32 range"),
        })?;

        debug!(
            referendum_index = index,
            agent = %context.agent_id,
            "looking up referendum"
        );

        let referendum =
            self.lookup_referendum(index)
                .await
                .map_err(|e| ToolError::ExecutionFailed {
                    reason: e.to_string(),
                })?;

        let output = serde_json::to_value(&referendum).map_err(|e| ToolError::ExecutionFailed {
            reason: format!("failed to serialize referendum: {e}"),
        })?;

        Ok(ToolResult {
            output,
            classification: DataClassification::Public,
            artifacts: vec![],
        })
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build the storage key for `Referenda::ReferendumInfoFor(index)`.
///
/// In a full implementation this would use the metadata-derived storage
/// prefix hash. Here we produce a deterministic key suitable for the
/// [`ChainClient::query_storage`] interface.
fn build_referendum_storage_key(index: u32) -> Vec<u8> {
    let mut key = b"Referenda:ReferendumInfoFor:".to_vec();
    key.extend_from_slice(&index.to_le_bytes());
    key
}

/// Decode SCALE-encoded referendum bytes into a [`Referendum`].
///
/// The current implementation performs a best-effort decode. Production
/// use should use the runtime metadata for accurate decoding.
fn decode_referendum(index: u32, bytes: &[u8]) -> Result<Referendum, GovernanceError> {
    // Minimal decode: in production this would parse the SCALE enum.
    // For now, attempt to interpret the bytes as a JSON-encoded referendum
    // (as a mock chain client would provide), falling back to a decode error.
    serde_json::from_slice(bytes).map_err(|e| GovernanceError::Decode {
        message: format!("failed to decode referendum {index}: {e}"),
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::MockChainClient;
    use crate::types::{ReferendumStatus, TimelineEvent};
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
        let tool = ReferendumLookupTool::new(client);
        let spec = tool.spec();
        assert_eq!(spec.name, "polkagent.governance.referendum_lookup");
    }

    #[test]
    fn spec_requires_chain_query_grant() {
        let client = Arc::new(MockChainClient::new());
        let tool = ReferendumLookupTool::new(client);
        let spec = tool.spec();
        assert_eq!(spec.required_grant, Some("chain.query".to_string()));
    }

    #[test]
    fn spec_has_index_in_schema() {
        let client = Arc::new(MockChainClient::new());
        let tool = ReferendumLookupTool::new(client);
        let spec = tool.spec();
        let props = &spec.input_schema["properties"];
        assert!(props.get("index").is_some());
        let required = spec.input_schema["required"].as_array();
        assert!(required.is_some());
        let required_names: Vec<&str> = required
            .unwrap_or_else(|| panic!("required should be an array"))
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert!(required_names.contains(&"index"));
    }

    #[tokio::test]
    async fn execute_missing_index() {
        let client = Arc::new(MockChainClient::new());
        let tool = ReferendumLookupTool::new(client);
        let result = tool.execute(serde_json::json!({}), &test_context()).await;
        assert!(matches!(result, Err(ToolError::InvalidInput { .. })));
    }

    #[tokio::test]
    async fn execute_invalid_index_type() {
        let client = Arc::new(MockChainClient::new());
        let tool = ReferendumLookupTool::new(client);
        let input = serde_json::json!({ "index": "not_a_number" });
        let result = tool.execute(input, &test_context()).await;
        assert!(matches!(result, Err(ToolError::InvalidInput { .. })));
    }

    #[tokio::test]
    async fn execute_referendum_not_found() {
        let client = Arc::new(MockChainClient::new());
        let tool = ReferendumLookupTool::new(client);
        // Index 999 is not in the mock data.
        let input = serde_json::json!({ "index": 999 });
        let result = tool.execute(input, &test_context()).await;
        assert!(matches!(result, Err(ToolError::ExecutionFailed { .. })));
    }

    #[tokio::test]
    async fn execute_success() {
        let mut client = MockChainClient::new();

        let ref_data = Referendum {
            index: 42,
            track: 1,
            origin: "SmallSpender".to_string(),
            status: ReferendumStatus::Deciding,
            ayes: 1_000_000,
            nays: 500_000,
            support: 1_500_000,
            proposer: "5GrwvaEF...".to_string(),
            timeline: vec![TimelineEvent {
                event: "submitted".to_string(),
                block_number: 10_000,
                timestamp: None,
            }],
        };
        let bytes = serde_json::to_vec(&ref_data).unwrap_or_else(|e| panic!("serialize ref: {e}"));
        let key = build_referendum_storage_key(42);
        client.insert_storage(key, bytes);

        let tool = ReferendumLookupTool::new(Arc::new(client));
        let input = serde_json::json!({ "index": 42 });
        let result = tool
            .execute(input, &test_context())
            .await
            .unwrap_or_else(|e| panic!("execute failed: {e}"));

        assert_eq!(result.output["index"], 42);
        assert_eq!(result.output["status"], "deciding");
        assert_eq!(result.output["track"], 1);
        assert_eq!(result.classification, DataClassification::Public);
    }

    #[test]
    fn build_storage_key_deterministic() {
        let k1 = build_referendum_storage_key(42);
        let k2 = build_referendum_storage_key(42);
        assert_eq!(k1, k2);
    }

    #[test]
    fn build_storage_key_different_indices() {
        let k1 = build_referendum_storage_key(1);
        let k2 = build_referendum_storage_key(2);
        assert_ne!(k1, k2);
    }
}
