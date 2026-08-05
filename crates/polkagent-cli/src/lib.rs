//! `polkagent-cli` library root.
//!
//! This module re-exports the TUI subsystem so that integration tests can
//! exercise theme, input, state, and widget logic without depending on the
//! binary entry point.

#![warn(clippy::pedantic)]
#![allow(clippy::module_name_repetitions, clippy::missing_errors_doc)]
// Assertion-oriented unit tests unwrap controlled fixtures for precise failures.
#![cfg_attr(
    test,
    allow(
        clippy::expect_used,
        clippy::unwrap_used,
        clippy::field_reassign_with_default,
        clippy::float_cmp,
        clippy::items_after_statements
    )
)]

pub mod cli;
pub mod commands;
pub mod error_explainer;
pub mod exit_codes;
pub mod output;
pub mod tui;
