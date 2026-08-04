#![forbid(unsafe_code)]
#![allow(
    clippy::module_name_repetitions,
    clippy::must_use_candidate,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc
)]

use async_trait::async_trait;
use polkagent_chain_trait::{
    ChainClient, ChainError, ChainProfileId, GenesisHash, XcmHop, XcmMechanism, XcmPlan,
    XcmRoute, XcmVersionCompat, estimate_xcm_fees, resolve_xcm_mechanism,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum TransferError {
    #[error("chain error: {0}")]
    Chain(#[from] ChainError),

    #[error("XCM error: {0}")]
    Xcm(#[from] polkagent_chain_trait::XcmError),

    #[error("route unsupported: no valid XCM path from '{origin}' to '{dest}' for asset '{asset}'")]
    RouteUnsupported {
        origin: String,
        dest: String,
        asset: String,
    },

    #[error("asset '{asset}' not accepted for fee payment on source chain")]
    AssetNotAccepted { asset: String },
}

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutePlan {
    pub xcm_plan: XcmPlan,
    pub source: ChainProfileId,
    pub destination: ChainProfileId,
    pub asset: String,
}

// ---------------------------------------------------------------------------
// Route planner trait
// ---------------------------------------------------------------------------

#[async_trait]
pub trait RoutePlanner: Send + Sync {
    async fn plan_route(
        &self,
        source: &ChainProfileId,
        destination: &ChainProfileId,
        asset: &str,
    ) -> Result<RoutePlan, TransferError>;
}

// ---------------------------------------------------------------------------
// Default implementation backed by ChainClient
// ---------------------------------------------------------------------------

pub struct ChainRoutePlanner<C> {
    client: C,
    dest_genesis: GenesisHash,
    xcm_version: u8,
}

impl<C: ChainClient> ChainRoutePlanner<C> {
    pub fn new(client: C, dest_genesis: GenesisHash) -> Self {
        Self {
            client,
            dest_genesis,
            xcm_version: 4,
        }
    }

    pub fn with_xcm_version(mut self, version: u8) -> Self {
        self.xcm_version = version;
        self
    }
}

#[async_trait]
impl<C: ChainClient> RoutePlanner for ChainRoutePlanner<C> {
    async fn plan_route(
        &self,
        source: &ChainProfileId,
        destination: &ChainProfileId,
        asset: &str,
    ) -> Result<RoutePlan, TransferError> {
        // 1. Resolve the transfer mechanism (teleport vs reserve).
        let mechanism = resolve_xcm_mechanism(&self.client, source, destination, asset)
            .await
            .map_err(|e| match e {
                polkagent_chain_trait::XcmError::NoSupportedMechanism {
                    origin,
                    dest,
                    asset,
                } => TransferError::RouteUnsupported {
                    origin,
                    dest,
                    asset,
                },
                other => TransferError::Xcm(other),
            })?;

        // 2. Check that the asset is accepted for fee payment.
        let accepted = self
            .client
            .xcm_query_acceptable_payment_assets(self.xcm_version)
            .await;
        let asset_accepted = match accepted {
            Ok(assets) => assets.iter().any(|a| a == asset),
            Err(ChainError::Unsupported { .. }) => true, // assume accepted if API unavailable
            Err(e) => return Err(TransferError::Chain(e)),
        };
        if !asset_accepted {
            return Err(TransferError::AssetNotAccepted {
                asset: asset.to_string(),
            });
        }

        // 3. Build the route.
        let route = XcmRoute {
            source: source.clone(),
            destination: destination.clone(),
            hops: vec![XcmHop {
                chain: destination.clone(),
                mechanism,
            }],
            mechanisms: vec![mechanism],
        };

        // 4. Estimate fees (use a zero-length message placeholder for planning).
        let fee_estimate =
            estimate_xcm_fees(&self.client, &self.dest_genesis, &[], 0).await?;

        // 5. Identify risks.
        let mut risks = Vec::new();
        if fee_estimate.is_heuristic {
            risks.push("fee estimate is heuristic — runtime queries unavailable".into());
        }
        if mechanism == XcmMechanism::ReserveTransfer {
            risks.push("reserve-backed transfer: assets locked on source chain".into());
        }

        // 6. Check version compatibility (best-effort).
        let version_compat = XcmVersionCompat {
            version: self.xcm_version,
            compatible: true,
        };

        let xcm_plan = XcmPlan {
            route,
            fee_estimate,
            risks,
            version_compat,
        };

        Ok(RoutePlan {
            xcm_plan,
            source: source.clone(),
            destination: destination.clone(),
            asset: asset.to_string(),
        })
    }
}

// ---------------------------------------------------------------------------
// Free function convenience wrapper
// ---------------------------------------------------------------------------

pub async fn plan_route(
    client: &dyn ChainClient,
    source: &ChainProfileId,
    destination: &ChainProfileId,
    asset: &str,
    dest_genesis: &GenesisHash,
) -> Result<RoutePlan, TransferError> {
    // 1. Resolve the transfer mechanism.
    let mechanism = resolve_xcm_mechanism(client, source, destination, asset)
        .await
        .map_err(|e| match e {
            polkagent_chain_trait::XcmError::NoSupportedMechanism {
                origin,
                dest,
                asset,
            } => TransferError::RouteUnsupported {
                origin,
                dest,
                asset,
            },
            other => TransferError::Xcm(other),
        })?;

    // 2. Check that the asset is accepted for fee payment.
    let accepted = client.xcm_query_acceptable_payment_assets(4).await;
    let asset_accepted = match accepted {
        Ok(assets) => assets.iter().any(|a| a == asset),
        Err(ChainError::Unsupported { .. }) => true,
        Err(e) => return Err(TransferError::Chain(e)),
    };
    if !asset_accepted {
        return Err(TransferError::AssetNotAccepted {
            asset: asset.to_string(),
        });
    }

    // 3. Build the route.
    let route = XcmRoute {
        source: source.clone(),
        destination: destination.clone(),
        hops: vec![XcmHop {
            chain: destination.clone(),
            mechanism,
        }],
        mechanisms: vec![mechanism],
    };

    // 4. Estimate fees.
    let fee_estimate = estimate_xcm_fees(client, dest_genesis, &[], 0).await?;

    // 5. Identify risks.
    let mut risks = Vec::new();
    if fee_estimate.is_heuristic {
        risks.push("fee estimate is heuristic — runtime queries unavailable".into());
    }
    if mechanism == XcmMechanism::ReserveTransfer {
        risks.push("reserve-backed transfer: assets locked on source chain".into());
    }

    // 6. Version compatibility.
    let version_compat = XcmVersionCompat {
        version: 4,
        compatible: true,
    };

    let xcm_plan = XcmPlan {
        route,
        fee_estimate,
        risks,
        version_compat,
    };

    Ok(RoutePlan {
        xcm_plan,
        source: source.clone(),
        destination: destination.clone(),
        asset: asset.to_string(),
    })
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use polkagent_chain_fake::FakeChainClientBuilder;

    // -----------------------------------------------------------------------
    // Helpers
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

    fn make_client() -> polkagent_chain_fake::FakeChainClient {
        FakeChainClientBuilder::new("polkadot").build()
    }

    // -----------------------------------------------------------------------
    // 1. plan_route returns teleport route for DOT
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn plan_route_teleport_dot() {
        let client = make_client();
        let plan = plan_route(&client, &src(), &dst(), "DOT", &dest_genesis())
            .await
            .expect("plan ok");

        assert_eq!(plan.source, src());
        assert_eq!(plan.destination, dst());
        assert_eq!(plan.asset, "DOT");
        assert_eq!(plan.xcm_plan.route.mechanisms, vec![XcmMechanism::Teleport]);
    }

    // -----------------------------------------------------------------------
    // 2. plan_route returns reserve route for USDT
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn plan_route_reserve_usdt() {
        let client = make_client();
        let plan = plan_route(&client, &src(), &dst(), "USDT", &dest_genesis())
            .await
            .expect("plan ok");

        assert_eq!(plan.asset, "USDT");
        assert_eq!(
            plan.xcm_plan.route.mechanisms,
            vec![XcmMechanism::ReserveTransfer]
        );
        assert!(plan
            .xcm_plan
            .risks
            .iter()
            .any(|r| r.contains("reserve-backed")));
    }

    // -----------------------------------------------------------------------
    // 3. plan_route returns RouteUnsupported for unknown asset
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn plan_route_unsupported_asset() {
        let client = make_client();
        let err = plan_route(&client, &src(), &dst(), "BTC", &dest_genesis())
            .await
            .unwrap_err();

        assert!(matches!(err, TransferError::RouteUnsupported { .. }));
        let msg = format!("{err}");
        assert!(msg.contains("BTC"));
    }

    // -----------------------------------------------------------------------
    // 4. plan_route returns RouteUnsupported for unknown chains
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn plan_route_unsupported_chain_pair() {
        let client = make_client();
        let unknown_src = ChainProfileId::new("ethereum");
        let unknown_dst = ChainProfileId::new("solana");
        let err = plan_route(&client, &unknown_src, &unknown_dst, "ETH", &dest_genesis())
            .await
            .unwrap_err();

        assert!(matches!(err, TransferError::RouteUnsupported { .. }));
    }

    // -----------------------------------------------------------------------
    // 5. plan_route includes fee estimate
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn plan_route_includes_fee_estimate() {
        let client = make_client();
        let plan = plan_route(&client, &src(), &dst(), "DOT", &dest_genesis())
            .await
            .expect("plan ok");

        let fee = &plan.xcm_plan.fee_estimate;
        assert_eq!(fee.total, fee.source_fee + fee.dest_weight_fee + fee.delivery_fee);
    }

    // -----------------------------------------------------------------------
    // 6. plan_route includes version compatibility
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn plan_route_version_compat() {
        let client = make_client();
        let plan = plan_route(&client, &src(), &dst(), "DOT", &dest_genesis())
            .await
            .expect("plan ok");

        assert_eq!(plan.xcm_plan.version_compat.version, 4);
        assert!(plan.xcm_plan.version_compat.compatible);
    }

    // -----------------------------------------------------------------------
    // 7. plan_route hops contain destination
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn plan_route_hops_contain_destination() {
        let client = make_client();
        let plan = plan_route(&client, &src(), &dst(), "DOT", &dest_genesis())
            .await
            .expect("plan ok");

        assert_eq!(plan.xcm_plan.route.hops.len(), 1);
        assert_eq!(plan.xcm_plan.route.hops[0].chain, dst());
        assert_eq!(
            plan.xcm_plan.route.hops[0].mechanism,
            XcmMechanism::Teleport
        );
    }

    // -----------------------------------------------------------------------
    // 8. RoutePlan serializes
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn route_plan_serializes() {
        let client = make_client();
        let plan = plan_route(&client, &src(), &dst(), "DOT", &dest_genesis())
            .await
            .expect("plan ok");

        let json = serde_json::to_string(&plan).expect("serialize");
        assert!(json.contains("polkadot"));
        assert!(json.contains("asset-hub"));
        assert!(json.contains("DOT"));
    }

    // -----------------------------------------------------------------------
    // 9. RoutePlan deserializes
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn route_plan_roundtrip() {
        let client = make_client();
        let plan = plan_route(&client, &src(), &dst(), "DOT", &dest_genesis())
            .await
            .expect("plan ok");

        let json = serde_json::to_string(&plan).expect("serialize");
        let back: RoutePlan = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.source, plan.source);
        assert_eq!(back.destination, plan.destination);
        assert_eq!(back.asset, plan.asset);
    }

    // -----------------------------------------------------------------------
    // 10. TransferError display for RouteUnsupported
    // -----------------------------------------------------------------------

    #[test]
    fn transfer_error_route_unsupported_display() {
        let err = TransferError::RouteUnsupported {
            origin: "polkadot".into(),
            dest: "kusama".into(),
            asset: "BTC".into(),
        };
        let msg = format!("{err}");
        assert!(msg.contains("route unsupported"));
        assert!(msg.contains("BTC"));
        assert!(msg.contains("polkadot"));
        assert!(msg.contains("kusama"));
    }

    // -----------------------------------------------------------------------
    // 11. TransferError display for AssetNotAccepted
    // -----------------------------------------------------------------------

    #[test]
    fn transfer_error_asset_not_accepted_display() {
        let err = TransferError::AssetNotAccepted {
            asset: "WBTC".into(),
        };
        let msg = format!("{err}");
        assert!(msg.contains("WBTC"));
        assert!(msg.contains("not accepted"));
    }

    // -----------------------------------------------------------------------
    // 12. TransferError from ChainError
    // -----------------------------------------------------------------------

    #[test]
    fn transfer_error_from_chain_error() {
        let chain_err = ChainError::Internal {
            message: "boom".into(),
        };
        let err: TransferError = chain_err.into();
        assert!(matches!(err, TransferError::Chain(_)));
    }

    // -----------------------------------------------------------------------
    // 13. ChainRoutePlanner plan_route teleport
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn chain_route_planner_teleport() {
        let client = make_client();
        let planner = ChainRoutePlanner::new(client, dest_genesis());
        let plan = planner
            .plan_route(&src(), &dst(), "DOT")
            .await
            .expect("plan ok");

        assert_eq!(plan.xcm_plan.route.mechanisms, vec![XcmMechanism::Teleport]);
    }

    // -----------------------------------------------------------------------
    // 14. ChainRoutePlanner plan_route reserve
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn chain_route_planner_reserve() {
        let client = make_client();
        let planner = ChainRoutePlanner::new(client, dest_genesis());
        let plan = planner
            .plan_route(&src(), &dst(), "USDT")
            .await
            .expect("plan ok");

        assert_eq!(
            plan.xcm_plan.route.mechanisms,
            vec![XcmMechanism::ReserveTransfer]
        );
    }

    // -----------------------------------------------------------------------
    // 15. ChainRoutePlanner refuses unknown asset
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn chain_route_planner_refuses_unknown() {
        let client = make_client();
        let planner = ChainRoutePlanner::new(client, dest_genesis());
        let err = planner
            .plan_route(&src(), &dst(), "SHIB")
            .await
            .unwrap_err();

        assert!(matches!(err, TransferError::RouteUnsupported { .. }));
    }

    // -----------------------------------------------------------------------
    // 16. ChainRoutePlanner with custom xcm_version
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn chain_route_planner_custom_version() {
        let client = make_client();
        let planner = ChainRoutePlanner::new(client, dest_genesis()).with_xcm_version(3);
        let plan = planner
            .plan_route(&src(), &dst(), "DOT")
            .await
            .expect("plan ok");

        assert_eq!(plan.xcm_plan.version_compat.version, 3);
    }

    // -----------------------------------------------------------------------
    // 17. plan_route with fault injection returns error
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn plan_route_with_fault_injection() {
        let client = FakeChainClientBuilder::new("polkadot")
            .fail_next_n(1)
            .build();
        let err = plan_route(&client, &src(), &dst(), "DOT", &dest_genesis())
            .await
            .unwrap_err();

        assert!(matches!(
            err,
            TransferError::Chain(_) | TransferError::Xcm(_)
        ));
    }

    // -----------------------------------------------------------------------
    // 18. reserve route adds risk about locked assets
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn reserve_route_warns_about_locked_assets() {
        let client = make_client();
        let plan = plan_route(&client, &src(), &dst(), "USDT", &dest_genesis())
            .await
            .expect("plan ok");

        assert!(plan
            .xcm_plan
            .risks
            .iter()
            .any(|r| r.contains("locked on source")));
    }

    // -----------------------------------------------------------------------
    // 19. teleport route has no locked-assets risk
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn teleport_route_no_locked_assets_risk() {
        let client = make_client();
        let plan = plan_route(&client, &src(), &dst(), "DOT", &dest_genesis())
            .await
            .expect("plan ok");

        assert!(!plan
            .xcm_plan
            .risks
            .iter()
            .any(|r| r.contains("locked on source")));
    }

    // -----------------------------------------------------------------------
    // 20. route source and destination match request
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn route_endpoints_match_request() {
        let client = make_client();
        let plan = plan_route(&client, &src(), &dst(), "DOT", &dest_genesis())
            .await
            .expect("plan ok");

        assert_eq!(plan.xcm_plan.route.source, src());
        assert_eq!(plan.xcm_plan.route.destination, dst());
    }
}
