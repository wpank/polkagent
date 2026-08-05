//! Staking information tool.
//!
//! [`StakingInfoTool`] queries an account's staking status on a
//! Polkadot-SDK relay chain: validator/nominator role, active and total
//! stake, unbonding chunks, reward destination, and nominated validators.
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
use crate::types::{StakingPosition, StakingRole};

// ---------------------------------------------------------------------------
// StakingInfoTool
// ---------------------------------------------------------------------------

/// Build the storage key for `Staking::Ledger(account_id)`.
fn build_staking_storage_key(account_id: &str) -> Vec<u8> {
    let mut key = b"Staking:Ledger:".to_vec();
    key.extend_from_slice(account_id.as_bytes());
    key
}

/// Queries the staking position of an account on a Polkadot-SDK relay chain.
///
/// Returns the account's role (validator, nominator, or idle), the active
/// and total bonded amounts, any unbonding chunks, the reward destination,
/// and the list of nominated validators.
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
pub struct StakingInfoTool {
    chain_client: Arc<dyn ChainClient>,
}

impl StakingInfoTool {
    /// Create a new `StakingInfoTool` backed by the given chain client.
    pub fn new(chain_client: Arc<dyn ChainClient>) -> Self {
        Self { chain_client }
    }
}

#[async_trait]
impl ToolHandler for StakingInfoTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "polkagent.treasury.staking_info".to_string(),
            description: "Query staking status for an account: role (validator/nominator/idle), bonded amounts, unbonding, reward destination, and nominations.".to_string(),
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

        // Validate chain is recognized.
        crate::balance::chain_token_info(chain)?;

        debug!(account = account_id, chain, "querying staking info");

        let storage_key = build_staking_storage_key(account_id);
        let chain_profile = ChainProfileId::new(chain);

        let default_position = || StakingPosition {
            role: StakingRole::Idle,
            active_stake: 0,
            total_stake: 0,
            unlocking: vec![],
            reward_destination: "Staked".to_string(),
            nominations: vec![],
        };

        let position = match self
            .chain_client
            .query_storage(&storage_key, None, chain_profile)
            .await
        {
            Ok(Some(bytes)) => {
                serde_json::from_slice::<StakingPosition>(&bytes).unwrap_or_else(|e| {
                    warn!(
                        account = account_id,
                        chain,
                        error = %e,
                        "failed to decode staking position, using defaults"
                    );
                    default_position()
                })
            }
            Ok(None) => {
                debug!(account = account_id, chain, "no staking data on chain");
                default_position()
            }
            Err(e) => {
                warn!(
                    account = account_id,
                    chain,
                    error = %e,
                    "chain query failed for staking, falling back to defaults"
                );
                default_position()
            }
        };

        let output = serde_json::to_value(&position).map_err(|e| ToolError::ExecutionFailed {
            reason: format!("failed to serialize staking position: {e}"),
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
    use crate::tests::MockChainClient;
    use crate::types::{StakingPosition, UnlockingChunk};
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

    fn make_tool() -> StakingInfoTool {
        StakingInfoTool::new(Arc::new(MockChainClient::new()))
    }

    #[test]
    fn spec_name_and_grant() {
        let spec = make_tool().spec();
        assert_eq!(spec.name, "polkagent.treasury.staking_info");
        assert_eq!(spec.required_grant.as_deref(), Some("chain.query"));
    }

    #[test]
    fn spec_has_account_id_required() {
        let spec = make_tool().spec();
        let required = spec.input_schema["required"]
            .as_array()
            .expect("required array");
        assert!(required.iter().any(|v| v.as_str() == Some("account_id")));
    }

    #[tokio::test]
    async fn execute_returns_staking_position() {
        let input = serde_json::json!({
            "account_id": "5GrwvaEFcWWqn1bRJJpPi8HBdhQ",
            "chain": "polkadot"
        });

        let result = make_tool()
            .execute(input, &test_context())
            .await
            .expect("should succeed");

        let pos: StakingPosition =
            serde_json::from_value(result.output).expect("deserialize output");
        assert_eq!(pos.role, StakingRole::Idle);
        assert_eq!(pos.reward_destination, "Staked");
    }

    #[tokio::test]
    async fn execute_with_chain_data() {
        let mut client = MockChainClient::new();
        let position = StakingPosition {
            role: StakingRole::Nominator,
            active_stake: 10_000_000_000_000,
            total_stake: 12_000_000_000_000,
            unlocking: vec![UnlockingChunk {
                value: 2_000_000_000_000,
                era: 1234,
            }],
            reward_destination: "Staked".to_string(),
            nominations: vec!["5ValidatorA".to_string()],
        };
        let key = build_staking_storage_key("5GrwvaEFcWWqn1bRJJpPi8HBdhQ");
        client.insert_storage(key, serde_json::to_vec(&position).expect("serialize"));

        let tool = StakingInfoTool::new(Arc::new(client));
        let input = serde_json::json!({
            "account_id": "5GrwvaEFcWWqn1bRJJpPi8HBdhQ",
            "chain": "polkadot"
        });

        let result = tool
            .execute(input, &test_context())
            .await
            .expect("should succeed");

        let pos: StakingPosition =
            serde_json::from_value(result.output).expect("deserialize output");
        assert_eq!(pos.role, StakingRole::Nominator);
        assert_eq!(pos.active_stake, 10_000_000_000_000);
        assert_eq!(pos.unlocking.len(), 1);
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
            "chain": "solana"
        });
        let result = make_tool().execute(input, &test_context()).await;
        assert!(matches!(result, Err(ToolError::ExecutionFailed { .. })));
    }
}
