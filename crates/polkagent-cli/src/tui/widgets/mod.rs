//! Reusable TUI widget components.
//!
//! Each widget is a standalone rendering function or type that accepts a
//! `&Theme` and a `&Frame` area to draw into. Widgets never mutate state.

pub mod action_card;
pub mod balance_display;
pub mod chain_status;
pub mod context_gauge;
pub mod error_digest;
pub mod header_bar;
pub mod progress_bar;
pub mod status_bar;
pub mod tab_bar;
pub mod token_sparkline;
