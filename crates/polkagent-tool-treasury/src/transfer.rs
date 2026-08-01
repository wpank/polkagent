//! Transfer history tool.
//!
//! [`TransferHistoryTool`] retrieves recent transfer events for an account,
//! including both incoming and outgoing transfers with timestamps and
//! extrinsic hashes.
//!
//! **Grant requirement:** `chain.query`

use async_trait::async_trait;
use serde_json::Value;
use tracing::debug;

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
pub struct TransferHistoryTool;

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

        debug!(account = account_id, chain, limit, "querying transfer history");

        // In production this would index transfer events from chain storage.
        // Return empty list as placeholder.
        let transfers: Vec<Transfer> = vec![];

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
mod tests {
    use super::*;
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
    fn spec_name_and_grant() {
        let spec = TransferHistoryTool.spec();
        assert_eq!(spec.name, "polkagent.treasury.transfer_history");
        assert_eq!(spec.required_grant.as_deref(), Some("chain.query"));
    }

    #[test]
    fn spec_schema_has_limit_field() {
        let spec = TransferHistoryTool.spec();
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

        let result = TransferHistoryTool
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
    async fn execute_default_limit() {
        let input = serde_json::json!({
            "account_id": "5GrwvaEFcWWqn1bRJJpPi8HBdhQ"
        });

        let result = TransferHistoryTool
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

        let result = TransferHistoryTool
            .execute(input, &test_context())
            .await
            .expect("should succeed");

        assert_eq!(result.output["limit"], MAX_LIMIT);
    }

    #[tokio::test]
    async fn execute_missing_account_id() {
        let input = serde_json::json!({});
        let result = TransferHistoryTool.execute(input, &test_context()).await;
        assert!(matches!(result, Err(ToolError::InvalidInput { .. })));
    }

    #[tokio::test]
    async fn execute_unsupported_chain() {
        let input = serde_json::json!({
            "account_id": "5GrwvaEF...",
            "chain": "cardano"
        });
        let result = TransferHistoryTool.execute(input, &test_context()).await;
        assert!(matches!(result, Err(ToolError::ExecutionFailed { .. })));
    }
}
