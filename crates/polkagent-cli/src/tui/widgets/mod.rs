//! Reusable TUI widget components.
//!
//! Each widget is a standalone rendering function or type that accepts a
//! `&Theme` and a `&Frame` area to draw into. Widgets never mutate state.

pub mod header_bar;
pub mod status_bar;
