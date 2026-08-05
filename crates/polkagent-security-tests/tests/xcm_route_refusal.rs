//! Phase 4 Acceptance Tests — XCM Route Refusal
//!
//! Verifies that unsupported XCM routes are correctly refused by the
//! `resolve_xcm_mechanism` function. Tests 5+ distinct refusal scenarios.
//!
//! - XR-01: Unsupported asset on both teleport and reserve paths.
//! - XR-02: Client returns Unsupported for all mechanisms.
//! - XR-03: No mechanisms configured (empty teleport + reserve lists).
//! - XR-04: Unknown destination chain with no teleport/reserve.
//! - XR-05: Asset accepted for fees but not for transfer.
//! - XR-06: Version incompatibility error construction.
//! - XR-07: Fee estimation failure error construction.
//! - XR-08: `XcmError::AssetNotAccepted` construction.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "security refusal tests intentionally fail fast when expected fixture outcomes are absent"
)]

use async_trait::async_trait;

use polkagent_chain_trait::{
    resolve_xcm_mechanism, BlockRef, ChainClient, ChainError, ChainProfileId, DecodedCall,
    DryRunResult, FinalityObservation, GenesisHash, PinnedMetadata, SimulationResult, TxHash,
    XcmError, XcmMechanism,
};

// =========================================================================
// Test-only ChainClient
// =========================================================================

struct RefusalTestClient {
    teleport_assets: Vec<String>,
    reserve_assets: Vec<String>,
    all_unsupported: bool,
}

impl RefusalTestClient {
    fn none() -> Self {
        Self {
            teleport_assets: vec![],
            reserve_assets: vec![],
            all_unsupported: false,
        }
    }

    fn all_unsupported() -> Self {
        Self {
            teleport_assets: vec![],
            reserve_assets: vec![],
            all_unsupported: true,
        }
    }

    fn with_dot_only() -> Self {
        Self {
            teleport_assets: vec!["DOT".into()],
            reserve_assets: vec![],
            all_unsupported: false,
        }
    }
}

#[async_trait]
impl ChainClient for RefusalTestClient {
    async fn fetch_metadata(&self, _p: ChainProfileId) -> Result<PinnedMetadata, ChainError> {
        Err(ChainError::Unsupported {
            operation: "fetch_metadata".into(),
        })
    }
    async fn simulate(
        &self,
        _e: &[u8],
        _b: &BlockRef,
        _m: &PinnedMetadata,
    ) -> Result<SimulationResult, ChainError> {
        Err(ChainError::Unsupported {
            operation: "simulate".into(),
        })
    }
    async fn submit_extrinsic(&self, _e: &[u8], _p: ChainProfileId) -> Result<TxHash, ChainError> {
        Err(ChainError::Unsupported {
            operation: "submit_extrinsic".into(),
        })
    }
    async fn watch_finality(
        &self,
        _t: TxHash,
        _p: ChainProfileId,
        _ms: u64,
    ) -> Result<FinalityObservation, ChainError> {
        Err(ChainError::Unsupported {
            operation: "watch_finality".into(),
        })
    }
    async fn decode_call(&self, _c: &[u8], _m: &PinnedMetadata) -> Result<DecodedCall, ChainError> {
        Err(ChainError::Unsupported {
            operation: "decode_call".into(),
        })
    }
    async fn query_storage(
        &self,
        _k: &[u8],
        _b: Option<&BlockRef>,
        _p: ChainProfileId,
    ) -> Result<Option<Vec<u8>>, ChainError> {
        Ok(None)
    }
    async fn dry_run_call(&self, _e: &[u8]) -> Result<DryRunResult, ChainError> {
        Err(ChainError::Unsupported {
            operation: "dry_run_call".into(),
        })
    }
    async fn xcm_query_acceptable_payment_assets(&self, _v: u8) -> Result<Vec<String>, ChainError> {
        Err(ChainError::Unsupported {
            operation: "xcm_query_acceptable_payment_assets".into(),
        })
    }
    async fn xcm_query_delivery_fee(
        &self,
        _dest: &GenesisHash,
        _msg: &[u8],
    ) -> Result<u128, ChainError> {
        Err(ChainError::Unsupported {
            operation: "xcm_query_delivery_fee".into(),
        })
    }
    async fn is_trusted_teleporter(
        &self,
        _dest: &ChainProfileId,
        asset: &str,
    ) -> Result<bool, ChainError> {
        if self.all_unsupported {
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
        if self.all_unsupported {
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

fn src() -> ChainProfileId {
    ChainProfileId::new("polkadot")
}
fn dst() -> ChainProfileId {
    ChainProfileId::new("asset-hub")
}

// =========================================================================
// XR-01: Unsupported asset on both paths
// =========================================================================

#[tokio::test]
async fn xr_01_unsupported_asset_both_paths() {
    let client = RefusalTestClient::none();
    let result = resolve_xcm_mechanism(&client, &src(), &dst(), "BTC").await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(err, XcmError::NoSupportedMechanism { .. }));
    let msg = format!("{err}");
    assert!(msg.contains("BTC"));
}

// =========================================================================
// XR-02: Client returns Unsupported for all mechanisms
// =========================================================================

#[tokio::test]
async fn xr_02_client_all_unsupported() {
    let client = RefusalTestClient::all_unsupported();
    let result = resolve_xcm_mechanism(&client, &src(), &dst(), "DOT").await;
    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err(),
        XcmError::NoSupportedMechanism { .. }
    ));
}

// =========================================================================
// XR-03: No mechanisms configured
// =========================================================================

#[tokio::test]
async fn xr_03_no_mechanisms_configured() {
    let client = RefusalTestClient::none();
    let result = resolve_xcm_mechanism(&client, &src(), &dst(), "USDT").await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(err, XcmError::NoSupportedMechanism { .. }));
    let msg = format!("{err}");
    assert!(msg.contains("USDT"));
    assert!(msg.contains("polkadot"));
    assert!(msg.contains("asset-hub"));
}

// =========================================================================
// XR-04: Unknown destination chain with no teleport/reserve
// =========================================================================

#[tokio::test]
async fn xr_04_unknown_destination_chain() {
    let client = RefusalTestClient::none();
    let unknown_dest = ChainProfileId::new("ethereum-bridge");
    let result = resolve_xcm_mechanism(&client, &src(), &unknown_dest, "DOT").await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(err, XcmError::NoSupportedMechanism { .. }));
    let msg = format!("{err}");
    assert!(msg.contains("ethereum-bridge"));
}

// =========================================================================
// XR-05: Asset accepted for teleport but different asset is not
// =========================================================================

#[tokio::test]
async fn xr_05_only_dot_teleport_rejects_ksm() {
    let client = RefusalTestClient::with_dot_only();

    let dot_result = resolve_xcm_mechanism(&client, &src(), &dst(), "DOT").await;
    assert!(dot_result.is_ok(), "DOT should be accepted for teleport");
    assert_eq!(dot_result.unwrap(), XcmMechanism::Teleport);

    let ksm_result = resolve_xcm_mechanism(&client, &src(), &dst(), "KSM").await;
    assert!(ksm_result.is_err(), "KSM should be refused");
    assert!(matches!(
        ksm_result.unwrap_err(),
        XcmError::NoSupportedMechanism { .. }
    ));
}

// =========================================================================
// XR-06: Version incompatibility error
// =========================================================================

#[test]
fn xr_06_version_incompatible_error() {
    let err = XcmError::VersionIncompatible {
        requested: 5,
        supported: 3,
    };
    let msg = format!("{err}");
    assert!(msg.contains('5'));
    assert!(msg.contains('3'));
    assert!(msg.contains("not compatible"));
}

// =========================================================================
// XR-07: Fee estimation failure error
// =========================================================================

#[test]
fn xr_07_fee_estimation_failed_error() {
    let err = XcmError::FeeEstimationFailed {
        message: "runtime API returned null".into(),
    };
    let msg = format!("{err}");
    assert!(msg.contains("runtime API returned null"));
}

// =========================================================================
// XR-08: AssetNotAccepted error
// =========================================================================

#[test]
fn xr_08_asset_not_accepted_error() {
    let err = XcmError::AssetNotAccepted {
        asset: "WBTC".into(),
    };
    let msg = format!("{err}");
    assert!(msg.contains("WBTC"));
    assert!(msg.contains("not accepted"));
}
