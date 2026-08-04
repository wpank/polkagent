//! Track info tool.
//!
//! The [`TrackInfoTool`] lists governance tracks with their parameters
//! (decision period, confirmation period, approval/support curves, etc.)
//! or retrieves details for a specific track by ID.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tracing::debug;

use polkagent_chain_trait::ChainClient;
use polkagent_core::config::DataClassification;
use polkagent_tool::registry::{ToolContext, ToolError, ToolHandler, ToolResult, ToolSpec};

use crate::error::GovernanceError;
use crate::types::Track;

// ---------------------------------------------------------------------------
// TrackInfoTool
// ---------------------------------------------------------------------------

/// Lists OpenGov governance tracks and their parameters.
///
/// When called without a `track_id`, returns all tracks. When called with
/// a specific `track_id`, returns that single track's details.
///
/// Requires `chain.query` grant for read-only chain state access.
pub struct TrackInfoTool {
    chain_client: Arc<dyn ChainClient>,
}

impl TrackInfoTool {
    /// Create a new `TrackInfoTool` backed by the given chain client.
    pub fn new(chain_client: Arc<dyn ChainClient>) -> Self {
        Self { chain_client }
    }

    /// Query all tracks from chain state.
    async fn list_tracks(&self) -> Result<Vec<Track>, GovernanceError> {
        self.chain_client
            .health()
            .await
            .map_err(GovernanceError::Chain)?;

        let storage_key = b"Referenda:Tracks".to_vec();

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
            Some(bytes) => serde_json::from_slice(&bytes).map_err(|e| GovernanceError::Decode {
                message: format!("failed to decode tracks: {e}"),
            }),
            None => Ok(vec![]),
        }
    }

    /// Query a specific track by ID.
    async fn get_track(&self, track_id: u16) -> Result<Track, GovernanceError> {
        self.chain_client
            .health()
            .await
            .map_err(GovernanceError::Chain)?;

        let mut storage_key = b"Referenda:Tracks:".to_vec();
        storage_key.extend_from_slice(&track_id.to_le_bytes());

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
            Some(bytes) => serde_json::from_slice(&bytes).map_err(|e| GovernanceError::Decode {
                message: format!("failed to decode track {track_id}: {e}"),
            }),
            None => Err(GovernanceError::TrackNotFound { id: track_id }),
        }
    }
}

#[async_trait]
impl ToolHandler for TrackInfoTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "polkagent.governance.track_info".to_string(),
            description: "List OpenGov governance tracks with parameters (decision period, \
                          confirmation period, approval/support curves). Optionally filter \
                          by track ID."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "track_id": {
                        "type": "integer",
                        "description": "Optional track ID to look up a specific track. \
                                        If omitted, all tracks are returned."
                    }
                }
            }),
            required_grant: Some("chain.query".to_string()),
            output_classification: DataClassification::Public,
        }
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        debug!(agent = %context.agent_id, "querying governance tracks");

        let output = if let Some(track_id_val) = input.get("track_id") {
            let track_id = track_id_val
                .as_u64()
                .ok_or_else(|| ToolError::InvalidInput {
                    reason: "'track_id' must be a non-negative integer".to_string(),
                })?;

            let track_id = u16::try_from(track_id).map_err(|_| ToolError::InvalidInput {
                reason: format!("track_id {track_id} exceeds u16 range"),
            })?;

            let track = self
                .get_track(track_id)
                .await
                .map_err(|e| ToolError::ExecutionFailed {
                    reason: e.to_string(),
                })?;

            serde_json::to_value(&track).map_err(|e| ToolError::ExecutionFailed {
                reason: format!("failed to serialize track: {e}"),
            })?
        } else {
            let tracks = self
                .list_tracks()
                .await
                .map_err(|e| ToolError::ExecutionFailed {
                    reason: e.to_string(),
                })?;

            serde_json::json!({
                "count": tracks.len(),
                "tracks": serde_json::to_value(&tracks).map_err(|e| ToolError::ExecutionFailed {
                    reason: format!("failed to serialize tracks: {e}"),
                })?,
            })
        };

        Ok(ToolResult {
            output,
            classification: DataClassification::Public,
            artifacts: vec![],
        })
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
        }
    }

    fn sample_track() -> Track {
        Track {
            id: 0,
            name: "Root".to_string(),
            max_deciding: 1,
            decision_period: 403_200,
            confirm_period: 28_800,
            min_approval_curve: "reciprocal(0.0, 0.5, 0.5)".to_string(),
            min_support_curve: "linear_decreasing(0.5, 0.0)".to_string(),
            prepare_period: 1_200,
            min_deposit: 100_000_000_000_000,
        }
    }

    #[test]
    fn spec_has_correct_name() {
        let client = Arc::new(MockChainClient::new());
        let tool = TrackInfoTool::new(client);
        let spec = tool.spec();
        assert_eq!(spec.name, "polkagent.governance.track_info");
    }

    #[test]
    fn spec_requires_chain_query_grant() {
        let client = Arc::new(MockChainClient::new());
        let tool = TrackInfoTool::new(client);
        assert_eq!(tool.spec().required_grant, Some("chain.query".to_string()));
    }

    #[test]
    fn spec_track_id_is_optional() {
        let client = Arc::new(MockChainClient::new());
        let tool = TrackInfoTool::new(client);
        let spec = tool.spec();
        // "required" should be absent or not include "track_id".
        let required = spec.input_schema.get("required");
        if let Some(arr) = required.and_then(Value::as_array) {
            let names: Vec<&str> = arr.iter().filter_map(Value::as_str).collect();
            assert!(!names.contains(&"track_id"));
        }
    }

    #[tokio::test]
    async fn execute_list_all_empty() {
        let client = Arc::new(MockChainClient::new());
        let tool = TrackInfoTool::new(client);
        let input = serde_json::json!({});
        let result = tool
            .execute(input, &test_context())
            .await
            .unwrap_or_else(|e| panic!("execute failed: {e}"));
        assert_eq!(result.output["count"], 0);
    }

    #[tokio::test]
    async fn execute_list_all_with_data() {
        let mut client = MockChainClient::new();

        let tracks = vec![sample_track()];
        let bytes = serde_json::to_vec(&tracks).unwrap_or_else(|e| panic!("{e}"));
        client.insert_storage(b"Referenda:Tracks".to_vec(), bytes);

        let tool = TrackInfoTool::new(Arc::new(client));
        let input = serde_json::json!({});
        let result = tool
            .execute(input, &test_context())
            .await
            .unwrap_or_else(|e| panic!("execute failed: {e}"));
        assert_eq!(result.output["count"], 1);
        assert_eq!(result.output["tracks"][0]["name"], "Root");
    }

    #[tokio::test]
    async fn execute_specific_track_not_found() {
        let client = Arc::new(MockChainClient::new());
        let tool = TrackInfoTool::new(client);
        let input = serde_json::json!({ "track_id": 99 });
        let result = tool.execute(input, &test_context()).await;
        assert!(matches!(result, Err(ToolError::ExecutionFailed { .. })));
    }

    #[tokio::test]
    async fn execute_specific_track_found() {
        let mut client = MockChainClient::new();

        let track = sample_track();
        let bytes = serde_json::to_vec(&track).unwrap_or_else(|e| panic!("{e}"));
        let mut key = b"Referenda:Tracks:".to_vec();
        key.extend_from_slice(&0u16.to_le_bytes());
        client.insert_storage(key, bytes);

        let tool = TrackInfoTool::new(Arc::new(client));
        let input = serde_json::json!({ "track_id": 0 });
        let result = tool
            .execute(input, &test_context())
            .await
            .unwrap_or_else(|e| panic!("execute failed: {e}"));
        assert_eq!(result.output["name"], "Root");
        assert_eq!(result.output["max_deciding"], 1);
    }

    #[tokio::test]
    async fn execute_invalid_track_id_type() {
        let client = Arc::new(MockChainClient::new());
        let tool = TrackInfoTool::new(client);
        let input = serde_json::json!({ "track_id": "bad" });
        let result = tool.execute(input, &test_context()).await;
        assert!(matches!(result, Err(ToolError::InvalidInput { .. })));
    }
}
