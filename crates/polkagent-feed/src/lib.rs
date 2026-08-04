//! `polkagent-feed` — Cursor-backed durable event feed processing, triggers,
//! and recipes.
//!
//! This crate implements the feed system described in PRD-09 §4.  It provides:
//!
//! - **[`types`]** — [`Feed`], [`FeedItem`], [`Cursor`], [`FeedSource`],
//!   [`FeedStatus`], [`EventFilter`], and the [`FeedId`] newtype.
//! - **[`trigger`]** — [`Trigger`], [`TriggerCondition`], [`TriggerAction`],
//!   [`CompOp`], and the [`evaluate_trigger`] function.
//! - **[`recipe`]** — [`Recipe`], [`RecipeParameter`], [`ParamType`], and the
//!   [`instantiate_recipe`] function.
//! - **[`store`]** — the [`FeedStore`] async trait.
//! - **[`memory_store`]** — [`MemoryStore`]: an in-memory [`FeedStore`] for
//!   tests.
//! - **[`processor`]** — [`FeedProcessor`]: the processing engine that binds
//!   feeds, items, triggers, and the store together.
//! - **[`durable`]** — [`DurableFeedStore`]: cursor persistence, atomic
//!   advance, deduplication, gap detection, and [`InMemoryDurableFeedStore`].
//! - **[`error`]** — [`FeedError`] and the crate-level [`Result`] alias.
//!
//! # Module layout
//!
//! | Module | Responsibility |
//! |---|---|
//! | [`types`] | Core domain types |
//! | [`trigger`] | Condition/action evaluation |
//! | [`recipe`] | Reusable trigger-to-action blueprints |
//! | [`store`] | Persistence abstraction trait |
//! | [`memory_store`] | In-memory store for tests |
//! | [`processor`] | Feed processing engine |
//! | [`durable`] | Durable cursor persistence, atomic advance, dedup, gap detection |
//! | [`error`] | [`FeedError`] enum |

pub mod durable;
pub mod error;
pub mod memory_store;
pub mod processor;
pub mod recipe;
pub mod store;
pub mod trigger;
pub mod types;

#[cfg(test)]
mod tests;

// ---------------------------------------------------------------------------
// Flat re-exports
// ---------------------------------------------------------------------------

pub use error::{FeedError, Result};

pub use types::{Cursor, EventFilter, Feed, FeedId, FeedItem, FeedSource, FeedStatus};

pub use trigger::{
    evaluate_trigger, CompOp, Trigger, TriggerAction, TriggerCondition, TriggerId, TriggerResult,
};

pub use recipe::{instantiate_recipe, ParamType, Recipe, RecipeId, RecipeParameter};

pub use store::FeedStore;

pub use memory_store::MemoryStore;

pub use processor::{initial_cursor, FeedProcessor};

pub use durable::{
    DurableFeedStore, FeedCursor, Gap, InMemoryDurableFeedStore, PendingAction, TriggerDedup,
};
