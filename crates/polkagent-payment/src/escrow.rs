//! **EXPERIMENTAL** — Agent-to-agent escrow state machine for PRD-08 §5.3.
//!
//! This module provides a prototype escrow mechanism where Agent A deposits
//! funds into a Polkadot pure-proxy account, Agent B performs work, and the
//! escrow releases on verified completion via 2-of-2 multisig co-signing.
//!
//! # On-chain topology
//!
//! ```text
//! ┌─────────────────────────────────────────────────────┐
//! │               Pure Proxy (escrow account)           │
//! │                  (no private key)                   │
//! │                                                     │
//! │  Proxy relationships:                               │
//! │  ├─ Multisig(buyer, seller) [2-of-2, Balances]      │
//! │  └─ Buyer [time-delay = T blocks, Balances]         │
//! └─────────────────────────────────────────────────────┘
//! ```
//!
//! # Status
//!
//! **Research prototype — not production-ready.**
//!
//! See `docs/adr/ADR-001-Agent-to-Agent-Escrow.md` for the full design.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::PaymentError;
use crate::types::Amount;

// ---------------------------------------------------------------------------
// EscrowStatus
// ---------------------------------------------------------------------------

/// Lifecycle states for an agent-to-agent escrow.
///
/// **EXPERIMENTAL** — part of the PRD-08 §5.3 escrow prototype.
///
/// ```text
/// Proposed → Funded → Active → AwaitingRelease → Released
///                   ↘ Expired                  ↘ Disputed → Resolved
///          ↘ Cancelled                                    ↘ Refunded
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EscrowStatus {
    /// Terms agreed; awaiting buyer funding.
    Proposed,
    /// Buyer has transferred funds to the escrow pure-proxy account.
    Funded,
    /// Seller is performing work; funds are locked.
    Active,
    /// Seller has signaled completion; awaiting co-signed release.
    AwaitingRelease,
    /// Funds released to seller via multisig (terminal).
    Released,
    /// Timeout reached without release; funds returned to buyer (terminal).
    Expired,
    /// A dispute has been raised; escrow frozen.
    Disputed,
    /// Dispute resolved; funds sent to determined party (terminal).
    Resolved,
    /// Funds returned to buyer (terminal).
    Refunded,
    /// Escrow cancelled before funding completed (terminal).
    Cancelled,
}

impl EscrowStatus {
    /// Returns `true` if this status is terminal (no further transitions).
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Released | Self::Expired | Self::Resolved | Self::Refunded | Self::Cancelled
        )
    }
}

impl std::fmt::Display for EscrowStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Proposed => "proposed",
            Self::Funded => "funded",
            Self::Active => "active",
            Self::AwaitingRelease => "awaiting_release",
            Self::Released => "released",
            Self::Expired => "expired",
            Self::Disputed => "disputed",
            Self::Resolved => "resolved",
            Self::Refunded => "refunded",
            Self::Cancelled => "cancelled",
        };
        write!(f, "{s}")
    }
}

// ---------------------------------------------------------------------------
// Transition validation
// ---------------------------------------------------------------------------

/// Validate whether a transition from `from` to `to` is permitted.
///
/// Returns `Ok(())` if the transition is valid, or a [`PaymentError`] if not.
pub fn validate_escrow_transition(
    from: EscrowStatus,
    to: EscrowStatus,
) -> Result<(), PaymentError> {
    let valid = match from {
        EscrowStatus::Proposed => to == EscrowStatus::Funded || to == EscrowStatus::Cancelled,
        EscrowStatus::Funded => to == EscrowStatus::Active || to == EscrowStatus::Cancelled,
        EscrowStatus::Active => {
            to == EscrowStatus::AwaitingRelease
                || to == EscrowStatus::Expired
                || to == EscrowStatus::Disputed
        }
        EscrowStatus::AwaitingRelease => {
            to == EscrowStatus::Released || to == EscrowStatus::Disputed
        }
        EscrowStatus::Disputed => to == EscrowStatus::Resolved || to == EscrowStatus::Refunded,
        // Terminal states accept no further transitions.
        EscrowStatus::Released
        | EscrowStatus::Expired
        | EscrowStatus::Resolved
        | EscrowStatus::Refunded
        | EscrowStatus::Cancelled => false,
    };

    if valid {
        Ok(())
    } else {
        Err(PaymentError::InvalidStatusTransition {
            from: from.to_string(),
            to: to.to_string(),
        })
    }
}

// ---------------------------------------------------------------------------
// EscrowAgreement
// ---------------------------------------------------------------------------

/// The terms of an agent-to-agent escrow.
///
/// **EXPERIMENTAL** — part of the PRD-08 §5.3 escrow prototype.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EscrowAgreement {
    /// Unique identifier for this escrow.
    pub id: Uuid,
    /// The agent depositing funds (buyer).
    pub buyer_agent_id: String,
    /// The agent performing work (seller).
    pub seller_agent_id: String,
    /// The escrowed amount.
    pub amount: Amount,
    /// The on-chain address of the pure-proxy escrow account (set after
    /// proxy creation).
    pub escrow_account: Option<String>,
    /// Timeout in blocks after which the buyer can reclaim funds.
    pub timeout_blocks: u64,
    /// Human-readable description of the work to be performed.
    pub description: String,
    /// When this agreement was created.
    pub created_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// EscrowStateMachine
// ---------------------------------------------------------------------------

/// A state machine tracking the lifecycle of an agent-to-agent escrow.
///
/// **EXPERIMENTAL** — This is a research prototype for PRD-08 §5.3.
/// It mirrors the [`IntentStateMachine`](crate::intent::IntentStateMachine)
/// pattern with escrow-specific states.
///
/// # Lifecycle
///
/// 1. Create with [`EscrowStateMachine::new`] (starts in `Proposed`)
/// 2. Transition through states with [`transition`](Self::transition)
/// 3. Machine rejects invalid transitions and records full history
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EscrowStateMachine {
    agreement: EscrowAgreement,
    current_status: EscrowStatus,
    history: Vec<(EscrowStatus, DateTime<Utc>)>,
}

impl EscrowStateMachine {
    /// Create a new escrow state machine from an agreement.
    ///
    /// Starts in [`EscrowStatus::Proposed`].
    #[must_use]
    pub fn new(agreement: EscrowAgreement) -> Self {
        let now = Utc::now();
        Self {
            agreement,
            current_status: EscrowStatus::Proposed,
            history: vec![(EscrowStatus::Proposed, now)],
        }
    }

    /// The escrow agreement (terms, parties, amount).
    #[must_use]
    pub fn agreement(&self) -> &EscrowAgreement {
        &self.agreement
    }

    /// The current lifecycle status.
    #[must_use]
    pub fn current_status(&self) -> EscrowStatus {
        self.current_status
    }

    /// The full transition history as `(status, timestamp)` pairs.
    #[must_use]
    pub fn history(&self) -> &[(EscrowStatus, DateTime<Utc>)] {
        &self.history
    }

    /// Attempt to transition to `new_status`.
    ///
    /// Returns `Ok(())` if the transition is valid, recording it in the
    /// history with the current UTC timestamp. Returns an error if the
    /// transition is not allowed.
    pub fn transition(&mut self, new_status: EscrowStatus) -> Result<(), PaymentError> {
        validate_escrow_transition(self.current_status, new_status)?;
        let now = Utc::now();
        self.current_status = new_status;
        self.history.push((new_status, now));
        Ok(())
    }

    /// Set the on-chain escrow account address (pure-proxy address).
    ///
    /// Typically called after the pure-proxy creation extrinsic is confirmed.
    pub fn set_escrow_account(&mut self, account: impl Into<String>) {
        self.agreement.escrow_account = Some(account.into());
    }

    /// Returns `true` if the machine is in a terminal state.
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        self.current_status.is_terminal()
    }

    /// Returns `true` if funds are currently locked (funded, active, or
    /// awaiting release).
    #[must_use]
    pub fn funds_locked(&self) -> bool {
        matches!(
            self.current_status,
            EscrowStatus::Funded | EscrowStatus::Active | EscrowStatus::AwaitingRelease
        )
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::AssetId;

    fn test_agreement() -> EscrowAgreement {
        EscrowAgreement {
            id: Uuid::now_v7(),
            buyer_agent_id: "agent-buyer".into(),
            seller_agent_id: "agent-seller".into(),
            amount: Amount::new(50_000_000_000, AssetId::Native, 10),
            escrow_account: None,
            timeout_blocks: 100,
            description: "analyze governance proposal #42".into(),
            created_at: Utc::now(),
        }
    }

    // --- EscrowStatus ---

    #[test]
    fn display_all_variants() {
        assert_eq!(EscrowStatus::Proposed.to_string(), "proposed");
        assert_eq!(EscrowStatus::Funded.to_string(), "funded");
        assert_eq!(EscrowStatus::Active.to_string(), "active");
        assert_eq!(
            EscrowStatus::AwaitingRelease.to_string(),
            "awaiting_release"
        );
        assert_eq!(EscrowStatus::Released.to_string(), "released");
        assert_eq!(EscrowStatus::Expired.to_string(), "expired");
        assert_eq!(EscrowStatus::Disputed.to_string(), "disputed");
        assert_eq!(EscrowStatus::Resolved.to_string(), "resolved");
        assert_eq!(EscrowStatus::Refunded.to_string(), "refunded");
        assert_eq!(EscrowStatus::Cancelled.to_string(), "cancelled");
    }

    #[test]
    fn terminal_states() {
        assert!(EscrowStatus::Released.is_terminal());
        assert!(EscrowStatus::Expired.is_terminal());
        assert!(EscrowStatus::Resolved.is_terminal());
        assert!(EscrowStatus::Refunded.is_terminal());
        assert!(EscrowStatus::Cancelled.is_terminal());
        assert!(!EscrowStatus::Proposed.is_terminal());
        assert!(!EscrowStatus::Funded.is_terminal());
        assert!(!EscrowStatus::Active.is_terminal());
        assert!(!EscrowStatus::AwaitingRelease.is_terminal());
        assert!(!EscrowStatus::Disputed.is_terminal());
    }

    #[test]
    fn serde_round_trip() {
        for status in [
            EscrowStatus::Proposed,
            EscrowStatus::Funded,
            EscrowStatus::Active,
            EscrowStatus::AwaitingRelease,
            EscrowStatus::Released,
            EscrowStatus::Expired,
            EscrowStatus::Disputed,
            EscrowStatus::Resolved,
            EscrowStatus::Refunded,
            EscrowStatus::Cancelled,
        ] {
            let json = serde_json::to_string(&status).expect("serialize");
            let back: EscrowStatus = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(status, back);
        }
    }

    // --- validate_escrow_transition ---

    #[test]
    fn valid_happy_path() {
        let transitions = [
            (EscrowStatus::Proposed, EscrowStatus::Funded),
            (EscrowStatus::Funded, EscrowStatus::Active),
            (EscrowStatus::Active, EscrowStatus::AwaitingRelease),
            (EscrowStatus::AwaitingRelease, EscrowStatus::Released),
        ];
        for (from, to) in transitions {
            assert!(
                validate_escrow_transition(from, to).is_ok(),
                "transition {from} -> {to} should be valid"
            );
        }
    }

    #[test]
    fn valid_timeout_path() {
        assert!(validate_escrow_transition(EscrowStatus::Active, EscrowStatus::Expired).is_ok());
    }

    #[test]
    fn valid_dispute_path() {
        assert!(validate_escrow_transition(EscrowStatus::Active, EscrowStatus::Disputed).is_ok());
        assert!(
            validate_escrow_transition(EscrowStatus::AwaitingRelease, EscrowStatus::Disputed)
                .is_ok()
        );
        assert!(validate_escrow_transition(EscrowStatus::Disputed, EscrowStatus::Resolved).is_ok());
        assert!(validate_escrow_transition(EscrowStatus::Disputed, EscrowStatus::Refunded).is_ok());
    }

    #[test]
    fn valid_cancellation_path() {
        assert!(
            validate_escrow_transition(EscrowStatus::Proposed, EscrowStatus::Cancelled).is_ok()
        );
        assert!(validate_escrow_transition(EscrowStatus::Funded, EscrowStatus::Cancelled).is_ok());
    }

    #[test]
    fn cannot_transition_from_terminal() {
        let terminals = [
            EscrowStatus::Released,
            EscrowStatus::Expired,
            EscrowStatus::Resolved,
            EscrowStatus::Refunded,
            EscrowStatus::Cancelled,
        ];
        for terminal in terminals {
            for target in [
                EscrowStatus::Proposed,
                EscrowStatus::Funded,
                EscrowStatus::Active,
            ] {
                assert!(
                    validate_escrow_transition(terminal, target).is_err(),
                    "transition from terminal {terminal} -> {target} should be invalid"
                );
            }
        }
    }

    #[test]
    fn invalid_skip_transition() {
        assert!(validate_escrow_transition(EscrowStatus::Proposed, EscrowStatus::Active).is_err());
    }

    #[test]
    fn invalid_backward_transition() {
        assert!(validate_escrow_transition(EscrowStatus::Active, EscrowStatus::Proposed).is_err());
    }

    #[test]
    fn cannot_cancel_during_active_work() {
        assert!(validate_escrow_transition(EscrowStatus::Active, EscrowStatus::Cancelled).is_err());
    }

    // --- EscrowStateMachine ---

    #[test]
    fn new_starts_in_proposed() {
        let sm = EscrowStateMachine::new(test_agreement());
        assert_eq!(sm.current_status(), EscrowStatus::Proposed);
        assert_eq!(sm.history().len(), 1);
        assert_eq!(sm.history()[0].0, EscrowStatus::Proposed);
    }

    #[test]
    fn transition_records_history() {
        let mut sm = EscrowStateMachine::new(test_agreement());
        sm.transition(EscrowStatus::Funded)
            .expect("valid transition");
        assert_eq!(sm.current_status(), EscrowStatus::Funded);
        assert_eq!(sm.history().len(), 2);
        assert_eq!(sm.history()[1].0, EscrowStatus::Funded);
    }

    #[test]
    fn full_happy_path_machine() {
        let mut sm = EscrowStateMachine::new(test_agreement());
        sm.transition(EscrowStatus::Funded).expect("valid");
        sm.transition(EscrowStatus::Active).expect("valid");
        sm.transition(EscrowStatus::AwaitingRelease).expect("valid");
        sm.transition(EscrowStatus::Released).expect("valid");

        assert_eq!(sm.current_status(), EscrowStatus::Released);
        assert!(sm.is_terminal());
        assert_eq!(sm.history().len(), 5);
    }

    #[test]
    fn timeout_path_machine() {
        let mut sm = EscrowStateMachine::new(test_agreement());
        sm.transition(EscrowStatus::Funded).expect("valid");
        sm.transition(EscrowStatus::Active).expect("valid");
        sm.transition(EscrowStatus::Expired).expect("valid");

        assert_eq!(sm.current_status(), EscrowStatus::Expired);
        assert!(sm.is_terminal());
    }

    #[test]
    fn dispute_path_machine() {
        let mut sm = EscrowStateMachine::new(test_agreement());
        sm.transition(EscrowStatus::Funded).expect("valid");
        sm.transition(EscrowStatus::Active).expect("valid");
        sm.transition(EscrowStatus::Disputed).expect("valid");
        sm.transition(EscrowStatus::Refunded).expect("valid");

        assert_eq!(sm.current_status(), EscrowStatus::Refunded);
        assert!(sm.is_terminal());
    }

    #[test]
    fn invalid_transition_rejected() {
        let mut sm = EscrowStateMachine::new(test_agreement());
        let err = sm.transition(EscrowStatus::Active);
        assert!(err.is_err());
        assert_eq!(sm.current_status(), EscrowStatus::Proposed);
        assert_eq!(sm.history().len(), 1);
    }

    #[test]
    fn cannot_transition_after_terminal() {
        let mut sm = EscrowStateMachine::new(test_agreement());
        sm.transition(EscrowStatus::Cancelled).expect("valid");
        let err = sm.transition(EscrowStatus::Funded);
        assert!(err.is_err());
    }

    #[test]
    fn funds_locked_states() {
        let mut sm = EscrowStateMachine::new(test_agreement());
        assert!(!sm.funds_locked());

        sm.transition(EscrowStatus::Funded).expect("valid");
        assert!(sm.funds_locked());

        sm.transition(EscrowStatus::Active).expect("valid");
        assert!(sm.funds_locked());

        sm.transition(EscrowStatus::AwaitingRelease).expect("valid");
        assert!(sm.funds_locked());

        sm.transition(EscrowStatus::Released).expect("valid");
        assert!(!sm.funds_locked());
    }

    #[test]
    fn set_escrow_account() {
        let mut sm = EscrowStateMachine::new(test_agreement());
        assert!(sm.agreement().escrow_account.is_none());

        sm.set_escrow_account("5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY");
        assert_eq!(
            sm.agreement().escrow_account.as_deref(),
            Some("5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY")
        );
    }

    #[test]
    fn agreement_accessible() {
        let agreement = test_agreement();
        let sm = EscrowStateMachine::new(agreement.clone());
        assert_eq!(sm.agreement().buyer_agent_id, "agent-buyer");
        assert_eq!(sm.agreement().seller_agent_id, "agent-seller");
        assert_eq!(sm.agreement().timeout_blocks, 100);
    }

    #[test]
    fn history_timestamps_increase() {
        let mut sm = EscrowStateMachine::new(test_agreement());
        sm.transition(EscrowStatus::Funded).expect("valid");
        sm.transition(EscrowStatus::Active).expect("valid");
        let history = sm.history();
        for window in history.windows(2) {
            assert!(window[1].1 >= window[0].1);
        }
    }

    #[test]
    fn state_machine_serde_round_trip() {
        let mut sm = EscrowStateMachine::new(test_agreement());
        sm.transition(EscrowStatus::Funded).expect("valid");
        let json = serde_json::to_string(&sm).expect("serialize");
        let back: EscrowStateMachine = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.current_status(), EscrowStatus::Funded);
        assert_eq!(back.history().len(), 2);
    }

    #[test]
    fn agreement_serde_round_trip() {
        let agreement = test_agreement();
        let json = serde_json::to_string(&agreement).expect("serialize");
        let back: EscrowAgreement = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(agreement.id, back.id);
        assert_eq!(agreement.buyer_agent_id, back.buyer_agent_id);
        assert_eq!(agreement.amount, back.amount);
    }
}
