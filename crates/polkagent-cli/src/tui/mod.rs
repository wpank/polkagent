//! Terminal User Interface module.
//!
//! The Polkagent TUI follows the Elm Architecture (TEA):
//!
//! 1. **State** ([`state::TuiState`]) — all mutable data.
//! 2. **Actions** ([`input::TuiAction`]) — the only way to mutate state.
//! 3. **View** — pure functions from `(&TuiState, &Theme)` to ratatui widgets.
//!
//! The [`app::App`] shell owns the terminal, drives the event loop, and is
//! the single point of mutation for `TuiState`. See [`app::App::run`] for the
//! frame loop.
//!
//! # Module map
//!
//! ```text
//! tui/
//!   app.rs      — App struct, event loop, render pipeline, terminal init/teardown
//!   state.rs    — TuiState, AgentSummary, RunSummary, SystemHealth
//!   theme.rs    — ROSEDUST colour palette (Theme struct + style helpers)
//!   input.rs    — TuiAction enum, key event translation
//!   db.rs       — Read-only SQLite queries for the TUI
//!   views/
//!     dashboard.rs — F1 Dashboard view
//!     agents.rs    — F2 Agents list + detail
//!     runs.rs      — F3 Runs list + detail
//!     system.rs    — F4 System health + config
//!   widgets/
//!     header_bar.rs — Top chrome bar
//!     status_bar.rs — Bottom chrome bar
//! ```

pub mod app;
pub mod db;
pub mod input;
pub mod state;
pub mod theme;
pub mod views;
pub mod widgets;
