//! Portfolio summary tool.
//!
//! [`PortfolioSummaryTool`] provides an aggregated view of all assets held by
//! an account: native balances, staking positions, vesting schedules, and
//! crowdloan contributions. Each line item is tagged with its source category.
//!
//! **Grant requirement:** `chain.query`

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tracing::{debug, warn};

use polkagent_chain_trait::{ChainClient, ChainProfileId};
use polkagent_core::config::DataClassification;
use polkagent_tool::registry::{ToolContext, ToolError, ToolHandler, ToolResult, ToolSpec};

use crate::balance::{build_balance_storage_key, parse_account_input};
use crate::types::{AccountBalance, PortfolioEntry, PortfolioSource, StakingPosition, VestingInfo};

// ---------------------------------------------------------------------------
// PortfolioSummaryTool
// ---------------------------------------------------------------------------

/// Build the storage key for `Staking::Ledger(account_id)`.
fn build_staking_key(account_id: &str) -> Vec<u8> {
    let mut key = b"Staking:Ledger:".to_vec();
    key.extend_from_slice(account_id.as_bytes());
    key
}

/// Build the storage key for `Vesting::Vesting(account_id)`.
fn build_vesting_key(account_id: &str) -> Vec<u8> {
    let mut key = b"Vesting:Vesting:".to_vec();
    key.extend_from_slice(account_id.as_bytes());
    key
}

/// Build the storage key for `Crowdloan::Contributions(account_id)`.
fn build_crowdloan_key(account_id: &str) -> Vec<u8> {
    let mut key = b"Crowdloan:Contributions:".to_vec();
    key.extend_from_slice(account_id.as_bytes());
    key
}

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
pub struct PortfolioSummaryTool {
    chain_client: Arc<dyn ChainClient>,
}

impl PortfolioSummaryTool {
    /// Create a new `PortfolioSummaryTool` backed by the given chain client.
    pub fn new(chain_client: Arc<dyn ChainClient>) -> Self {
        Self { chain_client }
    }

    /// Query a storage key and decode the result as type `T`.
    ///
    /// Returns `None` if the key is absent or decoding fails (with a warning
    /// log), letting the caller fall back to a zero/default value.
    async fn query_and_decode<T: serde::de::DeserializeOwned>(
        &self,
        storage_key: &[u8],
        chain_profile: &ChainProfileId,
        account_id: &str,
        chain: &str,
        source_label: &str,
    ) -> Option<T> {
        match self
            .chain_client
            .query_storage(storage_key, None, chain_profile.clone())
            .await
        {
            Ok(Some(bytes)) => serde_json::from_slice(&bytes)
                .map_err(|e| {
                    warn!(
                        account = account_id,
                        chain,
                        source = source_label,
                        error = %e,
                        "failed to decode portfolio source data"
                    );
                })
                .ok(),
            Ok(None) => None,
            Err(e) => {
                warn!(
                    account = account_id,
                    chain,
                    source = source_label,
                    error = %e,
                    "chain query failed for portfolio source"
                );
                None
            }
        }
    }
}

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

        let chain_profile = ChainProfileId::new(chain);

        // Query native balance.
        let native_balance = self
            .query_and_decode::<AccountBalance>(
                &build_balance_storage_key(account_id),
                &chain_profile,
                account_id,
                chain,
                "balance",
            )
            .await
            .map(|b| b.free)
            .unwrap_or(0);

        // Query staking position.
        let staking_balance = self
            .query_and_decode::<StakingPosition>(
                &build_staking_key(account_id),
                &chain_profile,
                account_id,
                chain,
                "staking",
            )
            .await
            .map(|p| p.total_stake)
            .unwrap_or(0);

        // Query vesting info.
        let vesting_balance = self
            .query_and_decode::<VestingInfo>(
                &build_vesting_key(account_id),
                &chain_profile,
                account_id,
                chain,
                "vesting",
            )
            .await
            .map(|v| v.total_locked)
            .unwrap_or(0);

        // Query crowdloan contributions (stored as a simple u128 balance).
        let crowdloan_balance = self
            .query_and_decode::<PortfolioEntry>(
                &build_crowdloan_key(account_id),
                &chain_profile,
                account_id,
                chain,
                "crowdloan",
            )
            .await
            .map(|e| e.balance)
            .unwrap_or(0);

        let entries: Vec<PortfolioEntry> = vec![
            PortfolioEntry {
                asset: symbol.to_string(),
                balance: native_balance,
                value_usd: None,
                source: PortfolioSource::Native,
            },
            PortfolioEntry {
                asset: symbol.to_string(),
                balance: staking_balance,
                value_usd: None,
                source: PortfolioSource::Staking,
            },
            PortfolioEntry {
                asset: symbol.to_string(),
                balance: vesting_balance,
                value_usd: None,
                source: PortfolioSource::Vesting,
            },
            PortfolioEntry {
                asset: symbol.to_string(),
                balance: crowdloan_balance,
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

    fn make_tool() -> PortfolioSummaryTool {
        PortfolioSummaryTool::new(Arc::new(MockChainClient::new()))
    }

    #[test]
    fn spec_name_and_grant() {
        let spec = make_tool().spec();
        assert_eq!(spec.name, "polkagent.treasury.portfolio_summary");
        assert_eq!(spec.required_grant.as_deref(), Some("chain.query"));
    }

    #[test]
    fn spec_schema_has_required_account_id() {
        let spec = make_tool().spec();
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

        let result = make_tool()
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
    async fn execute_with_chain_data() {
        let mut client = MockChainClient::new();
        let account = "5GrwvaEFcWWqn1bRJJpPi8HBdhQ";

        // Insert native balance data.
        let balance = AccountBalance {
            free: 5_000_000_000_000,
            reserved: 100_000_000,
            frozen: 0,
            total: 5_000_100_000_000,
            asset_id: None,
            symbol: "DOT".to_string(),
            decimals: 10,
        };
        client.insert_storage(
            build_balance_storage_key(account),
            serde_json::to_vec(&balance).expect("serialize"),
        );

        // Insert staking data.
        let staking = StakingPosition {
            role: crate::types::StakingRole::Nominator,
            active_stake: 3_000_000_000_000,
            total_stake: 3_000_000_000_000,
            unlocking: vec![],
            reward_destination: "Staked".to_string(),
            nominations: vec![],
        };
        client.insert_storage(
            build_staking_key(account),
            serde_json::to_vec(&staking).expect("serialize"),
        );

        let tool = PortfolioSummaryTool::new(Arc::new(client));
        let input = serde_json::json!({
            "account_id": account,
            "chain": "polkadot"
        });

        let result = tool
            .execute(input, &test_context())
            .await
            .expect("should succeed");

        let entries = result.output["entries"].as_array().expect("entries array");
        // Native entry should have the queried free balance.
        let native = entries
            .iter()
            .find(|e| e["source"] == "native")
            .expect("native");
        assert_eq!(native["balance"], serde_json::json!(5_000_000_000_000_u128));
        // Staking entry should have the queried total_stake.
        let staking_entry = entries
            .iter()
            .find(|e| e["source"] == "staking")
            .expect("staking");
        assert_eq!(
            staking_entry["balance"],
            serde_json::json!(3_000_000_000_000_u128)
        );
    }

    #[tokio::test]
    async fn execute_missing_account_id() {
        let input = serde_json::json!({ "chain": "polkadot" });
        let result = make_tool().execute(input, &test_context()).await;
        assert!(matches!(result, Err(ToolError::InvalidInput { .. })));
    }

    #[tokio::test]
    async fn execute_unsupported_chain() {
        let input = serde_json::json!({
            "account_id": "5GrwvaEF...",
            "chain": "avalanche"
        });
        let result = make_tool().execute(input, &test_context()).await;
        assert!(matches!(result, Err(ToolError::ExecutionFailed { .. })));
    }

    #[tokio::test]
    async fn execute_default_chain_polkadot() {
        let input = serde_json::json!({
            "account_id": "5GrwvaEFcWWqn1bRJJpPi8HBdhQ"
        });

        let result = make_tool()
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
