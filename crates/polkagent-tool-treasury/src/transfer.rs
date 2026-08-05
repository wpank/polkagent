//! Transfer history tool.
//!
//! [`TransferHistoryTool`] retrieves recent transfer events for an account,
//! including both incoming and outgoing transfers with timestamps and
//! extrinsic hashes.
//!
//! **Grant requirement:** `chain.query`

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tracing::{debug, warn};

use polkagent_chain_trait::{ChainClient, ChainProfileId};
use polkagent_core::config::DataClassification;
use polkagent_tool::registry::{ToolContext, ToolError, ToolHandler, ToolResult, ToolSpec};

use crate::balance::parse_account_input;
use crate::types::Transfer;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Default maximum number of transfers to return.
const DEFAULT_LIMIT: u64 = 20;

/// Maximum allowed limit to prevent excessive queries.
const MAX_LIMIT: u64 = 100;

// ---------------------------------------------------------------------------
// TransferHistoryTool
// ---------------------------------------------------------------------------

/// Build the storage key for `Balances::Transfers(account_id)`.
fn build_transfer_storage_key(account_id: &str) -> Vec<u8> {
    let mut key = b"Balances:Transfers:".to_vec();
    key.extend_from_slice(account_id.as_bytes());
    key
}

/// Retrieves recent transfers in and out of an account.
///
/// Returns a list of [`Transfer`] records sorted by block number (most
/// recent first), including sender, recipient, amount, asset, block number,
/// timestamp, and extrinsic hash.
///
/// # Input schema
///
/// ```json
/// {
///   "account_id": "5GrwvaEF...",   // SS58 address (required)
///   "chain": "polkadot",            // optional, defaults to "polkadot"
///   "limit": 20                     // optional, max results (1-100)
/// }
/// ```
///
/// **Grant requirement:** `chain.query`
pub struct TransferHistoryTool {
    chain_client: Arc<dyn ChainClient>,
}

impl TransferHistoryTool {
    /// Create a new `TransferHistoryTool` backed by the given chain client.
    pub fn new(chain_client: Arc<dyn ChainClient>) -> Self {
        Self { chain_client }
    }
}

#[async_trait]
impl ToolHandler for TransferHistoryTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "polkagent.treasury.transfer_history".to_string(),
            description: "Retrieve recent transfers in and out of an account, with timestamps, amounts, and extrinsic hashes.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "account_id": {
                        "type": "string",
                        "description": "SS58-encoded account address"
                    },
                    "chain": {
                        "type": "string",
                        "description": "Chain identifier (e.g. polkadot, kusama). Defaults to polkadot.",
                        "default": "polkadot"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of transfers to return (1-100). Defaults to 20.",
                        "default": 20,
                        "minimum": 1,
                        "maximum": 100
                    }
                },
                "required": ["account_id"]
            }),
            required_grant: Some("chain.query".to_string()),
            output_classification: DataClassification::Internal,
        }
    }

    async fn execute(&self, input: Value, _context: &ToolContext) -> Result<ToolResult, ToolError> {
        let (account_id, chain) = parse_account_input(&input)?;
        let (symbol, _decimals) = crate::balance::chain_token_info(chain)?;

        let limit = input
            .get("limit")
            .and_then(Value::as_u64)
            .unwrap_or(DEFAULT_LIMIT)
            .min(MAX_LIMIT);

        debug!(
            account = account_id,
            chain, limit, "querying transfer history"
        );

        let storage_key = build_transfer_storage_key(account_id);
        let chain_profile = ChainProfileId::new(chain);

        let transfers: Vec<Transfer> = match self
            .chain_client
            .query_storage(&storage_key, None, chain_profile)
            .await
        {
            Ok(Some(bytes)) => {
                let all: Vec<Transfer> = serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                    warn!(
                        account = account_id,
                        chain,
                        error = %e,
                        "failed to decode transfer history, returning empty"
                    );
                    vec![]
                });
                // Apply the limit.
                all.into_iter().take(limit as usize).collect()
            }
            Ok(None) => {
                debug!(account = account_id, chain, "no transfer data on chain");
                vec![]
            }
            Err(e) => {
                warn!(
                    account = account_id,
                    chain,
                    error = %e,
                    "chain query failed for transfers, returning empty"
                );
                vec![]
            }
        };

        let output = serde_json::json!({
            "account_id": account_id,
            "chain": chain,
            "asset": symbol,
            "transfers": serde_json::to_value(&transfers).map_err(|e| ToolError::ExecutionFailed {
                reason: format!("failed to serialize transfers: {e}"),
            })?,
            "count": transfers.len(),
            "limit": limit,
        });

        Ok(ToolResult {
            output,
            classification: DataClassification::Internal,
            artifacts: vec![],
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
// Test assertions deliberately unwrap fixtures so failures retain precise context.
#[allow(clippy::expect_used, clippy::unwrap_used)]
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

    fn make_tool() -> TransferHistoryTool {
        TransferHistoryTool::new(Arc::new(MockChainClient::new()))
    }

    #[test]
    fn spec_name_and_grant() {
        let spec = make_tool().spec();
        assert_eq!(spec.name, "polkagent.treasury.transfer_history");
        assert_eq!(spec.required_grant.as_deref(), Some("chain.query"));
    }

    #[test]
    fn spec_schema_has_limit_field() {
        let spec = make_tool().spec();
        let limit_prop = &spec.input_schema["properties"]["limit"];
        assert_eq!(limit_prop["type"], "integer");
        assert_eq!(limit_prop["minimum"], 1);
        assert_eq!(limit_prop["maximum"], 100);
    }

    #[tokio::test]
    async fn execute_returns_transfer_list() {
        let input = serde_json::json!({
            "account_id": "5GrwvaEFcWWqn1bRJJpPi8HBdhQ",
            "chain": "polkadot",
            "limit": 10
        });

        let result = make_tool()
            .execute(input, &test_context())
            .await
            .expect("should succeed");

        assert_eq!(result.output["chain"], "polkadot");
        assert_eq!(result.output["asset"], "DOT");
        assert_eq!(result.output["limit"], 10);
        assert_eq!(result.output["count"], 0);

        let transfers = result.output["transfers"]
            .as_array()
            .expect("transfers array");
        assert!(transfers.is_empty());
    }

    #[tokio::test]
    async fn execute_with_chain_data() {
        let mut client = MockChainClient::new();
        let transfers = vec![
            Transfer {
                from: "5Alice".to_string(),
                to: "5GrwvaEFcWWqn1bRJJpPi8HBdhQ".to_string(),
                amount: 1_000_000_000_000,
                asset: "DOT".to_string(),
                block_number: 18_500_000,
                timestamp: Some("2024-06-15T12:30:00Z".to_string()),
                extrinsic_hash: Some("0xabc123".to_string()),
            },
            Transfer {
                from: "5GrwvaEFcWWqn1bRJJpPi8HBdhQ".to_string(),
                to: "5Bob".to_string(),
                amount: 500_000_000_000,
                asset: "DOT".to_string(),
                block_number: 18_500_001,
                timestamp: None,
                extrinsic_hash: None,
            },
        ];
        let key = build_transfer_storage_key("5GrwvaEFcWWqn1bRJJpPi8HBdhQ");
        client.insert_storage(key, serde_json::to_vec(&transfers).expect("serialize"));

        let tool = TransferHistoryTool::new(Arc::new(client));
        let input = serde_json::json!({
            "account_id": "5GrwvaEFcWWqn1bRJJpPi8HBdhQ",
            "chain": "polkadot",
            "limit": 10
        });

        let result = tool
            .execute(input, &test_context())
            .await
            .expect("should succeed");

        assert_eq!(result.output["count"], 2);
        let out_transfers = result.output["transfers"]
            .as_array()
            .expect("transfers array");
        assert_eq!(out_transfers.len(), 2);
        assert_eq!(
            out_transfers[0]["amount"],
            serde_json::json!(1_000_000_000_000_u128)
        );
    }

    #[tokio::test]
    async fn execute_default_limit() {
        let input = serde_json::json!({
            "account_id": "5GrwvaEFcWWqn1bRJJpPi8HBdhQ"
        });

        let result = make_tool()
            .execute(input, &test_context())
            .await
            .expect("should succeed");

        assert_eq!(result.output["limit"], DEFAULT_LIMIT);
    }

    #[tokio::test]
    async fn execute_clamps_limit_to_max() {
        let input = serde_json::json!({
            "account_id": "5GrwvaEFcWWqn1bRJJpPi8HBdhQ",
            "limit": 500
        });

        let result = make_tool()
            .execute(input, &test_context())
            .await
            .expect("should succeed");

        assert_eq!(result.output["limit"], MAX_LIMIT);
    }

    #[tokio::test]
    async fn execute_missing_account_id() {
        let input = serde_json::json!({});
        let result = make_tool().execute(input, &test_context()).await;
        assert!(matches!(result, Err(ToolError::InvalidInput { .. })));
    }

    #[tokio::test]
    async fn execute_unsupported_chain() {
        let input = serde_json::json!({
            "account_id": "5GrwvaEF...",
            "chain": "cardano"
        });
        let result = make_tool().execute(input, &test_context()).await;
        assert!(matches!(result, Err(ToolError::ExecutionFailed { .. })));
    }
}
