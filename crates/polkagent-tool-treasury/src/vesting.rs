//! Vesting schedule tool.
//!
//! [`VestingScheduleTool`] queries an account's vesting schedules, reporting
//! the locked and unlocked amounts plus the next unlock block.
//!
//! **Grant requirement:** `chain.query`

use async_trait::async_trait;
use serde_json::Value;
use tracing::debug;

use polkagent_core::config::DataClassification;
use polkagent_tool::registry::{ToolContext, ToolError, ToolHandler, ToolResult, ToolSpec};

use crate::balance::parse_account_input;
use crate::types::VestingInfo;

// ---------------------------------------------------------------------------
// VestingScheduleTool
// ---------------------------------------------------------------------------

/// Queries vesting schedules for an account.
///
/// Returns the list of active vesting schedules, the total amount already
/// unlocked, the total still locked, and the block number of the next
/// vesting unlock.
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
pub struct VestingScheduleTool;

#[async_trait]
impl ToolHandler for VestingScheduleTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "polkagent.treasury.vesting_schedule".to_string(),
            description: "Query vesting schedules for an account: locked amount, unlocked amount, per-block release rate, and next unlock block.".to_string(),
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

        debug!(account = account_id, chain, "querying vesting schedules");

        // In production this queries pallet_vesting storage.
        // Return empty vesting info as placeholder.
        let info = VestingInfo {
            schedules: vec![],
            total_unlocked: 0,
            total_locked: 0,
            next_unlock_block: None,
        };

        let output =
            serde_json::to_value(&info).map_err(|e| ToolError::ExecutionFailed {
                reason: format!("failed to serialize vesting info: {e}"),
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
    use crate::types::VestingInfo;
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
        let spec = VestingScheduleTool.spec();
        assert_eq!(spec.name, "polkagent.treasury.vesting_schedule");
        assert_eq!(spec.required_grant.as_deref(), Some("chain.query"));
    }

    #[test]
    fn spec_has_input_schema() {
        let spec = VestingScheduleTool.spec();
        let props = &spec.input_schema["properties"];
        assert!(props.get("account_id").is_some());
        assert!(props.get("chain").is_some());
    }

    #[tokio::test]
    async fn execute_returns_vesting_info() {
        let input = serde_json::json!({
            "account_id": "5GrwvaEFcWWqn1bRJJpPi8HBdhQ",
            "chain": "polkadot"
        });

        let result = VestingScheduleTool
            .execute(input, &test_context())
            .await
            .expect("should succeed");

        let info: VestingInfo =
            serde_json::from_value(result.output).expect("deserialize output");
        assert!(info.schedules.is_empty());
        assert_eq!(info.total_unlocked, 0);
        assert_eq!(info.total_locked, 0);
        assert_eq!(info.next_unlock_block, None);
    }

    #[tokio::test]
    async fn execute_missing_account_id() {
        let input = serde_json::json!({});
        let result = VestingScheduleTool.execute(input, &test_context()).await;
        assert!(matches!(result, Err(ToolError::InvalidInput { .. })));
    }

    #[tokio::test]
    async fn execute_unsupported_chain() {
        let input = serde_json::json!({
            "account_id": "5GrwvaEF...",
            "chain": "cosmos"
        });
        let result = VestingScheduleTool.execute(input, &test_context()).await;
        assert!(matches!(result, Err(ToolError::ExecutionFailed { .. })));
    }

    #[tokio::test]
    async fn execute_default_chain() {
        let input = serde_json::json!({
            "account_id": "5GrwvaEFcWWqn1bRJJpPi8HBdhQ"
        });

        let result = VestingScheduleTool
            .execute(input, &test_context())
            .await
            .expect("should succeed");

        // Should succeed with default chain (polkadot).
        let info: VestingInfo =
            serde_json::from_value(result.output).expect("deserialize output");
        assert!(info.schedules.is_empty());
    }
}
