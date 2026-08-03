//! XCM (Cross-Consensus Messaging) domain types and route resolution.
//!
//! This module provides the types and resolution logic for planning cross-chain
//! transfers within the Polkadot ecosystem. It determines whether a transfer
//! should use teleportation or reserve-backed transfers, estimates fees across
//! hops, and produces an [`XcmPlan`] that downstream components use to build
//! and submit XCM extrinsics.
//!
//! # Route resolution
//!
//! [`resolve_xcm_mechanism`] queries the [`ChainClient`] to determine the
//! correct transfer mechanism (teleport vs. reserve transfer) for a given
//! source, destination, and asset triple.
//!
//! [`estimate_xcm_fees`] queries the [`ChainClient`] for delivery fees and
//! acceptable payment assets, combining them into a [`FeeEstimate`].

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{ChainClient, ChainProfileId, GenesisHash};

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors specific to XCM route resolution and fee estimation.
#[derive(Debug, Error)]
pub enum XcmError {
    /// No supported transfer mechanism for the given route.
    #[error("no supported XCM mechanism for asset '{asset}' from '{origin}' to '{dest}'")]
    NoSupportedMechanism {
        origin: String,
        dest: String,
        asset: String,
    },

    /// Fee estimation failed.
    #[error("XCM fee estimation failed: {message}")]
    FeeEstimationFailed { message: String },

    /// The underlying chain client returned an error.
    #[error("chain client error during XCM resolution: {0}")]
    ChainClient(#[from] crate::ChainError),

    /// The asset is not accepted for fee payment.
    #[error("asset '{asset}' not accepted for XCM fee payment")]
    AssetNotAccepted { asset: String },

    /// Version compatibility issue.
    #[error("XCM version {requested} not compatible; supported: {supported}")]
    VersionIncompatible { requested: u8, supported: u8 },
}

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

/// The mechanism used for an XCM cross-chain transfer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum XcmMechanism {
    /// Assets are teleported (burned on source, minted on dest).
    ///
    /// Requires mutual trust between the two chains.
    Teleport,
    /// Assets are locked on source and a derivative is minted on dest.
    ///
    /// The source chain acts as the reserve.
    ReserveTransfer,
}

/// A single hop in an XCM route.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct XcmHop {
    /// Chain profile for this hop.
    pub chain: ChainProfileId,
    /// The mechanism used at this hop.
    pub mechanism: XcmMechanism,
}

/// A resolved route for an XCM transfer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct XcmRoute {
    /// The source chain.
    pub source: ChainProfileId,
    /// The destination chain.
    pub destination: ChainProfileId,
    /// Intermediate hops (empty for direct transfers).
    pub hops: Vec<XcmHop>,
    /// The transfer mechanisms available for this route.
    pub mechanisms: Vec<XcmMechanism>,
}

/// Breakdown of fees for an XCM transfer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeeEstimate {
    /// Fee charged on the source chain for originating the message.
    pub source_fee: u128,
    /// Weight/execution fee on the destination chain.
    pub dest_weight_fee: u128,
    /// Delivery fee for transporting the message.
    pub delivery_fee: u128,
    /// Total estimated fee (source_fee + dest_weight_fee + delivery_fee).
    pub total: u128,
    /// Whether this estimate is heuristic (true) or based on runtime queries (false).
    pub is_heuristic: bool,
}

/// A complete plan for executing an XCM transfer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XcmPlan {
    /// The resolved route.
    pub route: XcmRoute,
    /// Estimated fees for the transfer.
    pub fee_estimate: FeeEstimate,
    /// Identified risks or warnings for this transfer.
    pub risks: Vec<String>,
    /// XCM version compatibility status.
    pub version_compat: XcmVersionCompat,
}

/// XCM version compatibility information.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct XcmVersionCompat {
    /// The XCM version that will be used.
    pub version: u8,
    /// Whether both chains support this version.
    pub compatible: bool,
}

// ---------------------------------------------------------------------------
// Resolution functions
// ---------------------------------------------------------------------------

/// Determine the XCM transfer mechanism for a given source, destination,
/// and asset.
///
/// Queries the chain client to check teleport and reserve transfer support.
/// Returns the first supported mechanism, preferring teleportation when both
/// are available.
pub async fn resolve_xcm_mechanism(
    client: &dyn ChainClient,
    source: &ChainProfileId,
    dest: &ChainProfileId,
    asset: &str,
) -> Result<XcmMechanism, XcmError> {
    // Check teleport first (preferred when available).
    match client.is_trusted_teleporter(dest, asset).await {
        Ok(true) => return Ok(XcmMechanism::Teleport),
        Ok(false) => {}
        Err(crate::ChainError::Unsupported { .. }) => {}
        Err(e) => return Err(XcmError::ChainClient(e)),
    }

    // Fall back to reserve transfer.
    match client.is_reserve_transfer_supported(dest, asset).await {
        Ok(true) => return Ok(XcmMechanism::ReserveTransfer),
        Ok(false) => {}
        Err(crate::ChainError::Unsupported { .. }) => {}
        Err(e) => return Err(XcmError::ChainClient(e)),
    }

    Err(XcmError::NoSupportedMechanism {
        origin: source.0.clone(),
        dest: dest.0.clone(),
        asset: asset.to_string(),
    })
}

/// Estimate the fees for an XCM transfer.
///
/// Queries the chain client for delivery fees and combines them with a
/// heuristic source fee estimate. When runtime queries are unavailable,
/// falls back to fully heuristic estimates.
pub async fn estimate_xcm_fees(
    client: &dyn ChainClient,
    dest_genesis: &GenesisHash,
    message: &[u8],
    source_fee_estimate: u128,
) -> Result<FeeEstimate, XcmError> {
    // Query delivery fee from the runtime.
    let (delivery_fee, is_heuristic) = match client
        .xcm_query_delivery_fee(dest_genesis, message)
        .await
    {
        Ok(fee) => (fee, false),
        Err(crate::ChainError::Unsupported { .. }) => {
            // Fall back to a heuristic estimate.
            (source_fee_estimate / 10, true)
        }
        Err(e) => return Err(XcmError::ChainClient(e)),
    };

    // Use dry-run for destination weight fee if available.
    let dest_weight_fee = if !message.is_empty() {
        match client.dry_run_call(message).await {
            Ok(result) => result.dest_weight_fee.unwrap_or(0),
            Err(crate::ChainError::Unsupported { .. }) => 0,
            Err(_) => 0,
        }
    } else {
        0
    };

    let total = source_fee_estimate + dest_weight_fee + delivery_fee;

    Ok(FeeEstimate {
        source_fee: source_fee_estimate,
        dest_weight_fee,
        delivery_fee,
        total,
        is_heuristic,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        BlockRef, ChainError, DecodedCall, DryRunResult, FinalityObservation, PinnedMetadata,
        SimulationResult, TxHash,
    };
    use async_trait::async_trait;

    // -----------------------------------------------------------------------
    // Test chain client
    // -----------------------------------------------------------------------

    /// A minimal mock chain client for XCM tests.
    struct XcmTestClient {
        teleport_assets: Vec<String>,
        reserve_assets: Vec<String>,
        delivery_fee: u128,
        acceptable_assets: Vec<String>,
        unsupported: bool,
    }

    impl XcmTestClient {
        fn new() -> Self {
            Self {
                teleport_assets: vec!["DOT".into()],
                reserve_assets: vec!["USDT".into()],
                delivery_fee: 50_000,
                acceptable_assets: vec!["DOT".into(), "USDT".into()],
                unsupported: false,
            }
        }

        fn unsupported() -> Self {
            Self {
                teleport_assets: vec![],
                reserve_assets: vec![],
                delivery_fee: 0,
                acceptable_assets: vec![],
                unsupported: true,
            }
        }

        fn with_no_mechanisms() -> Self {
            Self {
                teleport_assets: vec![],
                reserve_assets: vec![],
                delivery_fee: 50_000,
                acceptable_assets: vec!["DOT".into()],
                unsupported: false,
            }
        }
    }

    #[async_trait]
    impl ChainClient for XcmTestClient {
        async fn fetch_metadata(
            &self,
            _p: ChainProfileId,
        ) -> Result<PinnedMetadata, ChainError> {
            Err(ChainError::Unsupported { operation: "fetch_metadata".into() })
        }
        async fn simulate(
            &self,
            _e: &[u8],
            _b: &BlockRef,
            _m: &PinnedMetadata,
        ) -> Result<SimulationResult, ChainError> {
            Err(ChainError::Unsupported { operation: "simulate".into() })
        }
        async fn submit_extrinsic(
            &self,
            _e: &[u8],
            _p: ChainProfileId,
        ) -> Result<TxHash, ChainError> {
            Err(ChainError::Unsupported { operation: "submit_extrinsic".into() })
        }
        async fn watch_finality(
            &self,
            _t: TxHash,
            _p: ChainProfileId,
            _ms: u64,
        ) -> Result<FinalityObservation, ChainError> {
            Err(ChainError::Unsupported { operation: "watch_finality".into() })
        }
        async fn decode_call(
            &self,
            _c: &[u8],
            _m: &PinnedMetadata,
        ) -> Result<DecodedCall, ChainError> {
            Err(ChainError::Unsupported { operation: "decode_call".into() })
        }
        async fn query_storage(
            &self,
            _k: &[u8],
            _b: Option<&BlockRef>,
            _p: ChainProfileId,
        ) -> Result<Option<Vec<u8>>, ChainError> {
            Ok(None)
        }
        async fn dry_run_call(
            &self,
            _e: &[u8],
        ) -> Result<DryRunResult, ChainError> {
            if self.unsupported {
                return Err(ChainError::Unsupported { operation: "dry_run_call".into() });
            }
            Ok(DryRunResult {
                execution_ok: true,
                events: vec![],
                dest_weight_fee: Some(100_000),
            })
        }
        async fn xcm_query_acceptable_payment_assets(
            &self,
            _v: u8,
        ) -> Result<Vec<String>, ChainError> {
            if self.unsupported {
                return Err(ChainError::Unsupported {
                    operation: "xcm_query_acceptable_payment_assets".into(),
                });
            }
            Ok(self.acceptable_assets.clone())
        }
        async fn xcm_query_delivery_fee(
            &self,
            _dest: &GenesisHash,
            _msg: &[u8],
        ) -> Result<u128, ChainError> {
            if self.unsupported {
                return Err(ChainError::Unsupported {
                    operation: "xcm_query_delivery_fee".into(),
                });
            }
            Ok(self.delivery_fee)
        }
        async fn is_trusted_teleporter(
            &self,
            _dest: &ChainProfileId,
            asset: &str,
        ) -> Result<bool, ChainError> {
            if self.unsupported {
                return Err(ChainError::Unsupported {
                    operation: "is_trusted_teleporter".into(),
                });
            }
            Ok(self.teleport_assets.contains(&asset.to_string()))
        }
        async fn is_reserve_transfer_supported(
            &self,
            _dest: &ChainProfileId,
            asset: &str,
        ) -> Result<bool, ChainError> {
            if self.unsupported {
                return Err(ChainError::Unsupported {
                    operation: "is_reserve_transfer_supported".into(),
                });
            }
            Ok(self.reserve_assets.contains(&asset.to_string()))
        }
        async fn health(&self) -> Result<(), ChainError> {
            Ok(())
        }
    }

    // -----------------------------------------------------------------------
    // Helper
    // -----------------------------------------------------------------------

    fn src() -> ChainProfileId {
        ChainProfileId::new("polkadot")
    }
    fn dst() -> ChainProfileId {
        ChainProfileId::new("asset-hub")
    }
    fn dest_genesis() -> GenesisHash {
        GenesisHash::new("0xdead")
    }

    // -----------------------------------------------------------------------
    // 1. XcmMechanism serialization
    // -----------------------------------------------------------------------

    #[test]
    fn xcm_mechanism_teleport_serializes() {
        let json = serde_json::to_string(&XcmMechanism::Teleport).expect("serialize");
        assert_eq!(json, r#""teleport""#);
    }

    // -----------------------------------------------------------------------
    // 2. XcmMechanism reserve serialization
    // -----------------------------------------------------------------------

    #[test]
    fn xcm_mechanism_reserve_transfer_serializes() {
        let json = serde_json::to_string(&XcmMechanism::ReserveTransfer).expect("serialize");
        assert_eq!(json, r#""reserve_transfer""#);
    }

    // -----------------------------------------------------------------------
    // 3. XcmRoute serialization
    // -----------------------------------------------------------------------

    #[test]
    fn xcm_route_serializes() {
        let route = XcmRoute {
            source: src(),
            destination: dst(),
            hops: vec![],
            mechanisms: vec![XcmMechanism::Teleport],
        };
        let json = serde_json::to_string(&route).expect("serialize");
        assert!(json.contains("polkadot"));
        assert!(json.contains("asset-hub"));
        assert!(json.contains("teleport"));
    }

    // -----------------------------------------------------------------------
    // 4. FeeEstimate total computation
    // -----------------------------------------------------------------------

    #[test]
    fn fee_estimate_total_is_sum() {
        let est = FeeEstimate {
            source_fee: 100,
            dest_weight_fee: 200,
            delivery_fee: 50,
            total: 350,
            is_heuristic: false,
        };
        assert_eq!(est.total, est.source_fee + est.dest_weight_fee + est.delivery_fee);
    }

    // -----------------------------------------------------------------------
    // 5. FeeEstimate serialization
    // -----------------------------------------------------------------------

    #[test]
    fn fee_estimate_serializes() {
        let est = FeeEstimate {
            source_fee: 1000,
            dest_weight_fee: 2000,
            delivery_fee: 500,
            total: 3500,
            is_heuristic: true,
        };
        let json = serde_json::to_string(&est).expect("serialize");
        assert!(json.contains("is_heuristic"));
        assert!(json.contains("3500"));
    }

    // -----------------------------------------------------------------------
    // 6. XcmPlan serialization
    // -----------------------------------------------------------------------

    #[test]
    fn xcm_plan_serializes() {
        let plan = XcmPlan {
            route: XcmRoute {
                source: src(),
                destination: dst(),
                hops: vec![],
                mechanisms: vec![XcmMechanism::Teleport],
            },
            fee_estimate: FeeEstimate {
                source_fee: 100,
                dest_weight_fee: 200,
                delivery_fee: 50,
                total: 350,
                is_heuristic: false,
            },
            risks: vec!["test risk".into()],
            version_compat: XcmVersionCompat {
                version: 4,
                compatible: true,
            },
        };
        let json = serde_json::to_string(&plan).expect("serialize");
        assert!(json.contains("test risk"));
        assert!(json.contains("version"));
    }

    // -----------------------------------------------------------------------
    // 7. XcmVersionCompat serialization
    // -----------------------------------------------------------------------

    #[test]
    fn version_compat_serializes() {
        let vc = XcmVersionCompat { version: 3, compatible: false };
        let json = serde_json::to_string(&vc).expect("serialize");
        assert!(json.contains("\"version\":3"));
        assert!(json.contains("\"compatible\":false"));
    }

    // -----------------------------------------------------------------------
    // 8. resolve_xcm_mechanism returns Teleport for DOT
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn resolve_returns_teleport_for_dot() {
        let client = XcmTestClient::new();
        let result = resolve_xcm_mechanism(&client, &src(), &dst(), "DOT").await;
        assert_eq!(result.unwrap(), XcmMechanism::Teleport);
    }

    // -----------------------------------------------------------------------
    // 9. resolve_xcm_mechanism returns ReserveTransfer for USDT
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn resolve_returns_reserve_for_usdt() {
        let client = XcmTestClient::new();
        let result = resolve_xcm_mechanism(&client, &src(), &dst(), "USDT").await;
        assert_eq!(result.unwrap(), XcmMechanism::ReserveTransfer);
    }

    // -----------------------------------------------------------------------
    // 10. resolve_xcm_mechanism returns error for unsupported asset
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn resolve_returns_error_for_unsupported_asset() {
        let client = XcmTestClient::new();
        let result = resolve_xcm_mechanism(&client, &src(), &dst(), "BTC").await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), XcmError::NoSupportedMechanism { .. }));
    }

    // -----------------------------------------------------------------------
    // 11. resolve_xcm_mechanism returns error when client unsupported
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn resolve_returns_error_when_client_unsupported() {
        let client = XcmTestClient::unsupported();
        let result = resolve_xcm_mechanism(&client, &src(), &dst(), "DOT").await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), XcmError::NoSupportedMechanism { .. }));
    }

    // -----------------------------------------------------------------------
    // 12. resolve_xcm_mechanism returns error when no mechanisms
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn resolve_returns_error_when_no_mechanisms() {
        let client = XcmTestClient::with_no_mechanisms();
        let result = resolve_xcm_mechanism(&client, &src(), &dst(), "DOT").await;
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------------
    // 13. estimate_xcm_fees with runtime queries
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn estimate_fees_with_runtime_queries() {
        let client = XcmTestClient::new();
        let est = estimate_xcm_fees(&client, &dest_genesis(), &[1, 2, 3], 1_000_000)
            .await
            .expect("ok");
        assert_eq!(est.source_fee, 1_000_000);
        assert_eq!(est.delivery_fee, 50_000);
        assert_eq!(est.dest_weight_fee, 100_000);
        assert_eq!(est.total, 1_150_000);
        assert!(!est.is_heuristic);
    }

    // -----------------------------------------------------------------------
    // 14. estimate_xcm_fees falls back to heuristic
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn estimate_fees_falls_back_to_heuristic() {
        let client = XcmTestClient::unsupported();
        let est = estimate_xcm_fees(&client, &dest_genesis(), &[1, 2, 3], 1_000_000)
            .await
            .expect("ok");
        assert!(est.is_heuristic);
        // Heuristic delivery fee = source_fee / 10
        assert_eq!(est.delivery_fee, 100_000);
    }

    // -----------------------------------------------------------------------
    // 15. estimate_xcm_fees with empty message
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn estimate_fees_empty_message() {
        let client = XcmTestClient::new();
        let est = estimate_xcm_fees(&client, &dest_genesis(), &[], 500_000)
            .await
            .expect("ok");
        assert_eq!(est.dest_weight_fee, 0);
        assert_eq!(est.total, 500_000 + 0 + 50_000);
    }

    // -----------------------------------------------------------------------
    // 16. XcmError display for NoSupportedMechanism
    // -----------------------------------------------------------------------

    #[test]
    fn xcm_error_no_supported_mechanism_display() {
        let err = XcmError::NoSupportedMechanism {
            origin: "polkadot".into(),
            dest: "kusama".into(),
            asset: "BTC".into(),
        };
        let msg = format!("{err}");
        assert!(msg.contains("BTC"));
        assert!(msg.contains("polkadot"));
        assert!(msg.contains("kusama"));
    }

    // -----------------------------------------------------------------------
    // 17. XcmError display for FeeEstimationFailed
    // -----------------------------------------------------------------------

    #[test]
    fn xcm_error_fee_estimation_failed_display() {
        let err = XcmError::FeeEstimationFailed {
            message: "timeout".into(),
        };
        let msg = format!("{err}");
        assert!(msg.contains("timeout"));
    }

    // -----------------------------------------------------------------------
    // 18. XcmError display for AssetNotAccepted
    // -----------------------------------------------------------------------

    #[test]
    fn xcm_error_asset_not_accepted_display() {
        let err = XcmError::AssetNotAccepted {
            asset: "WBTC".into(),
        };
        let msg = format!("{err}");
        assert!(msg.contains("WBTC"));
    }

    // -----------------------------------------------------------------------
    // 19. XcmError display for VersionIncompatible
    // -----------------------------------------------------------------------

    #[test]
    fn xcm_error_version_incompatible_display() {
        let err = XcmError::VersionIncompatible {
            requested: 5,
            supported: 3,
        };
        let msg = format!("{err}");
        assert!(msg.contains("5"));
        assert!(msg.contains("3"));
    }

    // -----------------------------------------------------------------------
    // 20. XcmHop serialization
    // -----------------------------------------------------------------------

    #[test]
    fn xcm_hop_serializes() {
        let hop = XcmHop {
            chain: ChainProfileId::new("bridge-hub"),
            mechanism: XcmMechanism::ReserveTransfer,
        };
        let json = serde_json::to_string(&hop).expect("serialize");
        assert!(json.contains("bridge-hub"));
        assert!(json.contains("reserve_transfer"));
    }

    // -----------------------------------------------------------------------
    // 21. XcmMechanism equality
    // -----------------------------------------------------------------------

    #[test]
    fn xcm_mechanism_equality() {
        assert_eq!(XcmMechanism::Teleport, XcmMechanism::Teleport);
        assert_ne!(XcmMechanism::Teleport, XcmMechanism::ReserveTransfer);
    }

    // -----------------------------------------------------------------------
    // 22. XcmRoute with hops
    // -----------------------------------------------------------------------

    #[test]
    fn xcm_route_with_hops() {
        let route = XcmRoute {
            source: ChainProfileId::new("polkadot"),
            destination: ChainProfileId::new("moonbeam"),
            hops: vec![
                XcmHop {
                    chain: ChainProfileId::new("asset-hub"),
                    mechanism: XcmMechanism::Teleport,
                },
            ],
            mechanisms: vec![XcmMechanism::Teleport, XcmMechanism::ReserveTransfer],
        };
        assert_eq!(route.hops.len(), 1);
        assert_eq!(route.mechanisms.len(), 2);
    }

    // -----------------------------------------------------------------------
    // 23. XcmError from ChainError
    // -----------------------------------------------------------------------

    #[test]
    fn xcm_error_from_chain_error() {
        let chain_err = ChainError::Internal {
            message: "test".into(),
        };
        let xcm_err: XcmError = chain_err.into();
        assert!(matches!(xcm_err, XcmError::ChainClient(_)));
    }

    // -----------------------------------------------------------------------
    // 24. resolve_xcm_mechanism prefers teleport over reserve
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn resolve_prefers_teleport_over_reserve() {
        let client = XcmTestClient {
            teleport_assets: vec!["MULTI".into()],
            reserve_assets: vec!["MULTI".into()],
            delivery_fee: 0,
            acceptable_assets: vec![],
            unsupported: false,
        };
        let result = resolve_xcm_mechanism(&client, &src(), &dst(), "MULTI").await;
        assert_eq!(result.unwrap(), XcmMechanism::Teleport);
    }

    // -----------------------------------------------------------------------
    // 25. XcmPlan with risks
    // -----------------------------------------------------------------------

    #[test]
    fn xcm_plan_risks_captured() {
        let plan = XcmPlan {
            route: XcmRoute {
                source: src(),
                destination: dst(),
                hops: vec![],
                mechanisms: vec![XcmMechanism::ReserveTransfer],
            },
            fee_estimate: FeeEstimate {
                source_fee: 0,
                dest_weight_fee: 0,
                delivery_fee: 0,
                total: 0,
                is_heuristic: true,
            },
            risks: vec![
                "heuristic fee estimate".into(),
                "first transfer to destination".into(),
            ],
            version_compat: XcmVersionCompat {
                version: 4,
                compatible: true,
            },
        };
        assert_eq!(plan.risks.len(), 2);
    }

    // -----------------------------------------------------------------------
    // 26. FeeEstimate deserialization
    // -----------------------------------------------------------------------

    #[test]
    fn fee_estimate_deserializes() {
        let json = r#"{"source_fee":100,"dest_weight_fee":200,"delivery_fee":50,"total":350,"is_heuristic":false}"#;
        let est: FeeEstimate = serde_json::from_str(json).expect("deserialize");
        assert_eq!(est.total, 350);
        assert!(!est.is_heuristic);
    }

    // -----------------------------------------------------------------------
    // 27. XcmMechanism deserialization
    // -----------------------------------------------------------------------

    #[test]
    fn xcm_mechanism_deserializes() {
        let teleport: XcmMechanism = serde_json::from_str(r#""teleport""#).expect("ok");
        assert_eq!(teleport, XcmMechanism::Teleport);
        let reserve: XcmMechanism = serde_json::from_str(r#""reserve_transfer""#).expect("ok");
        assert_eq!(reserve, XcmMechanism::ReserveTransfer);
    }
}
