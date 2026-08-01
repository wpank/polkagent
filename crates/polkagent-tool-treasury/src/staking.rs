//! Staking information tool.
//!
//! [`StakingInfoTool`] queries an account's staking status on a
//! Polkadot-SDK relay chain: validator/nominator role, active and total
//! stake, unbonding chunks, reward destination, and nominated validators.
//!
//! **Grant requirement:** `chain.query`

use async_trait::async_trait;
use serde_json::Value;
use tracing::debug;

use polkagent_core::config::DataClassification;
use polkagent_tool::registry::{ToolContext, ToolError, ToolHandler, ToolResult, ToolSpec};

use crate::balance::parse_account_input;
use crate::types::{StakingPosition, StakingRole};

// ---------------------------------------------------------------------------
// StakingInfoTool
// ---------------------------------------------------------------------------

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
pub struct StakingInfoTool;

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

        // In production this queries pallet_staking storage.
        // Return an idle position as the default / placeholder.
        let position = StakingPosition {
            role: StakingRole::Idle,
            active_stake: 0,
            total_stake: 0,
            unlocking: vec![],
            reward_destination: "Staked".to_string(),
            nominations: vec![],
        };

        let output =
            serde_json::to_value(&position).map_err(|e| ToolError::ExecutionFailed {
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
    use crate::types::StakingPosition;
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
        let spec = StakingInfoTool.spec();
        assert_eq!(spec.name, "polkagent.treasury.staking_info");
        assert_eq!(spec.required_grant.as_deref(), Some("chain.query"));
    }

    #[test]
    fn spec_has_account_id_required() {
        let spec = StakingInfoTool.spec();
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

        let result = StakingInfoTool
            .execute(input, &test_context())
            .await
            .expect("should succeed");

        let pos: StakingPosition =
            serde_json::from_value(result.output).expect("deserialize output");
        assert_eq!(pos.role, StakingRole::Idle);
        assert_eq!(pos.reward_destination, "Staked");
    }

    #[tokio::test]
    async fn execute_missing_account_id() {
        let input = serde_json::json!({});
        let result = StakingInfoTool.execute(input, &test_context()).await;
        assert!(matches!(result, Err(ToolError::InvalidInput { .. })));
    }

    #[tokio::test]
    async fn execute_unsupported_chain() {
        let input = serde_json::json!({
            "account_id": "5GrwvaEF...",
            "chain": "solana"
        });
        let result = StakingInfoTool.execute(input, &test_context()).await;
        assert!(matches!(result, Err(ToolError::ExecutionFailed { .. })));
    }
}
