//! Payment action types describing the kind of on-chain operation.

use serde::{Deserialize, Serialize};

/// The kind of on-chain operation a payment intent represents.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaymentAction {
    /// A simple transfer of the chain's native token.
    NativeTransfer,
    /// A transfer of a registered on-chain asset (e.g. USDT on Asset Hub).
    AssetTransfer,
    /// A cross-chain (XCM) transfer.
    CrossChain,
    /// A batch of multiple operations submitted atomically.
    Batch,
    /// A proxied call executed on behalf of another account.
    ProxyCall,
}

impl std::fmt::Display for PaymentAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::NativeTransfer => "native_transfer",
            Self::AssetTransfer => "asset_transfer",
            Self::CrossChain => "cross_chain",
            Self::Batch => "batch",
            Self::ProxyCall => "proxy_call",
        };
        write!(f, "{s}")
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_variants() {
        assert_eq!(PaymentAction::NativeTransfer.to_string(), "native_transfer");
        assert_eq!(PaymentAction::AssetTransfer.to_string(), "asset_transfer");
        assert_eq!(PaymentAction::CrossChain.to_string(), "cross_chain");
        assert_eq!(PaymentAction::Batch.to_string(), "batch");
        assert_eq!(PaymentAction::ProxyCall.to_string(), "proxy_call");
    }

    #[test]
    fn serde_round_trip() {
        for action in [
            PaymentAction::NativeTransfer,
            PaymentAction::AssetTransfer,
            PaymentAction::CrossChain,
            PaymentAction::Batch,
            PaymentAction::ProxyCall,
        ] {
            let json = serde_json::to_string(&action).expect("serialize");
            let back: PaymentAction = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(action, back);
        }
    }

    #[test]
    fn deserialize_from_string() {
        let action: PaymentAction =
            serde_json::from_str("\"native_transfer\"").expect("deserialize");
        assert_eq!(action, PaymentAction::NativeTransfer);
    }

    #[test]
    fn clone_and_eq() {
        let a = PaymentAction::CrossChain;
        let b = a.clone();
        assert_eq!(a, b);
    }

    #[test]
    fn hash_consistency() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(PaymentAction::NativeTransfer);
        set.insert(PaymentAction::NativeTransfer);
        assert_eq!(set.len(), 1);
    }
}
