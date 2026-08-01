//! Durable outbox with at-least-once delivery, FIFO partitioned ordering, and
//! consumer-side deduplication.
//!
//! # Overview
//!
//! The outbox pattern ensures that messages are persisted *before* they are
//! considered sent. This crate provides:
//!
//! * [`DurableOutbox`] — the core outbox: enqueue, lease-based claim,
//!   acknowledge, and nack with automatic dead-lettering.
//! * [`DeduplicationLog`] — tracks processed `(consumer_id, idempotency_key)`
//!   pairs within a configurable sliding window so that consumers can skip
//!   re-processing duplicate deliveries without executing side effects twice.
//! * [`ExponentialBackoff`] — exponential-backoff-with-full-jitter policy used
//!   by the outbox to compute retry delays.
//!
//! # Quick start
//!
//! ```rust
//! use polkagent_outbox::{DurableOutbox, DeduplicationLog, OutboxMessage};
//! use chrono::Duration;
//! use serde_json::json;
//!
//! let mut outbox = DurableOutbox::new();
//! let mut dedup = DeduplicationLog::with_default_window();
//!
//! // Producer: enqueue a message.
//! let msg = OutboxMessage::new("user:42", "order:99:created", json!({"amount": 100}), 3);
//! let id = outbox.enqueue(msg).expect("enqueue");
//!
//! // Consumer: claim and process.
//! if let Some(item) = outbox.claim_next("worker-1").expect("claim") {
//!     let idem_key = &item.message.idempotency_key;
//!     if !dedup.is_duplicate("worker-1", idem_key) {
//!         // … apply side effects …
//!         dedup.record_processed("worker-1", idem_key).expect("record");
//!     }
//!     outbox.acknowledge(item.id, "worker-1").expect("ack");
//! }
//! ```
//!
//! # Storage note
//!
//! The current backend is in-memory (`HashMap`/`BTreeMap`). A SQLite backend
//! will be added in a future iteration without changing this public API.

pub mod dedup;
pub mod dlq;
pub mod message;
pub mod outbox;
pub mod retry;

// Flat re-exports for ergonomic use.
pub use dedup::{DedupError, DeduplicationLog};
pub use dlq::{
    DeadLetter, DeadLetterId, DeadLetterQueue, DeliveryError, DlqError, DlqPolicy, DlqResult,
    DlqStats, InMemoryDlq, ReplayResult,
};
pub use message::{OutboxId, OutboxItem, OutboxMessage};
pub use outbox::{DurableOutbox, OutboxConfig, OutboxError};
pub use retry::ExponentialBackoff;
