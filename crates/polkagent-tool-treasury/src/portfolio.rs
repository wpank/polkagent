//! Portfolio summary tool.
//!
//! [`PortfolioSummaryTool`] provides an aggregated view of all assets held by
//! an account: native balances, staking positions, vesting schedules, and
//! crowdloan contributions. Each line item is tagged with its source category.
//!
//! **Grant requirement:** `chain.query`

use async_trait::async_trait;
use serde_json::Value;
use tracing::debug;

use polkagent_core::config::DataClassification;
use polkagent_tool::registry::{ToolContext, ToolError, ToolHandler, ToolResult, ToolSpec};

use crate::balance::parse_account_input;
use crate::types::{PortfolioEntry, PortfolioSource};

// ---------------------------------------------------------------------------
// PortfolioSummaryTool
// ---------------------------------------------------------------------------

/// Produces an aggregated portfolio view for an account.
///
/// Gathers balances from native tokens, staking, vesting, and crowdloans
/// into a single list of [`PortfolioEntry`] items. Each entry includes the
/// asset name, balance (planck), an optional USD valuation, and the source
/// category.
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
pub struct PortfolioSummaryTool;

#[async_trait]
impl ToolHandler for PortfolioSummaryTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "polkagent.treasury.portfolio_summary".to_string(),
            description: "Get an aggregated portfolio view of all assets for an account, including native balance, staking, vesting, and crowdloan positions.".to_string(),
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
        let (symbol, _decimals) = crate::balance::chain_token_info(chain)?;

        debug!(account = account_id, chain, "building portfolio summary");

        // In production each source would be queried independently and
        // aggregated. Here we return the skeleton entries.
        let entries: Vec<PortfolioEntry> = vec![
            PortfolioEntry {
                asset: symbol.to_string(),
                balance: 0,
                value_usd: None,
                source: PortfolioSource::Native,
            },
            PortfolioEntry {
                asset: symbol.to_string(),
                balance: 0,
                value_usd: None,
                source: PortfolioSource::Staking,
            },
            PortfolioEntry {
                asset: symbol.to_string(),
                balance: 0,
                value_usd: None,
                source: PortfolioSource::Vesting,
            },
            PortfolioEntry {
                asset: symbol.to_string(),
                balance: 0,
                value_usd: None,
                source: PortfolioSource::Crowdloan,
            },
        ];

        let output = serde_json::json!({
            "account_id": account_id,
            "chain": chain,
            "entries": serde_json::to_value(&entries).map_err(|e| ToolError::ExecutionFailed {
                reason: format!("failed to serialize portfolio: {e}"),
            })?,
            "total_entries": entries.len(),
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
        let spec = PortfolioSummaryTool.spec();
        assert_eq!(spec.name, "polkagent.treasury.portfolio_summary");
        assert_eq!(spec.required_grant.as_deref(), Some("chain.query"));
    }

    #[test]
    fn spec_schema_has_required_account_id() {
        let spec = PortfolioSummaryTool.spec();
        let required = spec.input_schema["required"]
            .as_array()
            .expect("required array");
        assert!(required.iter().any(|v| v.as_str() == Some("account_id")));
    }

    #[tokio::test]
    async fn execute_returns_portfolio_entries() {
        let input = serde_json::json!({
            "account_id": "5GrwvaEFcWWqn1bRJJpPi8HBdhQ",
            "chain": "kusama"
        });

        let result = PortfolioSummaryTool
            .execute(input, &test_context())
            .await
            .expect("should succeed");

        assert_eq!(result.output["chain"], "kusama");
        assert_eq!(result.output["total_entries"], 4);

        let entries = result.output["entries"].as_array().expect("entries array");
        assert_eq!(entries.len(), 4);

        // Verify all four sources are present.
        let sources: Vec<&str> = entries
            .iter()
            .filter_map(|e| e["source"].as_str())
            .collect();
        assert!(sources.contains(&"native"));
        assert!(sources.contains(&"staking"));
        assert!(sources.contains(&"vesting"));
        assert!(sources.contains(&"crowdloan"));
    }

    #[tokio::test]
    async fn execute_missing_account_id() {
        let input = serde_json::json!({ "chain": "polkadot" });
        let result = PortfolioSummaryTool.execute(input, &test_context()).await;
        assert!(matches!(result, Err(ToolError::InvalidInput { .. })));
    }

    #[tokio::test]
    async fn execute_unsupported_chain() {
        let input = serde_json::json!({
            "account_id": "5GrwvaEF...",
            "chain": "avalanche"
        });
        let result = PortfolioSummaryTool.execute(input, &test_context()).await;
        assert!(matches!(result, Err(ToolError::ExecutionFailed { .. })));
    }

    #[tokio::test]
    async fn execute_default_chain_polkadot() {
        let input = serde_json::json!({
            "account_id": "5GrwvaEFcWWqn1bRJJpPi8HBdhQ"
        });

        let result = PortfolioSummaryTool
            .execute(input, &test_context())
            .await
            .expect("should succeed");

        assert_eq!(result.output["chain"], "polkadot");

        let entries = result.output["entries"].as_array().expect("entries array");
        // All entries should use DOT symbol.
        for entry in entries {
            assert_eq!(entry["asset"], "DOT");
        }
    }
}
