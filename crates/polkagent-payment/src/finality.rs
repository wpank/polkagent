//! Transaction finality tracking: outcome types and the watcher trait.

use std::pin::Pin;

use futures::Stream;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// TransactionOutcome
// ---------------------------------------------------------------------------

/// The observed outcome of a submitted transaction as it progresses toward
/// finality.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum TransactionOutcome {
    /// The transaction is in the mempool but not yet included in a block.
    Pending,
    /// The transaction has been included in a block (not yet finalized).
    Included {
        /// The block number where the transaction was included.
        block: u64,
    },
    /// The transaction has reached finality.
    Finalized {
        /// The block number where the transaction was finalized.
        block: u64,
    },
    /// The transaction was dropped from the network.
    Dropped {
        /// The reason the transaction was dropped (e.g. "invalid nonce",
        /// "timeout").
        reason: String,
    },
    /// The transaction status is unknown (e.g. node disconnected).
    Unknown,
}

impl TransactionOutcome {
    /// Returns `true` if this outcome is terminal (no further updates
    /// expected).
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Finalized { .. } | Self::Dropped { .. })
    }

    /// Returns the block number if the transaction has been included or
    /// finalized.
    #[must_use]
    pub fn block_number(&self) -> Option<u64> {
        match self {
            Self::Included { block } | Self::Finalized { block } => Some(*block),
            _ => None,
        }
    }
}

impl std::fmt::Display for TransactionOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pending => write!(f, "pending"),
            Self::Included { block } => write!(f, "included(block={block})"),
            Self::Finalized { block } => write!(f, "finalized(block={block})"),
            Self::Dropped { reason } => write!(f, "dropped({reason})"),
            Self::Unknown => write!(f, "unknown"),
        }
    }
}

// ---------------------------------------------------------------------------
// FinalityWatcher
// ---------------------------------------------------------------------------

/// Watches a submitted transaction hash and yields a stream of
/// [`TransactionOutcome`] updates as the transaction progresses toward
/// finality.
///
/// Implementations may connect to a Substrate node via RPC/WebSocket and
/// subscribe to transaction status events.
#[async_trait::async_trait]
pub trait FinalityWatcher: Send + Sync {
    /// Begin watching the given transaction hash.
    ///
    /// Returns a stream that yields [`TransactionOutcome`] values as the
    /// transaction progresses. The stream should terminate after a terminal
    /// outcome ([`TransactionOutcome::Finalized`] or
    /// [`TransactionOutcome::Dropped`]).
    async fn watch(
        &self,
        tx_hash: &str,
    ) -> Pin<Box<dyn Stream<Item = TransactionOutcome> + Send + '_>>;
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_is_not_terminal() {
        assert!(!TransactionOutcome::Pending.is_terminal());
    }

    #[test]
    fn included_is_not_terminal() {
        let outcome = TransactionOutcome::Included { block: 100 };
        assert!(!outcome.is_terminal());
    }

    #[test]
    fn finalized_is_terminal() {
        let outcome = TransactionOutcome::Finalized { block: 200 };
        assert!(outcome.is_terminal());
    }

    #[test]
    fn dropped_is_terminal() {
        let outcome = TransactionOutcome::Dropped {
            reason: "timeout".into(),
        };
        assert!(outcome.is_terminal());
    }

    #[test]
    fn unknown_is_not_terminal() {
        assert!(!TransactionOutcome::Unknown.is_terminal());
    }

    #[test]
    fn block_number_pending() {
        assert_eq!(TransactionOutcome::Pending.block_number(), None);
    }

    #[test]
    fn block_number_included() {
        let outcome = TransactionOutcome::Included { block: 42 };
        assert_eq!(outcome.block_number(), Some(42));
    }

    #[test]
    fn block_number_finalized() {
        let outcome = TransactionOutcome::Finalized { block: 99 };
        assert_eq!(outcome.block_number(), Some(99));
    }

    #[test]
    fn block_number_dropped() {
        let outcome = TransactionOutcome::Dropped {
            reason: "invalid".into(),
        };
        assert_eq!(outcome.block_number(), None);
    }

    #[test]
    fn block_number_unknown() {
        assert_eq!(TransactionOutcome::Unknown.block_number(), None);
    }

    #[test]
    fn display_pending() {
        assert_eq!(TransactionOutcome::Pending.to_string(), "pending");
    }

    #[test]
    fn display_included() {
        let outcome = TransactionOutcome::Included { block: 10 };
        assert_eq!(outcome.to_string(), "included(block=10)");
    }

    #[test]
    fn display_finalized() {
        let outcome = TransactionOutcome::Finalized { block: 20 };
        assert_eq!(outcome.to_string(), "finalized(block=20)");
    }

    #[test]
    fn display_dropped() {
        let outcome = TransactionOutcome::Dropped {
            reason: "nonce too low".into(),
        };
        assert_eq!(outcome.to_string(), "dropped(nonce too low)");
    }

    #[test]
    fn display_unknown() {
        assert_eq!(TransactionOutcome::Unknown.to_string(), "unknown");
    }

    #[test]
    fn serde_round_trip_pending() {
        let outcome = TransactionOutcome::Pending;
        let json = serde_json::to_string(&outcome).expect("serialize");
        let back: TransactionOutcome = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(outcome, back);
    }

    #[test]
    fn serde_round_trip_included() {
        let outcome = TransactionOutcome::Included { block: 123 };
        let json = serde_json::to_string(&outcome).expect("serialize");
        let back: TransactionOutcome = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(outcome, back);
    }

    #[test]
    fn serde_round_trip_finalized() {
        let outcome = TransactionOutcome::Finalized { block: 456 };
        let json = serde_json::to_string(&outcome).expect("serialize");
        let back: TransactionOutcome = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(outcome, back);
    }

    #[test]
    fn serde_round_trip_dropped() {
        let outcome = TransactionOutcome::Dropped {
            reason: "expired".into(),
        };
        let json = serde_json::to_string(&outcome).expect("serialize");
        let back: TransactionOutcome = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(outcome, back);
    }

    #[test]
    fn serde_round_trip_unknown() {
        let outcome = TransactionOutcome::Unknown;
        let json = serde_json::to_string(&outcome).expect("serialize");
        let back: TransactionOutcome = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(outcome, back);
    }

    #[test]
    fn clone_and_eq() {
        let a = TransactionOutcome::Finalized { block: 100 };
        let b = a.clone();
        assert_eq!(a, b);
    }

    #[test]
    fn not_equal_different_variants() {
        let a = TransactionOutcome::Pending;
        let b = TransactionOutcome::Unknown;
        assert_ne!(a, b);
    }

    #[test]
    fn not_equal_different_blocks() {
        let a = TransactionOutcome::Included { block: 1 };
        let b = TransactionOutcome::Included { block: 2 };
        assert_ne!(a, b);
    }
}
