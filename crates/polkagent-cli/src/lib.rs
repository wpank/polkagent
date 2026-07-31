//! `polkagent-cli` library root.
//!
//! This module re-exports the TUI subsystem so that integration tests can
//! exercise theme, input, state, and widget logic without depending on the
//! binary entry point.

#![warn(clippy::pedantic)]
#![allow(clippy::module_name_repetitions, clippy::missing_errors_doc)]

pub mod exit_codes;
pub mod output;
pub mod tui;
