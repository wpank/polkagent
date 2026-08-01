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
//! | [`error`] | [`PaymentError`] enum. |

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

pub mod budget;
pub mod error;
pub mod estimator;
pub mod store;
pub mod types;

// ---------------------------------------------------------------------------
// Convenience re-exports
// ---------------------------------------------------------------------------

pub use budget::{BudgetChecker, BudgetConfig, BudgetDecision, BudgetState};
pub use error::PaymentError;
pub use estimator::{CostEstimator, PricingEntry};
pub use store::{BalanceSummary, PaymentStore};
pub use types::{
    Amount, AssetId, CostRecord, PaymentIntent, PaymentReceipt, PaymentStatus, UsageSummary,
};
