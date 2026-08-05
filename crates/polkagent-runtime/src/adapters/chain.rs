//! Chain-client adapter composition.

use std::sync::Arc;

use polkagent_chain_trait::{ChainClient, ChainProfile, ChainProfileId, GenesisHash, NetworkType};

use crate::{AdapterPolicy, ComponentReadiness, ReadinessWarning, RuntimeOptions, WarningCode};

/// Chain component ready to pass into `AppServiceBuilder`.
pub(crate) struct ChainBuild {
    pub(crate) client: Option<Arc<dyn ChainClient>>,
    pub(crate) readiness: ComponentReadiness,
    pub(crate) warnings: Vec<ReadinessWarning>,
}

/// Compose a configured Subxt client or an explicitly permitted fake.
pub(crate) fn build(options: &RuntimeOptions) -> ChainBuild {
    let rpc_url = std::env::var("POLKAGENT_CHAIN_RPC_URL")
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| {
            std::env::var("POLKAGENT_RPC_URL")
                .ok()
                .filter(|value| !value.is_empty())
        });

    if let Some(rpc_url) = rpc_url {
        let profile = ChainProfile {
            id: ChainProfileId::new("polkadot"),
            name: "Polkadot".to_owned(),
            genesis_hash: GenesisHash::new(
                "0x91b171bb158e2d3848fa23a9f1c25182fb8e20313b2c1eb49219da7a70ce90c3",
            ),
            spec_version: None,
            rpc_endpoints: vec![rpc_url],
            network_type: NetworkType::Production,
        };
        match polkagent_chain_subxt::SubxtChainClientBuilder::new()
            .add_profile(profile)
            .build()
        {
            Ok(client) => {
                return ChainBuild {
                    client: Some(Arc::new(client)),
                    readiness: ComponentReadiness::ready(
                        "Subxt chain profile configured (connectivity not probed)",
                    ),
                    warnings: Vec::new(),
                };
            }
            Err(error) => {
                return fallback(
                    options.adapter_policy,
                    Some(format!("configured chain profile was invalid: {error}")),
                );
            }
        }
    }

    fallback(options.adapter_policy, None)
}

fn fallback(policy: AdapterPolicy, reason: Option<String>) -> ChainBuild {
    if policy == AdapterPolicy::AllowSimulated {
        let message = reason.map_or_else(
            || {
                "no chain RPC endpoint was configured; using FakeChainClient by explicit policy"
                    .to_owned()
            },
            |reason| format!("{reason}; using FakeChainClient by explicit policy"),
        );
        return ChainBuild {
            client: Some(Arc::new(
                polkagent_chain_fake::FakeChainClientBuilder::polkadot()
                    .with_block_number(22_543_871)
                    .with_runtime_version(1_003_004, 0)
                    .build(),
            )),
            readiness: ComponentReadiness::degraded("simulated FakeChainClient"),
            warnings: vec![ReadinessWarning::new(WarningCode::SimulatedChain, message)],
        };
    }

    let message = reason.unwrap_or_else(|| "no chain RPC endpoint was configured".to_owned());
    ChainBuild {
        client: None,
        readiness: ComponentReadiness::disabled("chain-backed tools are not registered"),
        warnings: vec![ReadinessWarning::new(
            WarningCode::ChainUnavailable,
            message,
        )],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_fallback_disables_chain_instead_of_faking_it() {
        let built = fallback(AdapterPolicy::Strict, None);
        assert!(built.client.is_none());
        assert_eq!(built.readiness.state, crate::ComponentState::Disabled);
        assert_eq!(built.warnings[0].code, WarningCode::ChainUnavailable);
    }

    #[test]
    fn simulated_fallback_is_reported_as_degraded() {
        let built = fallback(AdapterPolicy::AllowSimulated, None);
        assert!(built.client.is_some());
        assert_eq!(built.readiness.state, crate::ComponentState::Degraded);
        assert_eq!(built.warnings[0].code, WarningCode::SimulatedChain);
    }
}
