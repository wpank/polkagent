//! Vesting schedule tool.
//!
//! [`VestingScheduleTool`] queries an account's vesting schedules, reporting
//! the locked and unlocked amounts plus the next unlock block.
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
use crate::types::VestingInfo;

// ---------------------------------------------------------------------------
// VestingScheduleTool
// ---------------------------------------------------------------------------

/// Build the storage key for `Vesting::Vesting(account_id)`.
fn build_vesting_storage_key(account_id: &str) -> Vec<u8> {
    let mut key = b"Vesting:Vesting:".to_vec();
    key.extend_from_slice(account_id.as_bytes());
    key
}

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
pub struct VestingScheduleTool {
    chain_client: Arc<dyn ChainClient>,
}

impl VestingScheduleTool {
    /// Create a new `VestingScheduleTool` backed by the given chain client.
    pub fn new(chain_client: Arc<dyn ChainClient>) -> Self {
        Self { chain_client }
    }
}

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

        let storage_key = build_vesting_storage_key(account_id);
        let chain_profile = ChainProfileId::new(chain);

        let default_info = || VestingInfo {
            schedules: vec![],
            total_unlocked: 0,
            total_locked: 0,
            next_unlock_block: None,
        };

        let info = match self
            .chain_client
            .query_storage(&storage_key, None, chain_profile)
            .await
        {
            Ok(Some(bytes)) => serde_json::from_slice::<VestingInfo>(&bytes).unwrap_or_else(|e| {
                warn!(
                    account = account_id,
                    chain,
                    error = %e,
                    "failed to decode vesting info, using defaults"
                );
                default_info()
            }),
            Ok(None) => {
                debug!(account = account_id, chain, "no vesting data on chain");
                default_info()
            }
            Err(e) => {
                warn!(
                    account = account_id,
                    chain,
                    error = %e,
                    "chain query failed for vesting, falling back to defaults"
                );
                default_info()
            }
        };

        let output = serde_json::to_value(&info).map_err(|e| ToolError::ExecutionFailed {
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
// Test assertions deliberately unwrap fixtures so failures retain precise context.
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::tests::MockChainClient;
    use crate::types::{VestingInfo, VestingSchedule};
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

    fn make_tool() -> VestingScheduleTool {
        VestingScheduleTool::new(Arc::new(MockChainClient::new()))
    }

    #[test]
    fn spec_name_and_grant() {
        let spec = make_tool().spec();
        assert_eq!(spec.name, "polkagent.treasury.vesting_schedule");
        assert_eq!(spec.required_grant.as_deref(), Some("chain.query"));
    }

    #[test]
    fn spec_has_input_schema() {
        let spec = make_tool().spec();
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

        let result = make_tool()
            .execute(input, &test_context())
            .await
            .expect("should succeed");

        let info: VestingInfo = serde_json::from_value(result.output).expect("deserialize output");
        assert!(info.schedules.is_empty());
        assert_eq!(info.total_unlocked, 0);
        assert_eq!(info.total_locked, 0);
        assert_eq!(info.next_unlock_block, None);
    }

    #[tokio::test]
    async fn execute_with_chain_data() {
        let mut client = MockChainClient::new();
        let info = VestingInfo {
            schedules: vec![VestingSchedule {
                locked: 10_000_000_000_000,
                per_block: 1_000_000,
                starting_block: 15_000_000,
            }],
            total_unlocked: 2_000_000_000_000,
            total_locked: 8_000_000_000_000,
            next_unlock_block: Some(18_000_100),
        };
        let key = build_vesting_storage_key("5GrwvaEFcWWqn1bRJJpPi8HBdhQ");
        client.insert_storage(key, serde_json::to_vec(&info).expect("serialize"));

        let tool = VestingScheduleTool::new(Arc::new(client));
        let input = serde_json::json!({
            "account_id": "5GrwvaEFcWWqn1bRJJpPi8HBdhQ",
            "chain": "polkadot"
        });

        let result = tool
            .execute(input, &test_context())
            .await
            .expect("should succeed");

        let out: VestingInfo = serde_json::from_value(result.output).expect("deserialize output");
        assert_eq!(out.schedules.len(), 1);
        assert_eq!(out.total_locked, 8_000_000_000_000);
        assert_eq!(out.total_unlocked, 2_000_000_000_000);
        assert_eq!(out.next_unlock_block, Some(18_000_100));
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
            "chain": "cosmos"
        });
        let result = make_tool().execute(input, &test_context()).await;
        assert!(matches!(result, Err(ToolError::ExecutionFailed { .. })));
    }

    #[tokio::test]
    async fn execute_default_chain() {
        let input = serde_json::json!({
            "account_id": "5GrwvaEFcWWqn1bRJJpPi8HBdhQ"
        });

        let result = make_tool()
            .execute(input, &test_context())
            .await
            .expect("should succeed");

        // Should succeed with default chain (polkadot).
        let info: VestingInfo = serde_json::from_value(result.output).expect("deserialize output");
        assert!(info.schedules.is_empty());
    }
}
