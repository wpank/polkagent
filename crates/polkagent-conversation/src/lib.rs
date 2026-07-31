//! `polkagent-conversation` — Conversation management and context window for
//! Polkagent agents.
//!
//! This crate provides the types and storage abstractions for managing
//! multi-turn conversations between agents and their interlocutors. It also
//! includes a token-budget-aware context window that automatically evicts
//! older messages to fit within a model's context limit.
//!
//! # Module overview
//!
//! | Module | Contents |
//! |--------|----------|
//! | [`types`] | Core domain types: [`Conversation`], [`Message`], [`MessageRole`], [`MessageContent`] |
//! | [`store`] | Abstract [`ConversationStore`] trait |
//! | [`memory`] | In-memory [`InMemoryConversationStore`] implementation |
//! | [`context`] | [`ContextWindow`] for token-budget management |
//! | [`error`] | [`ConversationError`] enum |
//!
//! # Quickstart
//!
//! ```no_run
//! use polkagent_core::ids::{AgentId, ConversationId};
//! use polkagent_conversation::memory::InMemoryConversationStore;
//! use polkagent_conversation::store::ConversationStore;
//! use polkagent_conversation::types::{Conversation, Message, MessageContent, MessageRole};
//! use chrono::Utc;
//! use uuid::Uuid;
//!
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! let store = InMemoryConversationStore::new();
//! let conv = Conversation::new(ConversationId::new(), AgentId::new());
//! let conv_id = store.create(conv).await?;
//!
//! let msg = Message {
//!     id: Uuid::now_v7(),
//!     conversation_id: conv_id,
//!     role: MessageRole::User,
//!     content: MessageContent::Text { text: "Hello!".into() },
//!     created_at: Utc::now(),
//!     token_count: None,
//! };
//! store.add_message(conv_id, msg).await?;
//! # Ok(())
//! # }
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

pub mod context;
pub mod error;
pub mod memory;
pub mod store;
pub mod types;

// ---------------------------------------------------------------------------
// Convenience re-exports
// ---------------------------------------------------------------------------

pub use context::ContextWindow;
pub use error::{ConversationError, ConversationResult};
pub use memory::InMemoryConversationStore;
pub use store::ConversationStore;
pub use types::{
    ContentPart, Conversation, ConversationSummary, Message, MessageContent, MessageRole,
};
