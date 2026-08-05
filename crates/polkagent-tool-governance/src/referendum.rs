//! Referendum lookup tool.
//!
//! The [`ReferendumLookupTool`] queries a referendum by its index and returns
//! detailed information including status, track, tally, proposer, and timeline
//! events.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tracing::debug;

use polkagent_chain_trait::ChainClient;
use polkagent_codec::ScaleDecoder;
use polkagent_core::config::DataClassification;
use polkagent_tool::registry::{ToolContext, ToolError, ToolHandler, ToolResult, ToolSpec};

use crate::error::GovernanceError;
use crate::types::{Referendum, ReferendumStatus, TimelineEvent};

// ---------------------------------------------------------------------------
// ReferendumLookupTool
// ---------------------------------------------------------------------------

/// Looks up an `OpenGov` referendum by its index.
///
/// Returns the referendum's status, track, tally (ayes/nays/support),
/// proposer, and timeline events. Requires `chain.query` grant for
/// read-only chain state access.
pub struct ReferendumLookupTool {
    chain_client: Arc<dyn ChainClient>,
}

impl ReferendumLookupTool {
    /// Create a new `ReferendumLookupTool` backed by the given chain client.
    pub fn new(chain_client: Arc<dyn ChainClient>) -> Self {
        Self { chain_client }
    }

    /// Query a referendum by index using the chain client.
    ///
    /// In a production implementation this would construct the appropriate
    /// storage key for `Referenda::ReferendumInfoFor(index)` and decode
    /// the SCALE-encoded response. The current implementation returns a
    /// placeholder that downstream integrations will replace with real
    /// chain queries.
    async fn lookup_referendum(&self, index: u32) -> Result<Referendum, GovernanceError> {
        // Verify chain connectivity.
        self.chain_client
            .health()
            .await
            .map_err(GovernanceError::Chain)?;

        // Build the storage key for Referenda::ReferendumInfoFor.
        // In production this would use proper SCALE encoding; here we
        // construct a representative key to query via the chain client.
        let storage_key = build_referendum_storage_key(index);

        let result = self
            .chain_client
            .query_storage(
                &storage_key,
                None,
                polkagent_chain_trait::ChainProfileId::new("default"),
            )
            .await
            .map_err(GovernanceError::Chain)?;

        match result {
            Some(bytes) => decode_referendum(index, &bytes),
            None => Err(GovernanceError::ReferendumNotFound { index }),
        }
    }
}

#[async_trait]
impl ToolHandler for ReferendumLookupTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "polkagent.governance.referendum_lookup".to_string(),
            description: "Look up an OpenGov referendum by index. Returns status, track, \
                          tally (ayes/nays/support), proposer, and timeline events."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "index": {
                        "type": "integer",
                        "description": "The referendum index to look up"
                    }
                },
                "required": ["index"]
            }),
            required_grant: Some("chain.query".to_string()),
            output_classification: DataClassification::Public,
        }
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let index =
            input
                .get("index")
                .and_then(Value::as_u64)
                .ok_or_else(|| ToolError::InvalidInput {
                    reason: "missing or invalid 'index' field — must be a non-negative integer"
                        .to_string(),
                })?;

        let index = u32::try_from(index).map_err(|_| ToolError::InvalidInput {
            reason: format!("referendum index {index} exceeds u32 range"),
        })?;

        debug!(
            referendum_index = index,
            agent = %context.agent_id,
            "looking up referendum"
        );

        let referendum =
            self.lookup_referendum(index)
                .await
                .map_err(|e| ToolError::ExecutionFailed {
                    reason: e.to_string(),
                })?;

        let output = serde_json::to_value(&referendum).map_err(|e| ToolError::ExecutionFailed {
            reason: format!("failed to serialize referendum: {e}"),
        })?;

        Ok(ToolResult {
            output,
            classification: DataClassification::Public,
            artifacts: vec![],
        })
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build the storage key for `Referenda::ReferendumInfoFor(index)`.
///
/// In a full implementation this would use the metadata-derived storage
/// prefix hash. Here we produce a deterministic key suitable for the
/// [`ChainClient::query_storage`] interface.
fn build_referendum_storage_key(index: u32) -> Vec<u8> {
    let mut key = b"Referenda:ReferendumInfoFor:".to_vec();
    key.extend_from_slice(&index.to_le_bytes());
    key
}

/// Decode referendum bytes into a [`Referendum`].
///
/// Attempts SCALE decoding first (matching the on-chain
/// `ReferendumInfo<..>` enum layout), then falls back to JSON
/// deserialization for mock/test data or API responses that arrive
/// pre-serialized.
fn decode_referendum(index: u32, bytes: &[u8]) -> Result<Referendum, GovernanceError> {
    // First, try SCALE decoding.
    if let Ok(referendum) = try_decode_scale_referendum(index, bytes) {
        return Ok(referendum);
    }

    // Fall back to JSON deserialization (mock chain clients and some
    // API adapters store referenda as JSON blobs).
    serde_json::from_slice(bytes).map_err(|e| GovernanceError::Decode {
        message: format!(
            "failed to decode referendum {index}: SCALE decode failed and JSON fallback failed: {e}"
        ),
    })
}

/// Attempt to decode a SCALE-encoded `ReferendumInfo` enum.
///
/// The on-chain layout (simplified) is:
///
/// ```text
/// enum ReferendumInfo {
///     Ongoing(ReferendumStatus) = 0,
///     Approved(block, …)       = 1,
///     Rejected(block, …)       = 2,
///     Cancelled(block, …)      = 3,
///     TimedOut(block, …)       = 4,
///     Killed(block)            = 5,
/// }
/// ```
///
/// For `Ongoing` referenda we extract the track, origin, tally, and
/// submission block. For terminal states we record the deciding block
/// and appropriate status.
fn try_decode_scale_referendum(index: u32, bytes: &[u8]) -> Result<Referendum, GovernanceError> {
    let mut dec = ScaleDecoder::new(bytes);

    // Enum discriminant.
    let variant = dec.decode_u8().map_err(|e| GovernanceError::Decode {
        message: format!("referendum {index}: failed to read variant byte: {e}"),
    })?;

    match variant {
        // Ongoing
        0 => decode_ongoing_referendum(index, &mut dec),
        // Approved
        1 => decode_terminal_referendum(index, &mut dec, ReferendumStatus::Approved),
        // Rejected
        2 => decode_terminal_referendum(index, &mut dec, ReferendumStatus::Rejected),
        // Cancelled
        3 => decode_terminal_referendum(index, &mut dec, ReferendumStatus::Cancelled),
        // TimedOut
        4 => decode_terminal_referendum(index, &mut dec, ReferendumStatus::TimedOut),
        // Killed
        5 => decode_killed_referendum(index, &mut dec),
        _ => Err(GovernanceError::Decode {
            message: format!("referendum {index}: unknown ReferendumInfo variant: {variant}"),
        }),
    }
}

/// Decode the body of an `Ongoing` referendum variant.
///
/// Expected SCALE fields (simplified from the Polkadot runtime):
///   - track: u16
///   - origin: Compact-encoded string (we read as bytes and attempt UTF-8)
///   - … (several fields we skip) …
///   - tally: { ayes: u128, nays: u128, support: u128 }
///   - `submission_block`: u32
///   - submitter: [u8; 32] (`AccountId`)
///
/// This is a best-effort decoder — it extracts what it can from the
/// beginning of the SCALE payload. If the layout does not match our
/// expectations at any point, we return an error so the JSON fallback
/// can take over.
fn decode_ongoing_referendum(
    index: u32,
    dec: &mut ScaleDecoder<'_>,
) -> Result<Referendum, GovernanceError> {
    // track: u16
    let track = dec
        .decode_u16()
        .map_err(|e| scale_field_err(index, "track", &e))?;

    // origin: SCALE-encoded string (compact-length-prefixed UTF-8).
    let origin = dec
        .decode_string()
        .map_err(|e| scale_field_err(index, "origin", &e))?;

    // proposal_hash: [u8; 32] — skip.
    let _proposal_hash: [u8; 32] = dec
        .decode_fixed_array::<32>()
        .map_err(|e| scale_field_err(index, "proposal_hash", &e))?;

    // enactment: enum { After(u32), At(u32) } — 1 byte variant + u32.
    let _enactment_variant = dec
        .decode_u8()
        .map_err(|e| scale_field_err(index, "enactment variant", &e))?;
    let _enactment_block = dec
        .decode_u32()
        .map_err(|e| scale_field_err(index, "enactment block", &e))?;

    // submitted: u32 (block number)
    let submitted_block = dec
        .decode_u32()
        .map_err(|e| scale_field_err(index, "submitted block", &e))?;

    // submission_deposit: Option<(AccountId, u128)>
    let has_deposit = dec
        .decode_u8()
        .map_err(|e| scale_field_err(index, "submission_deposit option", &e))?;
    if has_deposit == 1 {
        let _depositor: [u8; 32] = dec
            .decode_fixed_array::<32>()
            .map_err(|e| scale_field_err(index, "depositor", &e))?;
        let _amount = dec
            .decode_u128()
            .map_err(|e| scale_field_err(index, "deposit amount", &e))?;
    }

    // decision_deposit: Option<(AccountId, u128)>
    let has_decision_deposit = dec
        .decode_u8()
        .map_err(|e| scale_field_err(index, "decision_deposit option", &e))?;
    if has_decision_deposit == 1 {
        let _depositor: [u8; 32] = dec
            .decode_fixed_array::<32>()
            .map_err(|e| scale_field_err(index, "decision depositor", &e))?;
        let _amount = dec
            .decode_u128()
            .map_err(|e| scale_field_err(index, "decision deposit amount", &e))?;
    }

    // deciding: Option<DecidingStatus { since: u32, confirming: Option<u32> }>
    let has_deciding = dec
        .decode_u8()
        .map_err(|e| scale_field_err(index, "deciding option", &e))?;
    let status = if has_deciding == 1 {
        let _since = dec
            .decode_u32()
            .map_err(|e| scale_field_err(index, "deciding since", &e))?;
        let has_confirming = dec
            .decode_u8()
            .map_err(|e| scale_field_err(index, "confirming option", &e))?;
        if has_confirming == 1 {
            let _confirming_since = dec
                .decode_u32()
                .map_err(|e| scale_field_err(index, "confirming since", &e))?;
            ReferendumStatus::Confirming
        } else {
            ReferendumStatus::Deciding
        }
    } else {
        ReferendumStatus::Preparing
    };

    // tally: { ayes: u128, nays: u128, support: u128 }
    let ayes = dec
        .decode_u128()
        .map_err(|e| scale_field_err(index, "tally ayes", &e))?;
    let nays = dec
        .decode_u128()
        .map_err(|e| scale_field_err(index, "tally nays", &e))?;
    let support = dec
        .decode_u128()
        .map_err(|e| scale_field_err(index, "tally support", &e))?;

    // alarm: Option<(u32, (u32, u32))> — skip (we have enough data).
    // submitter AccountId — try to read but tolerate failure.
    let proposer = if dec.remaining() >= 32 {
        match dec.decode_fixed_array::<32>() {
            Ok(acct) => format!("0x{}", hex_encode_bytes(&acct)),
            Err(_) => "unknown".to_string(),
        }
    } else {
        "unknown".to_string()
    };

    Ok(Referendum {
        index,
        track,
        origin,
        status,
        ayes,
        nays,
        support,
        proposer,
        timeline: vec![TimelineEvent {
            event: "submitted".to_string(),
            block_number: u64::from(submitted_block),
            timestamp: None,
        }],
    })
}

/// Build a [`GovernanceError::Decode`] for a specific field failure.
fn scale_field_err(index: u32, field: &str, e: &polkagent_codec::CodecError) -> GovernanceError {
    GovernanceError::Decode {
        message: format!("referendum {index}: failed to decode {field}: {e}"),
    }
}

/// Decode a terminal referendum variant (`Approved`, `Rejected`,
/// `Cancelled`, `TimedOut`).
///
/// These share a common prefix: a `u32` block number indicating when
/// the terminal event occurred. We use that as the single timeline
/// event.
fn decode_terminal_referendum(
    index: u32,
    dec: &mut ScaleDecoder<'_>,
    status: ReferendumStatus,
) -> Result<Referendum, GovernanceError> {
    let block = dec.decode_u32().map_err(|e| GovernanceError::Decode {
        message: format!("referendum {index}: failed to decode terminal block: {e}"),
    })?;

    Ok(Referendum {
        index,
        track: 0,
        origin: String::new(),
        status,
        ayes: 0,
        nays: 0,
        support: 0,
        proposer: String::new(),
        timeline: vec![TimelineEvent {
            event: status.to_string(),
            block_number: u64::from(block),
            timestamp: None,
        }],
    })
}

/// Decode a `Killed` referendum variant.
///
/// Contains only the block number at which the referendum was killed.
fn decode_killed_referendum(
    index: u32,
    dec: &mut ScaleDecoder<'_>,
) -> Result<Referendum, GovernanceError> {
    let block = dec.decode_u32().map_err(|e| GovernanceError::Decode {
        message: format!("referendum {index}: failed to decode killed block: {e}"),
    })?;

    Ok(Referendum {
        index,
        track: 0,
        origin: String::new(),
        status: ReferendumStatus::Killed,
        ayes: 0,
        nays: 0,
        support: 0,
        proposer: String::new(),
        timeline: vec![TimelineEvent {
            event: "killed".to_string(),
            block_number: u64::from(block),
            timestamp: None,
        }],
    })
}

/// Hex-encode a byte slice (no `0x` prefix — caller adds it).
fn hex_encode_bytes(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::MockChainClient;
    use crate::types::{ReferendumStatus, TimelineEvent};
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

    #[test]
    fn spec_has_correct_name() {
        let client = Arc::new(MockChainClient::new());
        let tool = ReferendumLookupTool::new(client);
        let spec = tool.spec();
        assert_eq!(spec.name, "polkagent.governance.referendum_lookup");
    }

    #[test]
    fn spec_requires_chain_query_grant() {
        let client = Arc::new(MockChainClient::new());
        let tool = ReferendumLookupTool::new(client);
        let spec = tool.spec();
        assert_eq!(spec.required_grant, Some("chain.query".to_string()));
    }

    #[test]
    fn spec_has_index_in_schema() {
        let client = Arc::new(MockChainClient::new());
        let tool = ReferendumLookupTool::new(client);
        let spec = tool.spec();
        let props = &spec.input_schema["properties"];
        assert!(props.get("index").is_some());
        let required = spec.input_schema["required"].as_array();
        assert!(required.is_some());
        let required_names: Vec<&str> = required
            .unwrap_or_else(|| panic!("required should be an array"))
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert!(required_names.contains(&"index"));
    }

    #[tokio::test]
    async fn execute_missing_index() {
        let client = Arc::new(MockChainClient::new());
        let tool = ReferendumLookupTool::new(client);
        let result = tool.execute(serde_json::json!({}), &test_context()).await;
        assert!(matches!(result, Err(ToolError::InvalidInput { .. })));
    }

    #[tokio::test]
    async fn execute_invalid_index_type() {
        let client = Arc::new(MockChainClient::new());
        let tool = ReferendumLookupTool::new(client);
        let input = serde_json::json!({ "index": "not_a_number" });
        let result = tool.execute(input, &test_context()).await;
        assert!(matches!(result, Err(ToolError::InvalidInput { .. })));
    }

    #[tokio::test]
    async fn execute_referendum_not_found() {
        let client = Arc::new(MockChainClient::new());
        let tool = ReferendumLookupTool::new(client);
        // Index 999 is not in the mock data.
        let input = serde_json::json!({ "index": 999 });
        let result = tool.execute(input, &test_context()).await;
        assert!(matches!(result, Err(ToolError::ExecutionFailed { .. })));
    }

    #[tokio::test]
    async fn execute_success() {
        let mut client = MockChainClient::new();

        let ref_data = Referendum {
            index: 42,
            track: 1,
            origin: "SmallSpender".to_string(),
            status: ReferendumStatus::Deciding,
            ayes: 1_000_000,
            nays: 500_000,
            support: 1_500_000,
            proposer: "5GrwvaEF...".to_string(),
            timeline: vec![TimelineEvent {
                event: "submitted".to_string(),
                block_number: 10_000,
                timestamp: None,
            }],
        };
        let bytes = serde_json::to_vec(&ref_data).unwrap_or_else(|e| panic!("serialize ref: {e}"));
        let key = build_referendum_storage_key(42);
        client.insert_storage(key, bytes);

        let tool = ReferendumLookupTool::new(Arc::new(client));
        let input = serde_json::json!({ "index": 42 });
        let result = tool
            .execute(input, &test_context())
            .await
            .unwrap_or_else(|e| panic!("execute failed: {e}"));

        assert_eq!(result.output["index"], 42);
        assert_eq!(result.output["status"], "deciding");
        assert_eq!(result.output["track"], 1);
        assert_eq!(result.classification, DataClassification::Public);
    }

    #[test]
    fn build_storage_key_deterministic() {
        let k1 = build_referendum_storage_key(42);
        let k2 = build_referendum_storage_key(42);
        assert_eq!(k1, k2);
    }

    #[test]
    fn build_storage_key_different_indices() {
        let k1 = build_referendum_storage_key(1);
        let k2 = build_referendum_storage_key(2);
        assert_ne!(k1, k2);
    }
}
