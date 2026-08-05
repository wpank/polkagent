//! JSON-RPC 2.0 client for Substrate/Polkadot node communication.
//!
//! This module implements a raw JSON-RPC 2.0 client over HTTP using `reqwest`.
//! It supports the Substrate RPC methods needed by the [`ChainClient`] trait:
//!
//! - `state_getMetadata` - fetch runtime metadata as hex bytes
//! - `state_getStorage` - query a storage key at an optional block hash
//! - `author_submitExtrinsic` - submit a signed extrinsic
//! - `chain_getBlockHash` - get the block hash at a given height
//! - `chain_getFinalizedHead` - get the last finalized block hash
//! - `system_health` - node health status
//!
//! [`ChainClient`]: polkagent_chain_trait::ChainClient

use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::config::SubxtConfig;
use crate::error::SubxtError;

// ---------------------------------------------------------------------------
// JSON-RPC request/response types
// ---------------------------------------------------------------------------

/// A JSON-RPC 2.0 request.
#[derive(Debug, Serialize)]
pub struct JsonRpcRequest<'a> {
    /// JSON-RPC protocol version (always `"2.0"`).
    pub jsonrpc: &'a str,
    /// Request identifier.
    pub id: u64,
    /// RPC method name.
    pub method: &'a str,
    /// Optional positional parameters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

/// A JSON-RPC 2.0 response.
#[derive(Debug, Deserialize)]
pub struct JsonRpcResponse {
    /// JSON-RPC protocol version.
    #[allow(dead_code)]
    pub jsonrpc: String,
    /// Response identifier matching the request.
    pub id: u64,
    /// Successful result value, if any.
    pub result: Option<serde_json::Value>,
    /// Error object, if the request failed.
    pub error: Option<JsonRpcError>,
}

/// A JSON-RPC 2.0 error object.
#[derive(Debug, Deserialize)]
pub struct JsonRpcError {
    /// Numeric error code.
    pub code: i64,
    /// Human-readable error message.
    pub message: String,
    /// Optional additional error data.
    #[allow(dead_code)]
    pub data: Option<serde_json::Value>,
}

/// Node health status returned by `system_health`.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeHealth {
    /// Number of connected peers.
    pub peers: u64,
    /// Whether the node is syncing.
    pub is_syncing: bool,
    /// Whether the node should have peers (false for solo dev nodes).
    pub should_have_peers: bool,
}

// ---------------------------------------------------------------------------
// RpcClient
// ---------------------------------------------------------------------------

/// A JSON-RPC 2.0 client for communicating with Substrate nodes over HTTP.
///
/// Uses `reqwest::Client` for HTTP transport with configurable timeouts and
/// retry logic.
#[derive(Debug)]
pub struct RpcClient {
    http: reqwest::Client,
    config: SubxtConfig,
    next_id: AtomicU64,
}

impl RpcClient {
    /// Create a new RPC client with the given configuration.
    pub fn new(config: SubxtConfig) -> Result<Self, SubxtError> {
        let http = reqwest::Client::builder()
            .timeout(config.request_timeout)
            .connect_timeout(config.connect_timeout)
            .user_agent(&config.user_agent)
            .pool_max_idle_per_host(config.max_connections_per_endpoint)
            .build()
            .map_err(|e| SubxtError::Config {
                message: format!("failed to build HTTP client: {e}"),
            })?;

        Ok(Self {
            http,
            config,
            next_id: AtomicU64::new(1),
        })
    }

    /// Send a raw JSON-RPC request and return the result value.
    pub async fn call(
        &self,
        endpoint: &str,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, SubxtError> {
        self.call_with_retry(endpoint, method, params).await
    }

    /// Internal: call with retry logic.
    async fn call_with_retry(
        &self,
        endpoint: &str,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, SubxtError> {
        let mut last_error: Option<SubxtError> = None;

        for attempt in 0..=self.config.max_retries {
            if attempt > 0 {
                let delay = self.config.backoff_delay(attempt - 1);
                debug!(
                    method,
                    attempt,
                    delay_ms = delay.as_millis() as u64,
                    "retrying RPC call"
                );
                tokio::time::sleep(delay).await;
            }

            match self.call_once(endpoint, method, params.clone()).await {
                Ok(result) => return Ok(result),
                Err(e) => {
                    let retryable = matches!(
                        &e,
                        SubxtError::Transport {
                            retryable: true,
                            ..
                        }
                    );

                    if retryable && attempt < self.config.max_retries {
                        warn!(
                            method,
                            attempt,
                            error = %e,
                            "retryable RPC error, will retry"
                        );
                        last_error = Some(e);
                        continue;
                    }
                    return Err(e);
                }
            }
        }

        Err(last_error.unwrap_or_else(|| SubxtError::Transport {
            endpoint: endpoint.to_string(),
            message: "exhausted retries with no error captured".into(),
            retryable: false,
        }))
    }

    /// Send a single JSON-RPC request (no retry).
    async fn call_once(
        &self,
        endpoint: &str,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, SubxtError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);

        let request = JsonRpcRequest {
            jsonrpc: "2.0",
            id,
            method,
            params,
        };

        debug!(endpoint, method, id, "sending JSON-RPC request");

        let response = self
            .http
            .post(endpoint)
            .json(&request)
            .send()
            .await
            .map_err(SubxtError::from)?;

        let status = response.status();
        if !status.is_success() {
            return Err(SubxtError::Transport {
                endpoint: endpoint.to_string(),
                message: format!("HTTP {status}"),
                retryable: status.is_server_error(),
            });
        }

        let rpc_response: JsonRpcResponse =
            response
                .json()
                .await
                .map_err(|e| SubxtError::InvalidResponse {
                    endpoint: endpoint.to_string(),
                    message: format!("failed to parse JSON-RPC response: {e}"),
                })?;

        if let Some(error) = rpc_response.error {
            return Err(SubxtError::RpcError {
                endpoint: endpoint.to_string(),
                code: error.code,
                message: error.message,
            });
        }

        // A JSON-RPC response with `"result": null` is valid: the node is
        // reporting that the queried key has no on-chain value (e.g.
        // `state_getStorage` for an account that has never been funded).
        // Return `Value::Null` so that callers such as `state_get_storage`
        // can distinguish "null result" from "non-null result" and map it to
        // `None` (zero balance) rather than an error.
        Ok(rpc_response.result.unwrap_or(serde_json::Value::Null))
    }

    // -----------------------------------------------------------------------
    // Typed RPC methods
    // -----------------------------------------------------------------------

    /// Call `state_getMetadata` to fetch the runtime metadata as hex-encoded
    /// SCALE bytes.
    pub async fn state_get_metadata(
        &self,
        endpoint: &str,
        block_hash: Option<&str>,
    ) -> Result<String, SubxtError> {
        let params = block_hash.map(|h| serde_json::json!([h]));
        let result = self.call(endpoint, "state_getMetadata", params).await?;
        result
            .as_str()
            .map(String::from)
            .ok_or_else(|| SubxtError::InvalidResponse {
                endpoint: endpoint.to_string(),
                message: "state_getMetadata did not return a string".into(),
            })
    }

    /// Call `state_getStorage` to query a storage key at an optional block hash.
    ///
    /// Returns `None` if the key has no value (JSON null).
    pub async fn state_get_storage(
        &self,
        endpoint: &str,
        storage_key_hex: &str,
        block_hash: Option<&str>,
    ) -> Result<Option<String>, SubxtError> {
        let params = match block_hash {
            Some(h) => Some(serde_json::json!([storage_key_hex, h])),
            None => Some(serde_json::json!([storage_key_hex])),
        };
        let result = self.call(endpoint, "state_getStorage", params).await?;
        if result.is_null() {
            Ok(None)
        } else {
            result
                .as_str()
                .map(|s| Some(String::from(s)))
                .ok_or_else(|| SubxtError::InvalidResponse {
                    endpoint: endpoint.to_string(),
                    message: "state_getStorage did not return a string or null".into(),
                })
        }
    }

    /// Call `author_submitExtrinsic` to submit a signed extrinsic.
    ///
    /// Returns the transaction hash as a hex string.
    pub async fn author_submit_extrinsic(
        &self,
        endpoint: &str,
        extrinsic_hex: &str,
    ) -> Result<String, SubxtError> {
        let params = Some(serde_json::json!([extrinsic_hex]));
        let result = self
            .call(endpoint, "author_submitExtrinsic", params)
            .await?;
        result
            .as_str()
            .map(String::from)
            .ok_or_else(|| SubxtError::InvalidResponse {
                endpoint: endpoint.to_string(),
                message: "author_submitExtrinsic did not return a string".into(),
            })
    }

    /// Call `chain_getBlockHash` to get the block hash at a given height.
    ///
    /// If `block_number` is `None`, returns the latest block hash.
    pub async fn chain_get_block_hash(
        &self,
        endpoint: &str,
        block_number: Option<u64>,
    ) -> Result<String, SubxtError> {
        let params = block_number.map(|n| serde_json::json!([n]));
        let result = self.call(endpoint, "chain_getBlockHash", params).await?;
        result
            .as_str()
            .map(String::from)
            .ok_or_else(|| SubxtError::InvalidResponse {
                endpoint: endpoint.to_string(),
                message: "chain_getBlockHash did not return a string".into(),
            })
    }

    /// Call `chain_getFinalizedHead` to get the last finalized block hash.
    pub async fn chain_get_finalized_head(&self, endpoint: &str) -> Result<String, SubxtError> {
        let result = self.call(endpoint, "chain_getFinalizedHead", None).await?;
        result
            .as_str()
            .map(String::from)
            .ok_or_else(|| SubxtError::InvalidResponse {
                endpoint: endpoint.to_string(),
                message: "chain_getFinalizedHead did not return a string".into(),
            })
    }

    /// Call `chain_getHeader` to get the block header at a given hash.
    ///
    /// Returns the header as a JSON value containing `number` (hex) and
    /// `parentHash`.
    pub async fn chain_get_header(
        &self,
        endpoint: &str,
        block_hash: Option<&str>,
    ) -> Result<serde_json::Value, SubxtError> {
        let params = block_hash.map(|h| serde_json::json!([h]));
        self.call(endpoint, "chain_getHeader", params).await
    }

    /// Call `system_health` to check the node health.
    pub async fn system_health(&self, endpoint: &str) -> Result<NodeHealth, SubxtError> {
        let result = self.call(endpoint, "system_health", None).await?;
        serde_json::from_value(result).map_err(|e| SubxtError::InvalidResponse {
            endpoint: endpoint.to_string(),
            message: format!("failed to parse system_health response: {e}"),
        })
    }

    /// Call `system_version` to get the node implementation version.
    pub async fn system_version(&self, endpoint: &str) -> Result<String, SubxtError> {
        let result = self.call(endpoint, "system_version", None).await?;
        result
            .as_str()
            .map(String::from)
            .ok_or_else(|| SubxtError::InvalidResponse {
                endpoint: endpoint.to_string(),
                message: "system_version did not return a string".into(),
            })
    }

    /// Call `state_call` with the given function and input data.
    ///
    /// Used for runtime API calls such as `TransactionPaymentApi_query_info`.
    pub async fn state_call(
        &self,
        endpoint: &str,
        function: &str,
        data_hex: &str,
        block_hash: Option<&str>,
    ) -> Result<String, SubxtError> {
        let params = match block_hash {
            Some(h) => Some(serde_json::json!([function, data_hex, h])),
            None => Some(serde_json::json!([function, data_hex])),
        };
        let result = self.call(endpoint, "state_call", params).await?;
        result
            .as_str()
            .map(String::from)
            .ok_or_else(|| SubxtError::InvalidResponse {
                endpoint: endpoint.to_string(),
                message: "state_call did not return a string".into(),
            })
    }

    /// Return a reference to the underlying configuration.
    pub fn config(&self) -> &SubxtConfig {
        &self.config
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_rpc_request_serializes_correctly() {
        let req = JsonRpcRequest {
            jsonrpc: "2.0",
            id: 1,
            method: "system_health",
            params: None,
        };
        let json = serde_json::to_string(&req).expect("serialize");
        assert!(json.contains("\"jsonrpc\":\"2.0\""));
        assert!(json.contains("\"method\":\"system_health\""));
        assert!(
            !json.contains("params"),
            "params should be omitted when None"
        );
    }

    #[test]
    fn json_rpc_request_serializes_with_params() {
        let req = JsonRpcRequest {
            jsonrpc: "2.0",
            id: 2,
            method: "state_getStorage",
            params: Some(serde_json::json!(["0xabcd"])),
        };
        let json = serde_json::to_string(&req).expect("serialize");
        assert!(json.contains("\"params\":[\"0xabcd\"]"));
    }

    #[test]
    fn json_rpc_response_deserializes_result() {
        let json = r#"{"jsonrpc":"2.0","id":1,"result":"0xdeadbeef"}"#;
        let resp: JsonRpcResponse = serde_json::from_str(json).expect("deserialize");
        assert_eq!(resp.id, 1);
        assert!(resp.error.is_none());
        assert_eq!(
            resp.result.as_ref().and_then(|v| v.as_str()),
            Some("0xdeadbeef")
        );
    }

    #[test]
    fn json_rpc_response_deserializes_error() {
        let json =
            r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32600,"message":"Invalid request"}}"#;
        let resp: JsonRpcResponse = serde_json::from_str(json).expect("deserialize");
        assert!(resp.result.is_none());
        let err = resp.error.expect("should have error");
        assert_eq!(err.code, -32600);
        assert_eq!(err.message, "Invalid request");
    }

    #[test]
    fn node_health_deserializes() {
        let json = r#"{"peers":25,"isSyncing":false,"shouldHavePeers":true}"#;
        let health: NodeHealth = serde_json::from_str(json).expect("deserialize");
        assert_eq!(health.peers, 25);
        assert!(!health.is_syncing);
        assert!(health.should_have_peers);
    }

    #[test]
    fn rpc_client_creates_with_default_config() {
        let client = RpcClient::new(SubxtConfig::default());
        assert!(client.is_ok());
    }

    #[test]
    fn rpc_client_id_increments() {
        let client = RpcClient::new(SubxtConfig::default()).expect("create");
        let id1 = client.next_id.fetch_add(1, Ordering::Relaxed);
        let id2 = client.next_id.fetch_add(1, Ordering::Relaxed);
        assert_eq!(id1, 1);
        assert_eq!(id2, 2);
    }

    #[test]
    fn node_health_serializes() {
        let health = NodeHealth {
            peers: 10,
            is_syncing: true,
            should_have_peers: true,
        };
        let json = serde_json::to_string(&health).expect("serialize");
        assert!(json.contains("\"peers\":10"));
        assert!(json.contains("\"isSyncing\":true"));
    }

    /// Regression test: `{"result": null}` must deserialise as `result: None`
    /// and must NOT trigger the "neither result nor error" error path.
    /// Substrate returns this for `state_getStorage` when an account has no
    /// on-chain record.  The caller (`state_get_storage`) maps `Value::Null`
    /// to `Ok(None)` which the balance command then treats as zero.
    #[test]
    fn json_rpc_response_deserializes_null_result() {
        let json = r#"{"jsonrpc":"2.0","id":3,"result":null}"#;
        let resp: JsonRpcResponse = serde_json::from_str(json).expect("deserialize");
        assert_eq!(resp.id, 3);
        assert!(resp.error.is_none());
        // `result` is `None` when the JSON value is `null` because the field
        // is typed as `Option<serde_json::Value>`.
        assert!(resp.result.is_none());
    }
}
