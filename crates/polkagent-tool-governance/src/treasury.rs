//! Treasury overview tool.
//!
//! The [`TreasuryOverviewTool`] queries the on-chain treasury state, returning
//! the free balance, pending proposals, spend periods, and approved proposal
//! count.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tracing::debug;

use polkagent_chain_trait::ChainClient;
use polkagent_core::config::DataClassification;
use polkagent_tool::registry::{ToolContext, ToolError, ToolHandler, ToolResult, ToolSpec};

use crate::error::GovernanceError;
use crate::types::TreasuryInfo;

// ---------------------------------------------------------------------------
// TreasuryOverviewTool
// ---------------------------------------------------------------------------

/// Queries the on-chain treasury state.
///
/// Returns the treasury free balance, pending spend proposals, the next
/// spend period block number, and the count of approved-but-unpaid
/// proposals. Requires `chain.query` grant.
pub struct TreasuryOverviewTool {
    chain_client: Arc<dyn ChainClient>,
}

impl TreasuryOverviewTool {
    /// Create a new `TreasuryOverviewTool` backed by the given chain client.
    pub fn new(chain_client: Arc<dyn ChainClient>) -> Self {
        Self { chain_client }
    }

    /// Query the treasury state from the chain.
    async fn query_treasury(&self) -> Result<TreasuryInfo, GovernanceError> {
        self.chain_client
            .health()
            .await
            .map_err(GovernanceError::Chain)?;

        let storage_key = b"Treasury:Overview".to_vec();

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
                message: format!("failed to decode treasury info: {e}"),
            }),
            None => Err(GovernanceError::TreasuryUnavailable {
                reason: "treasury state not found in chain storage".to_string(),
            }),
        }
    }
}

#[async_trait]
impl ToolHandler for TreasuryOverviewTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "polkagent.governance.treasury_overview".to_string(),
            description: "Query the on-chain treasury state. Returns the free balance, \
                          pending proposals, next spend period, and approved proposal count."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
            required_grant: Some("chain.query".to_string()),
            output_classification: DataClassification::Public,
        }
    }

    async fn execute(&self, _input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        debug!(agent = %context.agent_id, "querying treasury overview");

        let info = self
            .query_treasury()
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                reason: e.to_string(),
            })?;

        let output = serde_json::to_value(&info).map_err(|e| ToolError::ExecutionFailed {
            reason: format!("failed to serialize treasury info: {e}"),
        })?;

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
    use crate::types::TreasuryProposal;
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
        let tool = TreasuryOverviewTool::new(client);
        assert_eq!(tool.spec().name, "polkagent.governance.treasury_overview");
    }

    #[test]
    fn spec_requires_chain_query_grant() {
        let client = Arc::new(MockChainClient::new());
        let tool = TreasuryOverviewTool::new(client);
        assert_eq!(tool.spec().required_grant, Some("chain.query".to_string()));
    }

    #[test]
    fn spec_has_no_required_input() {
        let client = Arc::new(MockChainClient::new());
        let tool = TreasuryOverviewTool::new(client);
        let spec = tool.spec();
        // No "required" field or empty required array.
        let required = spec.input_schema.get("required");
        assert!(required.is_none());
    }

    #[test]
    fn spec_output_classification_is_public() {
        let client = Arc::new(MockChainClient::new());
        let tool = TreasuryOverviewTool::new(client);
        assert_eq!(
            tool.spec().output_classification,
            DataClassification::Public
        );
    }

    #[tokio::test]
    async fn execute_treasury_unavailable() {
        let client = Arc::new(MockChainClient::new());
        let tool = TreasuryOverviewTool::new(client);
        let result = tool.execute(serde_json::json!({}), &test_context()).await;
        assert!(matches!(result, Err(ToolError::ExecutionFailed { .. })));
    }

    #[tokio::test]
    async fn execute_success() {
        let mut client = MockChainClient::new();

        let info = TreasuryInfo {
            free_balance: 50_000_000_000_000_000,
            pending_proposals: vec![TreasuryProposal {
                index: 0,
                proposer: "5Alice...".to_string(),
                beneficiary: "5Bob...".to_string(),
                value: 1_000_000_000_000,
            }],
            next_spend_period: 1_234_567,
            approved_count: 3,
        };
        let bytes = serde_json::to_vec(&info).unwrap_or_else(|e| panic!("{e}"));
        client.insert_storage(b"Treasury:Overview".to_vec(), bytes);

        let tool = TreasuryOverviewTool::new(Arc::new(client));
        let result = tool
            .execute(serde_json::json!({}), &test_context())
            .await
            .unwrap_or_else(|e| panic!("execute failed: {e}"));

        assert_eq!(result.output["approved_count"], 3);
        assert_eq!(result.output["next_spend_period"], 1_234_567);
        assert_eq!(
            result.output["pending_proposals"].as_array().map(Vec::len),
            Some(1)
        );
    }

    #[tokio::test]
    async fn execute_empty_treasury() {
        let mut client = MockChainClient::new();

        let info = TreasuryInfo {
            free_balance: 0,
            pending_proposals: vec![],
            next_spend_period: 0,
            approved_count: 0,
        };
        let bytes = serde_json::to_vec(&info).unwrap_or_else(|e| panic!("{e}"));
        client.insert_storage(b"Treasury:Overview".to_vec(), bytes);

        let tool = TreasuryOverviewTool::new(Arc::new(client));
        let result = tool
            .execute(serde_json::json!({}), &test_context())
            .await
            .unwrap_or_else(|e| panic!("execute failed: {e}"));

        assert_eq!(result.output["free_balance"], 0);
        assert_eq!(result.output["approved_count"], 0);
    }
}
