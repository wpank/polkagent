//! **EXPERIMENTAL** — Agent earn/spend ledger for PRD-08 deliverable 5.2.
//!
//! This module provides a prototype in-memory ledger that tracks simulated
//! micropayments an agent earns (by completing skill tasks) and spends
//! (by invoking tool calls). It also includes x402 payment header
//! construction for HTTP-based tool invocations.
//!
//! # Status
//!
//! **Research prototype — not production-ready.**

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::PaymentError;
use crate::types::{Amount, AssetId};

// ---------------------------------------------------------------------------
// LedgerEntryKind
// ---------------------------------------------------------------------------

/// The kind of ledger entry.
///
/// **EXPERIMENTAL** — part of the PRD-08 §5.2 earn/spend prototype.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LedgerEntryKind {
    /// An earning from completing a skill task.
    Earn {
        /// The skill that was completed.
        skill_name: String,
    },
    /// A spend to invoke a tool call.
    Spend {
        /// The tool that was invoked.
        tool_name: String,
    },
}

// ---------------------------------------------------------------------------
// LedgerEntry
// ---------------------------------------------------------------------------

/// A single entry in the agent's payment ledger.
///
/// **EXPERIMENTAL** — part of the PRD-08 §5.2 earn/spend prototype.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerEntry {
    /// Unique identifier for this ledger entry.
    pub id: Uuid,
    /// Whether this is an earn or spend event.
    pub kind: LedgerEntryKind,
    /// The amount in the smallest denomination (e.g. planck).
    pub amount_planck: u128,
    /// Human-readable description.
    pub description: String,
    /// When this entry was recorded.
    pub timestamp: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// X402PaymentHeader
// ---------------------------------------------------------------------------

/// A prototype x402 payment header for HTTP tool calls.
///
/// The x402 protocol uses HTTP 402 Payment Required responses to signal
/// that a resource requires payment. This header is attached to outgoing
/// HTTP requests to prove payment was made.
///
/// **EXPERIMENTAL** — signature field is a placeholder; no real
/// cryptographic signing is performed in this prototype.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct X402PaymentHeader {
    /// Protocol scheme (always `"x402"`).
    pub scheme: String,
    /// Payment amount in the smallest denomination.
    pub amount_planck: u128,
    /// Asset identifier string (e.g. `"NATIVE"`, `"USDT"`).
    pub asset: String,
    /// Payer agent identifier.
    pub payer: String,
    /// Simulated signature (placeholder for prototype).
    pub signature: String,
}

impl X402PaymentHeader {
    /// Format the header as an HTTP header value.
    ///
    /// Format: `x402 amount=<planck>;asset=<asset>;payer=<id>;sig=<sig>`
    #[must_use]
    pub fn to_header_value(&self) -> String {
        format!(
            "x402 amount={};asset={};payer={};sig={}",
            self.amount_planck, self.asset, self.payer, self.signature
        )
    }
}

// ---------------------------------------------------------------------------
// Ledger
// ---------------------------------------------------------------------------

/// An in-memory ledger tracking an agent's earnings and spending.
///
/// **EXPERIMENTAL** — This is a research prototype for PRD-08 §5.2.
/// It uses an in-memory `Vec` and is not persisted across restarts.
///
/// # Flow
///
/// 1. Agent completes a skill task → [`record_earning`](Self::record_earning)
/// 2. Agent invokes a tool call → [`record_spend`](Self::record_spend)
///    or [`prepare_x402_payment`](Self::prepare_x402_payment)
/// 3. Ledger tracks cumulative balance, earnings, and spending
#[derive(Debug, Clone)]
pub struct Ledger {
    agent_id: String,
    entries: Vec<LedgerEntry>,
    balance_planck: u128,
    asset: AssetId,
    decimals: u8,
}

impl Ledger {
    /// Create a new empty ledger for the given agent.
    #[must_use]
    pub fn new(agent_id: impl Into<String>, asset: AssetId, decimals: u8) -> Self {
        Self {
            agent_id: agent_id.into(),
            entries: Vec::new(),
            balance_planck: 0,
            asset,
            decimals,
        }
    }

    /// Record an earning (e.g. from completing a skill task).
    ///
    /// Credits `amount_planck` to the agent's balance and appends a
    /// ledger entry.
    pub fn record_earning(
        &mut self,
        skill_name: impl Into<String>,
        amount_planck: u128,
        description: impl Into<String>,
    ) -> Result<Uuid, PaymentError> {
        let new_balance = self
            .balance_planck
            .checked_add(amount_planck)
            .ok_or(PaymentError::ArithmeticOverflow {
                context: "ledger earning overflow".into(),
            })?;

        let id = Uuid::now_v7();
        self.entries.push(LedgerEntry {
            id,
            kind: LedgerEntryKind::Earn {
                skill_name: skill_name.into(),
            },
            amount_planck,
            description: description.into(),
            timestamp: Utc::now(),
        });
        self.balance_planck = new_balance;
        Ok(id)
    }

    /// Record a spend (e.g. to invoke a tool call).
    ///
    /// Debits `amount_planck` from the agent's balance. Returns
    /// [`PaymentError::InsufficientBalance`] if the balance is too low.
    pub fn record_spend(
        &mut self,
        tool_name: impl Into<String>,
        amount_planck: u128,
        description: impl Into<String>,
    ) -> Result<Uuid, PaymentError> {
        if amount_planck > self.balance_planck {
            return Err(PaymentError::InsufficientBalance {
                available_planck: self.balance_planck,
                required_planck: amount_planck,
            });
        }

        let id = Uuid::now_v7();
        self.entries.push(LedgerEntry {
            id,
            kind: LedgerEntryKind::Spend {
                tool_name: tool_name.into(),
            },
            amount_planck,
            description: description.into(),
            timestamp: Utc::now(),
        });
        self.balance_planck -= amount_planck;
        Ok(id)
    }

    /// Build an x402 payment header for an HTTP tool call.
    ///
    /// This debits the ledger and returns a header that can be attached
    /// to an outgoing HTTP request. Combines [`record_spend`](Self::record_spend)
    /// with header construction.
    pub fn prepare_x402_payment(
        &mut self,
        tool_name: impl Into<String>,
        amount_planck: u128,
    ) -> Result<X402PaymentHeader, PaymentError> {
        let tool = tool_name.into();
        let description = format!("x402 payment for tool '{tool}'");
        self.record_spend(&tool, amount_planck, &description)?;

        Ok(X402PaymentHeader {
            scheme: "x402".into(),
            amount_planck,
            asset: self.asset.to_string(),
            payer: self.agent_id.clone(),
            signature: "experimental-no-sig".into(),
        })
    }

    /// Current balance in the smallest denomination (planck).
    #[must_use]
    pub fn balance_planck(&self) -> u128 {
        self.balance_planck
    }

    /// Current balance as an [`Amount`].
    #[must_use]
    pub fn balance(&self) -> Amount {
        Amount::new(self.balance_planck, self.asset.clone(), self.decimals)
    }

    /// Total earnings across all entries.
    #[must_use]
    pub fn total_earned_planck(&self) -> u128 {
        self.entries
            .iter()
            .filter(|e| matches!(e.kind, LedgerEntryKind::Earn { .. }))
            .map(|e| e.amount_planck)
            .sum()
    }

    /// Total spending across all entries.
    #[must_use]
    pub fn total_spent_planck(&self) -> u128 {
        self.entries
            .iter()
            .filter(|e| matches!(e.kind, LedgerEntryKind::Spend { .. }))
            .map(|e| e.amount_planck)
            .sum()
    }

    /// The agent ID this ledger belongs to.
    #[must_use]
    pub fn agent_id(&self) -> &str {
        &self.agent_id
    }

    /// All ledger entries in chronological order.
    #[must_use]
    pub fn entries(&self) -> &[LedgerEntry] {
        &self.entries
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn test_ledger() -> Ledger {
        Ledger::new("agent-1", AssetId::Native, 10)
    }

    #[test]
    fn new_ledger_has_zero_balance() {
        let ledger = test_ledger();
        assert_eq!(ledger.balance_planck(), 0);
        assert!(ledger.entries().is_empty());
    }

    #[test]
    fn record_earning_increases_balance() {
        let mut ledger = test_ledger();
        let id = ledger
            .record_earning("data-analysis", 1_000_000, "completed data task")
            .expect("should succeed");
        assert_eq!(ledger.balance_planck(), 1_000_000);
        assert_eq!(ledger.entries().len(), 1);
        assert_eq!(ledger.entries()[0].id, id);
    }

    #[test]
    fn record_spend_decreases_balance() {
        let mut ledger = test_ledger();
        ledger
            .record_earning("skill-a", 5_000, "earned")
            .expect("earn");
        ledger
            .record_spend("http_fetch", 2_000, "tool call")
            .expect("spend");
        assert_eq!(ledger.balance_planck(), 3_000);
    }

    #[test]
    fn spend_over_balance_fails() {
        let mut ledger = test_ledger();
        ledger
            .record_earning("skill-a", 1_000, "earned")
            .expect("earn");
        let err = ledger
            .record_spend("http_fetch", 2_000, "over-spend")
            .unwrap_err();
        assert!(matches!(err, PaymentError::InsufficientBalance { .. }));
        assert_eq!(ledger.balance_planck(), 1_000);
    }

    #[test]
    fn totals_track_correctly() {
        let mut ledger = test_ledger();
        ledger.record_earning("s1", 100, "e1").expect("earn");
        ledger.record_earning("s2", 200, "e2").expect("earn");
        ledger.record_spend("t1", 50, "s1").expect("spend");
        assert_eq!(ledger.total_earned_planck(), 300);
        assert_eq!(ledger.total_spent_planck(), 50);
        assert_eq!(ledger.balance_planck(), 250);
    }

    #[test]
    fn x402_header_format() {
        let mut ledger = test_ledger();
        ledger
            .record_earning("skill", 10_000, "earned")
            .expect("earn");
        let header = ledger
            .prepare_x402_payment("http_fetch", 5_000)
            .expect("payment");
        assert_eq!(header.scheme, "x402");
        assert_eq!(header.amount_planck, 5_000);
        assert_eq!(header.payer, "agent-1");
        let hv = header.to_header_value();
        assert!(hv.starts_with("x402 "));
        assert!(hv.contains("amount=5000"));
        assert!(hv.contains("payer=agent-1"));
        assert_eq!(ledger.balance_planck(), 5_000);
    }

    #[test]
    fn x402_payment_insufficient_balance() {
        let mut ledger = test_ledger();
        let err = ledger
            .prepare_x402_payment("expensive_tool", 1_000)
            .unwrap_err();
        assert!(matches!(err, PaymentError::InsufficientBalance { .. }));
    }

    #[test]
    fn balance_returns_amount() {
        let mut ledger = test_ledger();
        ledger.record_earning("s", 42_000, "e").expect("earn");
        let bal = ledger.balance();
        assert_eq!(bal.value, 42_000);
        assert_eq!(bal.asset, AssetId::Native);
        assert_eq!(bal.decimals, 10);
    }

    #[test]
    fn ledger_entry_serde_round_trip() {
        let entry = LedgerEntry {
            id: Uuid::now_v7(),
            kind: LedgerEntryKind::Earn {
                skill_name: "test-skill".into(),
            },
            amount_planck: 42,
            description: "test".into(),
            timestamp: Utc::now(),
        };
        let json = serde_json::to_string(&entry).expect("serialize");
        let back: LedgerEntry = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(entry.id, back.id);
        assert_eq!(entry.amount_planck, back.amount_planck);
    }

    #[test]
    fn x402_header_serde_round_trip() {
        let header = X402PaymentHeader {
            scheme: "x402".into(),
            amount_planck: 1000,
            asset: "NATIVE".into(),
            payer: "agent-1".into(),
            signature: "sig".into(),
        };
        let json = serde_json::to_string(&header).expect("serialize");
        let back: X402PaymentHeader = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(header, back);
    }
}
