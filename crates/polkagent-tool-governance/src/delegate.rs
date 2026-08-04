//! Delegation info tool.
//!
//! The [`DelegationInfoTool`] queries the delegation graph for an account,
//! returning both outgoing delegations (where the account delegates to
//! others) and incoming delegations (where others delegate to the account).

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tracing::debug;

use polkagent_chain_trait::ChainClient;
use polkagent_core::config::DataClassification;
use polkagent_tool::registry::{ToolContext, ToolError, ToolHandler, ToolResult, ToolSpec};

use crate::error::GovernanceError;
use crate::types::Delegation;

// ---------------------------------------------------------------------------
// DelegationInfoTool
// ---------------------------------------------------------------------------

/// Queries the delegation graph for an account.
///
/// Returns outgoing delegations (the account's own delegations to others)
/// and incoming delegations (where other accounts delegate voting power to
/// this account). Requires `chain.query` grant.
pub struct DelegationInfoTool {
    chain_client: Arc<dyn ChainClient>,
}

impl DelegationInfoTool {
    /// Create a new `DelegationInfoTool` backed by the given chain client.
    pub fn new(chain_client: Arc<dyn ChainClient>) -> Self {
        Self { chain_client }
    }

    /// Query outgoing delegations for an account.
    async fn lookup_outgoing(&self, account: &str) -> Result<Vec<Delegation>, GovernanceError> {
        let mut key = b"ConvictionVoting:DelegationsOutgoing:".to_vec();
        key.extend_from_slice(account.as_bytes());

        let result = self
            .chain_client
            .query_storage(
                &key,
                None,
                polkagent_chain_trait::ChainProfileId::new("default"),
            )
            .await
            .map_err(GovernanceError::Chain)?;

        match result {
            Some(bytes) => serde_json::from_slice(&bytes).map_err(|e| GovernanceError::Decode {
                message: format!("failed to decode outgoing delegations for {account}: {e}"),
            }),
            None => Ok(vec![]),
        }
    }

    /// Query incoming delegations for an account.
    async fn lookup_incoming(&self, account: &str) -> Result<Vec<Delegation>, GovernanceError> {
        let mut key = b"ConvictionVoting:DelegationsIncoming:".to_vec();
        key.extend_from_slice(account.as_bytes());

        let result = self
            .chain_client
            .query_storage(
                &key,
                None,
                polkagent_chain_trait::ChainProfileId::new("default"),
            )
            .await
            .map_err(GovernanceError::Chain)?;

        match result {
            Some(bytes) => serde_json::from_slice(&bytes).map_err(|e| GovernanceError::Decode {
                message: format!("failed to decode incoming delegations for {account}: {e}"),
            }),
            None => Ok(vec![]),
        }
    }
}

#[async_trait]
impl ToolHandler for DelegationInfoTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "polkagent.governance.delegation_info".to_string(),
            description: "Query the delegation graph for an account. Returns outgoing \
                          delegations (account delegates to others) and incoming delegations \
                          (others delegate to the account)."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "account": {
                        "type": "string",
                        "description": "SS58-encoded account address"
                    },
                    "direction": {
                        "type": "string",
                        "enum": ["outgoing", "incoming", "both"],
                        "description": "Which delegation direction to query (default: both)"
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

        if account.is_empty() {
            return Err(ToolError::InvalidInput {
                reason: "account address must not be empty".to_string(),
            });
        }

        let direction = input
            .get("direction")
            .and_then(Value::as_str)
            .unwrap_or("both");

        debug!(
            account = account,
            direction = direction,
            agent = %context.agent_id,
            "querying delegation info"
        );

        let (outgoing, incoming) = match direction {
            "outgoing" => {
                let out = self.lookup_outgoing(account).await.map_err(|e| {
                    ToolError::ExecutionFailed {
                        reason: e.to_string(),
                    }
                })?;
                (out, vec![])
            }
            "incoming" => {
                let inc = self.lookup_incoming(account).await.map_err(|e| {
                    ToolError::ExecutionFailed {
                        reason: e.to_string(),
                    }
                })?;
                (vec![], inc)
            }
            "both" | _ => {
                let out = self.lookup_outgoing(account).await.map_err(|e| {
                    ToolError::ExecutionFailed {
                        reason: e.to_string(),
                    }
                })?;
                let inc = self.lookup_incoming(account).await.map_err(|e| {
                    ToolError::ExecutionFailed {
                        reason: e.to_string(),
                    }
                })?;
                (out, inc)
            }
        };

        let total_delegated_balance: u128 = outgoing.iter().map(|d| d.balance).sum();
        let total_received_balance: u128 = incoming.iter().map(|d| d.balance).sum();

        Ok(ToolResult {
            output: serde_json::json!({
                "account": account,
                "outgoing": {
                    "count": outgoing.len(),
                    "total_balance": total_delegated_balance,
                    "delegations": serde_json::to_value(&outgoing).map_err(|e| {
                        ToolError::ExecutionFailed {
                            reason: format!("failed to serialize outgoing: {e}"),
                        }
                    })?,
                },
                "incoming": {
                    "count": incoming.len(),
                    "total_balance": total_received_balance,
                    "delegations": serde_json::to_value(&incoming).map_err(|e| {
                        ToolError::ExecutionFailed {
                            reason: format!("failed to serialize incoming: {e}"),
                        }
                    })?,
                },
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
    use crate::types::Conviction;
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
        let tool = DelegationInfoTool::new(client);
        assert_eq!(tool.spec().name, "polkagent.governance.delegation_info");
    }

    #[test]
    fn spec_requires_chain_query_grant() {
        let client = Arc::new(MockChainClient::new());
        let tool = DelegationInfoTool::new(client);
        assert_eq!(tool.spec().required_grant, Some("chain.query".to_string()));
    }

    #[tokio::test]
    async fn execute_missing_account() {
        let client = Arc::new(MockChainClient::new());
        let tool = DelegationInfoTool::new(client);
        let result = tool.execute(serde_json::json!({}), &test_context()).await;
        assert!(matches!(result, Err(ToolError::InvalidInput { .. })));
    }

    #[tokio::test]
    async fn execute_empty_account() {
        let client = Arc::new(MockChainClient::new());
        let tool = DelegationInfoTool::new(client);
        let input = serde_json::json!({ "account": "" });
        let result = tool.execute(input, &test_context()).await;
        assert!(matches!(result, Err(ToolError::InvalidInput { .. })));
    }

    #[tokio::test]
    async fn execute_no_delegations() {
        let client = Arc::new(MockChainClient::new());
        let tool = DelegationInfoTool::new(client);
        let input = serde_json::json!({ "account": "5Alice..." });
        let result = tool
            .execute(input, &test_context())
            .await
            .unwrap_or_else(|e| panic!("execute failed: {e}"));

        assert_eq!(result.output["outgoing"]["count"], 0);
        assert_eq!(result.output["incoming"]["count"], 0);
    }

    #[tokio::test]
    async fn execute_with_outgoing_delegations() {
        let mut client = MockChainClient::new();

        let delegations = vec![Delegation {
            delegator: "5Alice...".to_string(),
            delegate: "5Bob...".to_string(),
            track: 0,
            conviction: Conviction::Locked1X,
            balance: 10_000_000_000_000,
        }];
        let bytes = serde_json::to_vec(&delegations).unwrap_or_else(|e| panic!("{e}"));
        let mut key = b"ConvictionVoting:DelegationsOutgoing:".to_vec();
        key.extend_from_slice(b"5Alice...");
        client.insert_storage(key, bytes);

        let tool = DelegationInfoTool::new(Arc::new(client));
        let input = serde_json::json!({ "account": "5Alice...", "direction": "outgoing" });
        let result = tool
            .execute(input, &test_context())
            .await
            .unwrap_or_else(|e| panic!("execute failed: {e}"));

        assert_eq!(result.output["outgoing"]["count"], 1);
        assert_eq!(result.output["incoming"]["count"], 0);
        assert_eq!(
            result.output["outgoing"]["delegations"][0]["delegate"],
            "5Bob..."
        );
    }

    #[tokio::test]
    async fn execute_with_incoming_delegations() {
        let mut client = MockChainClient::new();

        let delegations = vec![Delegation {
            delegator: "5Charlie...".to_string(),
            delegate: "5Alice...".to_string(),
            track: 1,
            conviction: Conviction::Locked3X,
            balance: 20_000_000_000_000,
        }];
        let bytes = serde_json::to_vec(&delegations).unwrap_or_else(|e| panic!("{e}"));
        let mut key = b"ConvictionVoting:DelegationsIncoming:".to_vec();
        key.extend_from_slice(b"5Alice...");
        client.insert_storage(key, bytes);

        let tool = DelegationInfoTool::new(Arc::new(client));
        let input = serde_json::json!({ "account": "5Alice...", "direction": "incoming" });
        let result = tool
            .execute(input, &test_context())
            .await
            .unwrap_or_else(|e| panic!("execute failed: {e}"));

        assert_eq!(result.output["outgoing"]["count"], 0);
        assert_eq!(result.output["incoming"]["count"], 1);
        assert_eq!(
            result.output["incoming"]["total_balance"],
            20_000_000_000_000_u64
        );
    }
}
