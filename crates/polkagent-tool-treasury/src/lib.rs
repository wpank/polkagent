//! `polkagent-tool-treasury` — Treasury and portfolio analysis tools for
//! Polkagent agents.
//!
//! This crate provides a suite of read-only chain query tools that let agents
//! analyze DOT/KSM balances, staking positions, crowdloans, transfer history,
//! vesting schedules, and aggregated asset portfolios on Polkadot-SDK chains.
//!
//! # Tools
//!
//! | Tool | Description |
//! |------|-------------|
//! | [`BalanceQueryTool`] | Free/reserved/frozen/total balance for an account |
//! | [`StakingInfoTool`] | Validator/nominator status, bonded amounts, rewards |
//! | [`PortfolioSummaryTool`] | Aggregated view of all asset positions |
//! | [`TransferHistoryTool`] | Recent transfers in/out with timestamps |
//! | [`VestingScheduleTool`] | Vesting schedules, unlocked amount, next unlock |
//!
//! All tools require the `chain.query` grant (read-only chain access) and
//! accept an `account_id` (SS58 string) plus an optional `chain` identifier
//! (defaults to `"polkadot"`).
//!
//! # Registration
//!
//! Use [`register_treasury_tools`] to add all treasury tools to a
//! [`ToolRegistry`] in one call:
//!
//! ```rust,no_run
//! use polkagent_tool::ToolRegistry;
//! use polkagent_tool_treasury::register_treasury_tools;
//!
//! let mut registry = ToolRegistry::new();
//! register_treasury_tools(&mut registry);
//! ```

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

pub mod balance;
pub mod error;
pub mod portfolio;
pub mod staking;
pub mod transfer;
pub mod types;
pub mod vesting;

// ---------------------------------------------------------------------------
// Convenience re-exports
// ---------------------------------------------------------------------------

pub use balance::BalanceQueryTool;
pub use error::TreasuryToolError;
pub use portfolio::PortfolioSummaryTool;
pub use staking::StakingInfoTool;
pub use transfer::TransferHistoryTool;
pub use types::{
    AccountBalance, PortfolioEntry, PortfolioSource, StakingPosition, StakingRole, Transfer,
    UnlockingChunk, VestingInfo, VestingSchedule,
};
pub use vesting::VestingScheduleTool;

use polkagent_tool::ToolRegistry;

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// Register all treasury and portfolio analysis tools into the given registry.
///
/// Adds:
/// - [`BalanceQueryTool`] — account balance breakdown
/// - [`StakingInfoTool`] — staking position details
/// - [`PortfolioSummaryTool`] — aggregated portfolio view
/// - [`TransferHistoryTool`] — recent transfer events
/// - [`VestingScheduleTool`] — vesting schedule information
///
/// Each tool requires a `chain.query` grant for execution.
pub fn register_treasury_tools(registry: &mut ToolRegistry) {
    registry.register(Box::new(BalanceQueryTool));
    registry.register(Box::new(StakingInfoTool));
    registry.register(Box::new(PortfolioSummaryTool));
    registry.register(Box::new(TransferHistoryTool));
    registry.register(Box::new(VestingScheduleTool));
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use polkagent_tool::ToolHandler;

    #[test]
    fn register_treasury_tools_adds_five_tools() {
        let mut registry = ToolRegistry::new();
        register_treasury_tools(&mut registry);
        assert_eq!(registry.len(), 5);
    }

    #[test]
    fn all_tool_names_are_namespaced() {
        let mut registry = ToolRegistry::new();
        register_treasury_tools(&mut registry);

        for spec in registry.list() {
            assert!(
                spec.name.starts_with("polkagent.treasury."),
                "tool name '{}' does not start with 'polkagent.treasury.'",
                spec.name
            );
        }
    }

    #[test]
    fn all_tools_require_chain_query_grant() {
        let mut registry = ToolRegistry::new();
        register_treasury_tools(&mut registry);

        for spec in registry.list() {
            assert_eq!(
                spec.required_grant.as_deref(),
                Some("chain.query"),
                "tool '{}' should require chain.query grant",
                spec.name
            );
        }
    }

    #[test]
    fn all_tools_have_account_id_in_schema() {
        let mut registry = ToolRegistry::new();
        register_treasury_tools(&mut registry);

        for spec in registry.list() {
            let has_account_id = spec.input_schema["properties"].get("account_id").is_some();
            assert!(
                has_account_id,
                "tool '{}' should have account_id in input schema",
                spec.name
            );
        }
    }

    #[test]
    fn tool_specs_are_individually_accessible() {
        let balance_spec = BalanceQueryTool.spec();
        assert_eq!(balance_spec.name, "polkagent.treasury.balance_query");

        let staking_spec = StakingInfoTool.spec();
        assert_eq!(staking_spec.name, "polkagent.treasury.staking_info");

        let portfolio_spec = PortfolioSummaryTool.spec();
        assert_eq!(portfolio_spec.name, "polkagent.treasury.portfolio_summary");

        let transfer_spec = TransferHistoryTool.spec();
        assert_eq!(transfer_spec.name, "polkagent.treasury.transfer_history");

        let vesting_spec = VestingScheduleTool.spec();
        assert_eq!(vesting_spec.name, "polkagent.treasury.vesting_schedule");
    }

    #[test]
    fn registry_can_look_up_each_tool() {
        let mut registry = ToolRegistry::new();
        register_treasury_tools(&mut registry);

        assert!(registry.get("polkagent.treasury.balance_query").is_some());
        assert!(registry.get("polkagent.treasury.staking_info").is_some());
        assert!(registry
            .get("polkagent.treasury.portfolio_summary")
            .is_some());
        assert!(registry
            .get("polkagent.treasury.transfer_history")
            .is_some());
        assert!(registry
            .get("polkagent.treasury.vesting_schedule")
            .is_some());
    }

    #[test]
    fn output_classification_is_internal() {
        let mut registry = ToolRegistry::new();
        register_treasury_tools(&mut registry);

        for spec in registry.list() {
            assert_eq!(
                spec.output_classification,
                polkagent_core::config::DataClassification::Internal,
                "tool '{}' should have Internal output classification",
                spec.name
            );
        }
    }

    #[test]
    fn re_registering_replaces_tools() {
        let mut registry = ToolRegistry::new();
        register_treasury_tools(&mut registry);
        assert_eq!(registry.len(), 5);

        // Register again: should replace, not duplicate.
        register_treasury_tools(&mut registry);
        assert_eq!(registry.len(), 5);
    }
}
