//! Voter history tool.
//!
//! The [`VoterHistoryTool`] looks up the voting history for an SS58-encoded
//! account across OpenGov referenda, returning a list of votes with
//! conviction, balance, and direction.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tracing::debug;

use polkagent_chain_trait::ChainClient;
use polkagent_core::config::DataClassification;
use polkagent_tool::registry::{ToolContext, ToolError, ToolHandler, ToolResult, ToolSpec};

use crate::error::GovernanceError;
use crate::types::Vote;

// ---------------------------------------------------------------------------
// VoterHistoryTool
// ---------------------------------------------------------------------------

/// Looks up voting history for an account across OpenGov referenda.
///
/// Returns a list of votes cast by the account, including the referendum
/// index, conviction, balance, and direction for each vote. Requires
/// `chain.query` grant for read-only chain state access.
pub struct VoterHistoryTool {
    chain_client: Arc<dyn ChainClient>,
}

impl VoterHistoryTool {
    /// Create a new `VoterHistoryTool` backed by the given chain client.
    pub fn new(chain_client: Arc<dyn ChainClient>) -> Self {
        Self { chain_client }
    }

    /// Query voting history for the given account.
    async fn lookup_votes(&self, account: &str) -> Result<Vec<Vote>, GovernanceError> {
        self.chain_client
            .health()
            .await
            .map_err(GovernanceError::Chain)?;

        if account.is_empty() {
            return Err(GovernanceError::InvalidAccount {
                address: account.to_string(),
            });
        }

        let mut storage_key = b"ConvictionVoting:VotingFor:".to_vec();
        storage_key.extend_from_slice(account.as_bytes());

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
                message: format!("failed to decode votes for {account}: {e}"),
            }),
            None => Err(GovernanceError::NoVotingHistory {
                account: account.to_string(),
            }),
        }
    }
}

#[async_trait]
impl ToolHandler for VoterHistoryTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "polkagent.governance.voter_history".to_string(),
            description: "Look up voting history for an SS58 account across OpenGov referenda. \
                          Returns votes with conviction, balance, and direction."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "account": {
                        "type": "string",
                        "description": "SS58-encoded account address"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of votes to return (default: all)"
                    }
                },
                "required": ["account"]
            }),
            required_grant: Some("chain.query".to_string()),
            output_classification: DataClassification::Internal,
        }
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let account = input
            .get("account")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidInput {
                reason: "missing or invalid 'account' field — must be an SS58 address string"
                    .to_string(),
            })?;

        let limit = input
            .get("limit")
            .and_then(Value::as_u64)
            .map(|v| v as usize);

        debug!(
            account = account,
            agent = %context.agent_id,
            "looking up voter history"
        );

        let mut votes = self.lookup_votes(account).await.map_err(|e| {
            ToolError::ExecutionFailed {
                reason: e.to_string(),
            }
        })?;

        if let Some(max) = limit {
            votes.truncate(max);
        }

        Ok(ToolResult {
            output: serde_json::json!({
                "account": account,
                "count": votes.len(),
                "votes": serde_json::to_value(&votes).map_err(|e| ToolError::ExecutionFailed {
                    reason: format!("failed to serialize votes: {e}"),
                })?,
            }),
            classification: DataClassification::Internal,
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
    use crate::types::{Conviction, VoteDirection};
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
        let tool = VoterHistoryTool::new(client);
        assert_eq!(
            tool.spec().name,
            "polkagent.governance.voter_history"
        );
    }

    #[test]
    fn spec_requires_chain_query_grant() {
        let client = Arc::new(MockChainClient::new());
        let tool = VoterHistoryTool::new(client);
        assert_eq!(
            tool.spec().required_grant,
            Some("chain.query".to_string())
        );
    }

    #[test]
    fn spec_output_classification_is_internal() {
        let client = Arc::new(MockChainClient::new());
        let tool = VoterHistoryTool::new(client);
        assert_eq!(
            tool.spec().output_classification,
            DataClassification::Internal
        );
    }

    #[tokio::test]
    async fn execute_missing_account() {
        let client = Arc::new(MockChainClient::new());
        let tool = VoterHistoryTool::new(client);
        let result = tool.execute(serde_json::json!({}), &test_context()).await;
        assert!(matches!(result, Err(ToolError::InvalidInput { .. })));
    }

    #[tokio::test]
    async fn execute_no_history() {
        let client = Arc::new(MockChainClient::new());
        let tool = VoterHistoryTool::new(client);
        let input = serde_json::json!({ "account": "5GrwvaEF..." });
        let result = tool.execute(input, &test_context()).await;
        assert!(matches!(result, Err(ToolError::ExecutionFailed { .. })));
    }

    #[tokio::test]
    async fn execute_success() {
        let mut client = MockChainClient::new();

        let votes = vec![
            Vote {
                account: "5Alice...".to_string(),
                referendum_index: 10,
                conviction: Conviction::Locked1X,
                balance: 1_000_000_000_000,
                direction: VoteDirection::Aye,
            },
            Vote {
                account: "5Alice...".to_string(),
                referendum_index: 11,
                conviction: Conviction::None,
                balance: 500_000_000_000,
                direction: VoteDirection::Nay,
            },
        ];
        let bytes = serde_json::to_vec(&votes).unwrap_or_else(|e| panic!("{e}"));
        let mut key = b"ConvictionVoting:VotingFor:".to_vec();
        key.extend_from_slice(b"5Alice...");
        client.insert_storage(key, bytes);

        let tool = VoterHistoryTool::new(Arc::new(client));
        let input = serde_json::json!({ "account": "5Alice..." });
        let result = tool
            .execute(input, &test_context())
            .await
            .unwrap_or_else(|e| panic!("execute failed: {e}"));

        assert_eq!(result.output["count"], 2);
        assert_eq!(result.output["account"], "5Alice...");
        assert_eq!(result.output["votes"][0]["direction"], "aye");
    }

    #[tokio::test]
    async fn execute_with_limit() {
        let mut client = MockChainClient::new();

        let votes = vec![
            Vote {
                account: "5Bob...".to_string(),
                referendum_index: 1,
                conviction: Conviction::Locked2X,
                balance: 2_000_000_000_000,
                direction: VoteDirection::Aye,
            },
            Vote {
                account: "5Bob...".to_string(),
                referendum_index: 2,
                conviction: Conviction::Locked1X,
                balance: 1_000_000_000_000,
                direction: VoteDirection::Nay,
            },
            Vote {
                account: "5Bob...".to_string(),
                referendum_index: 3,
                conviction: Conviction::None,
                balance: 500_000_000_000,
                direction: VoteDirection::Abstain,
            },
        ];
        let bytes = serde_json::to_vec(&votes).unwrap_or_else(|e| panic!("{e}"));
        let mut key = b"ConvictionVoting:VotingFor:".to_vec();
        key.extend_from_slice(b"5Bob...");
        client.insert_storage(key, bytes);

        let tool = VoterHistoryTool::new(Arc::new(client));
        let input = serde_json::json!({ "account": "5Bob...", "limit": 2 });
        let result = tool
            .execute(input, &test_context())
            .await
            .unwrap_or_else(|e| panic!("execute failed: {e}"));

        assert_eq!(result.output["count"], 2);
    }
}
