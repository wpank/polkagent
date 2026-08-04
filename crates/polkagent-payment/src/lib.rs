//! `polkagent-payment` — Payment tracking, budget enforcement, and cost
//! estimation for the Polkagent platform.
//!
//! # Overview
//!
//! This crate implements the payment and budget layer described in PRD-08. It
//! provides types for on-chain payment intents/receipts, LLM cost tracking,
//! budget enforcement, and cost estimation.
//!
//! # Module structure
//!
//! | Module | Responsibility |
//! |--------|---------------|
//! | [`types`] | Core domain types: [`AssetId`], [`Amount`], [`PaymentIntent`], [`PaymentStatus`], [`PaymentReceipt`], [`CostRecord`], [`UsageSummary`]. |
//! | [`budget`] | Budget enforcement: [`BudgetConfig`], [`BudgetState`], [`BudgetChecker`], [`BudgetDecision`]. |
//! | [`store`] | [`PaymentStore`] async trait for persistence. |
//! | [`estimator`] | [`CostEstimator`] with pricing tables for known LLM models. |
//! | [`action`] | [`PaymentAction`] enum for on-chain operation types. |
//! | [`intent`] | [`IntentStatus`], [`IntentStateMachine`], and validated transitions. |
//! | [`builder`] | [`PaymentIntentBuilder`] with fluent API. |
//! | [`finality`] | [`TransactionOutcome`] and [`FinalityWatcher`] trait. |
//! | [`preflight`] | Pre-flight checks: [`PreFlightCheck`] trait, [`CompositePreFlight`], balance/fee/ED/address/nonce/metadata checks. |
//! | [`error`] | [`PaymentError`] enum. |
//! | [`ledger`] | **EXPERIMENTAL** — Agent earn/spend [`Ledger`], [`X402PaymentHeader`] (PRD-08 §5.2 prototype). |
//! | [`escrow`] | **EXPERIMENTAL** — Agent-to-agent [`EscrowStateMachine`], [`EscrowAgreement`] (PRD-08 §5.3 prototype). |

#![forbid(unsafe_code)]
#![warn(
    missing_docs,
    clippy::pedantic,
    clippy::unwrap_used,
    clippy::expect_used
)]
#![allow(
    clippy::module_name_repetitions,
    clippy::must_use_candidate,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc
)]

pub mod action;
pub mod budget;
pub mod builder;
pub mod error;
pub mod escrow;
pub mod estimator;
pub mod finality;
pub mod intent;
pub mod ledger;
pub mod preflight;
pub mod risk;
pub mod store;
pub mod types;

// ---------------------------------------------------------------------------
// Convenience re-exports
// ---------------------------------------------------------------------------

pub use action::PaymentAction;
pub use budget::{BudgetChecker, BudgetConfig, BudgetDecision, BudgetState};
pub use builder::{BuiltIntent, PaymentIntentBuilder};
pub use error::PaymentError;
pub use escrow::{EscrowAgreement, EscrowStateMachine, EscrowStatus};
pub use estimator::{CostEstimator, PricingEntry};
pub use finality::{FinalityWatcher, TransactionOutcome};
pub use intent::{IntentStateMachine, IntentStatus};
pub use ledger::{Ledger, LedgerEntry, LedgerEntryKind, X402PaymentHeader};
pub use preflight::{
    AddressCheck, BalanceCheck, CompositePreFlight, ExistentialDepositCheck, FeeCheck,
    MetadataFreshnessCheck, NonceCheck, PreFlightBlocker, PreFlightCheck, PreFlightResult,
    PreFlightWarning, WarningSeverity,
};
pub use risk::{
    BatchHidingDetector, CompositeRiskGate, HighValueDetector, HomoglyphDetector, RiskCode,
    RiskFinding, RiskGate, RiskSeverity,
};
pub use store::{BalanceSummary, PaymentStore};
pub use types::{
    Amount, AssetId, CostRecord, PaymentIntent, PaymentReceipt, PaymentStatus, UsageSummary,
};
