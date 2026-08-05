//! JAM testnet RPC client.
//!
//! Provides [`JamClient`] which can query blocks from a JAM testnet node or
//! from a [`MockJamBackend`] for offline testing.

use tracing::debug;

use crate::error::JamError;
use crate::mock::MockJamBackend;
use crate::types::JamBlock;

/// Backend configuration for a [`JamClient`].
#[derive(Debug)]
enum Backend {
    /// Use a mock backend (no network calls).
    Mock(MockJamBackend),
    /// Connect to a live JAM testnet RPC endpoint.
    Rpc { endpoint: String },
}

/// A client for querying block data from a JAM testnet node.
///
/// # Maturity
///
/// **Experimental.** This client is not connected to any Polkagent core
/// correctness path. It exists for research and exploratory testing against
/// JAM testnets as they become available.
#[derive(Debug)]
pub struct JamClient {
    backend: Backend,
}

impl JamClient {
    /// Create a client backed by a mock (no network).
    #[must_use]
    pub fn mock(backend: MockJamBackend) -> Self {
        Self {
            backend: Backend::Mock(backend),
        }
    }

    /// Create a client targeting a live JAM testnet RPC endpoint.
    ///
    /// **Note:** Live JAM testnet RPC is not yet available. This constructor
    /// exists to establish the API surface. Calling [`get_block`] or
    /// [`get_latest_block`] on a live client will return
    /// [`JamError::NotConnected`] until a JAM testnet is operational.
    ///
    /// [`get_block`]: JamClient::get_block
    /// [`get_latest_block`]: JamClient::get_latest_block
    #[must_use]
    pub fn rpc(endpoint: impl Into<String>) -> Self {
        Self {
            backend: Backend::Rpc {
                endpoint: endpoint.into(),
            },
        }
    }

    /// Query a block by slot number.
    #[allow(clippy::unused_async)] // will need async when live RPC is implemented
    pub async fn get_block(&self, slot: u64) -> Result<JamBlock, JamError> {
        match &self.backend {
            Backend::Mock(mock) => {
                debug!(slot, "mock: fetching JAM block");
                mock.get_block(slot)
            }
            Backend::Rpc { endpoint } => {
                debug!(slot, endpoint = %endpoint, "JAM testnet RPC not yet available");
                Err(JamError::NotConnected)
            }
        }
    }

    /// Get the latest block.
    #[allow(clippy::unused_async)] // will need async when live RPC is implemented
    pub async fn get_latest_block(&self) -> Result<JamBlock, JamError> {
        match &self.backend {
            Backend::Mock(mock) => {
                debug!("mock: fetching latest JAM block");
                mock.get_latest_block()
            }
            Backend::Rpc { endpoint } => {
                debug!(endpoint = %endpoint, "JAM testnet RPC not yet available");
                Err(JamError::NotConnected)
            }
        }
    }

    /// Health check: verify the client can reach its backend.
    #[allow(clippy::unused_async)] // will need async when live RPC is implemented
    pub async fn health(&self) -> Result<(), JamError> {
        match &self.backend {
            Backend::Mock(_) => Ok(()),
            Backend::Rpc { .. } => Err(JamError::NotConnected),
        }
    }
}

#[cfg(test)]
// Client adapter tests intentionally panic at the exact backend contract
// boundary that failed so malformed fixtures remain easy to diagnose.
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "JAM client test assertions intentionally panic with focused diagnostics"
)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mock_client_fetches_block() {
        let backend = MockJamBackend::with_blocks(3);
        let client = JamClient::mock(backend);

        let block = client.get_block(2).await.expect("block 2");
        assert_eq!(block.header.slot, 2);
    }

    #[tokio::test]
    async fn mock_client_fetches_latest() {
        let backend = MockJamBackend::with_blocks(5);
        let client = JamClient::mock(backend);

        let latest = client.get_latest_block().await.expect("latest");
        assert_eq!(latest.header.slot, 5);
    }

    #[tokio::test]
    async fn mock_client_returns_not_found() {
        let backend = MockJamBackend::with_blocks(2);
        let client = JamClient::mock(backend);

        let err = client.get_block(99).await.unwrap_err();
        assert!(matches!(err, JamError::BlockNotFound { slot: 99 }));
    }

    #[tokio::test]
    async fn mock_client_health_ok() {
        let client = JamClient::mock(MockJamBackend::new());
        assert!(client.health().await.is_ok());
    }

    #[tokio::test]
    async fn rpc_client_returns_not_connected() {
        let client = JamClient::rpc("http://localhost:9955");

        let err = client.get_block(0).await.unwrap_err();
        assert!(matches!(err, JamError::NotConnected));

        let err = client.get_latest_block().await.unwrap_err();
        assert!(matches!(err, JamError::NotConnected));

        let err = client.health().await.unwrap_err();
        assert!(matches!(err, JamError::NotConnected));
    }

    #[tokio::test]
    async fn mock_block_parsing_verifies_structure() {
        let backend = MockJamBackend::with_blocks(3);
        let client = JamClient::mock(backend);

        let block = client.get_block(1).await.expect("block 1");
        assert!(!block.hash.is_empty());
        assert!(!block.header.parent_hash.is_empty());
        assert!(!block.header.state_root.is_empty());
        assert!(!block.header.extrinsic_root.is_empty());

        // Verify JSON round-trip of fetched block.
        let json = serde_json::to_string(&block).expect("serialize");
        let parsed: JamBlock = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(block, parsed);
    }
}
