//! Balance query tool.
//!
//! [`BalanceQueryTool`] queries an account's balance breakdown (free, reserved,
//! frozen, total) for the chain's native token or a specified asset. It
//! returns an [`AccountBalance`] as structured JSON.
//!
//! **Grant requirement:** `chain.query`

use async_trait::async_trait;
use serde_json::Value;
use tracing::debug;

use polkagent_core::config::DataClassification;
use polkagent_tool::registry::{ToolContext, ToolError, ToolHandler, ToolResult, ToolSpec};

use crate::error::TreasuryToolError;
use crate::types::AccountBalance;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Default chain identifier when none is provided in the input.
const DEFAULT_CHAIN: &str = "polkadot";

/// Parse the `account_id` and optional `chain` from tool input JSON.
pub(crate) fn parse_account_input(input: &Value) -> Result<(&str, &str), TreasuryToolError> {
    let account_id = input
        .get("account_id")
        .and_then(Value::as_str)
        .ok_or_else(|| TreasuryToolError::InvalidInput {
            reason: "missing or invalid 'account_id' field".to_string(),
        })?;

    if account_id.is_empty() {
        return Err(TreasuryToolError::InvalidAddress {
            address: account_id.to_string(),
        });
    }

    let chain = input
        .get("chain")
        .and_then(Value::as_str)
        .unwrap_or(DEFAULT_CHAIN);

    Ok((account_id, chain))
}

/// Return the native token metadata for a known chain.
pub(crate) fn chain_token_info(chain: &str) -> Result<(&str, u8), TreasuryToolError> {
    match chain {
        "polkadot" => Ok(("DOT", 10)),
        "kusama" => Ok(("KSM", 12)),
        "westend" => Ok(("WND", 12)),
        "rococo" => Ok(("ROC", 12)),
        _ => Err(TreasuryToolError::UnsupportedChain {
            chain: chain.to_string(),
        }),
    }
}

// ---------------------------------------------------------------------------
// BalanceQueryTool
// ---------------------------------------------------------------------------

/// Queries an account's free, reserved, frozen, and total balance.
///
/// Supports the native token of Polkadot, Kusama, Westend, and Rococo.
/// Returns a structured [`AccountBalance`] in the tool output.
///
/// # Input schema
///
/// ```json
/// {
///   "account_id": "5GrwvaEF...",  // SS58 address (required)
///   "chain": "polkadot"            // optional, defaults to "polkadot"
/// }
/// ```
///
/// **Grant requirement:** `chain.query`
pub struct BalanceQueryTool;

#[async_trait]
impl ToolHandler for BalanceQueryTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "polkagent.treasury.balance_query".to_string(),
            description: "Query the free, reserved, frozen, and total balance for an account on a Polkadot-SDK chain.".to_string(),
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
        let (symbol, decimals) = chain_token_info(chain)?;

        debug!(account = account_id, chain, "querying balance");

        // In production this would query chain state via ChainClient.
        // For now we return a representative balance structure.
        let balance = AccountBalance {
            free: 0,
            reserved: 0,
            frozen: 0,
            total: 0,
            asset_id: None,
            symbol: symbol.to_string(),
            decimals,
        };

        let output = serde_json::to_value(&balance).map_err(|e| ToolError::ExecutionFailed {
            reason: format!("failed to serialize balance: {e}"),
        })?;

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
        let spec = BalanceQueryTool.spec();
        assert_eq!(spec.name, "polkagent.treasury.balance_query");
        assert_eq!(spec.required_grant.as_deref(), Some("chain.query"));
    }

    #[test]
    fn spec_has_input_schema() {
        let spec = BalanceQueryTool.spec();
        let props = &spec.input_schema["properties"];
        assert!(props.get("account_id").is_some());
        assert!(props.get("chain").is_some());
    }

    #[test]
    fn parse_account_input_valid() {
        let input = serde_json::json!({
            "account_id": "5GrwvaEFcWWqn1bRJJpPi8HBdhQ",
            "chain": "kusama"
        });
        let (account, chain) = parse_account_input(&input).expect("should parse");
        assert_eq!(account, "5GrwvaEFcWWqn1bRJJpPi8HBdhQ");
        assert_eq!(chain, "kusama");
    }

    #[test]
    fn parse_account_input_default_chain() {
        let input = serde_json::json!({
            "account_id": "5GrwvaEF..."
        });
        let (_, chain) = parse_account_input(&input).expect("should parse");
        assert_eq!(chain, "polkadot");
    }

    #[test]
    fn parse_account_input_missing_account() {
        let input = serde_json::json!({ "chain": "polkadot" });
        let err = parse_account_input(&input).unwrap_err();
        assert!(matches!(err, TreasuryToolError::InvalidInput { .. }));
    }

    #[test]
    fn parse_account_input_empty_account() {
        let input = serde_json::json!({ "account_id": "" });
        let err = parse_account_input(&input).unwrap_err();
        assert!(matches!(err, TreasuryToolError::InvalidAddress { .. }));
    }

    #[test]
    fn chain_token_info_polkadot() {
        let (symbol, decimals) = chain_token_info("polkadot").expect("known chain");
        assert_eq!(symbol, "DOT");
        assert_eq!(decimals, 10);
    }

    #[test]
    fn chain_token_info_kusama() {
        let (symbol, decimals) = chain_token_info("kusama").expect("known chain");
        assert_eq!(symbol, "KSM");
        assert_eq!(decimals, 12);
    }

    #[test]
    fn chain_token_info_unknown() {
        let err = chain_token_info("ethereum").unwrap_err();
        assert!(matches!(err, TreasuryToolError::UnsupportedChain { .. }));
    }

    #[tokio::test]
    async fn execute_returns_balance_json() {
        let input = serde_json::json!({
            "account_id": "5GrwvaEFcWWqn1bRJJpPi8HBdhQ",
            "chain": "polkadot"
        });

        let result = BalanceQueryTool
            .execute(input, &test_context())
            .await
            .expect("should succeed");

        let balance: AccountBalance =
            serde_json::from_value(result.output).expect("deserialize output");
        assert_eq!(balance.symbol, "DOT");
        assert_eq!(balance.decimals, 10);
        assert_eq!(balance.asset_id, None);
    }

    #[tokio::test]
    async fn execute_missing_account_id() {
        let input = serde_json::json!({});
        let result = BalanceQueryTool.execute(input, &test_context()).await;
        assert!(matches!(result, Err(ToolError::InvalidInput { .. })));
    }

    #[tokio::test]
    async fn execute_unsupported_chain() {
        let input = serde_json::json!({
            "account_id": "5GrwvaEF...",
            "chain": "ethereum"
        });
        let result = BalanceQueryTool.execute(input, &test_context()).await;
        assert!(matches!(result, Err(ToolError::ExecutionFailed { .. })));
    }
}
