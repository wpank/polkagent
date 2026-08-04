//! Abstract storage trait for the conversation subsystem.
//!
//! The [`ConversationStore`] trait defines the contract that any backing store
//! must implement. See [`crate::memory`] for an in-memory implementation
//! suitable for testing and simple use cases.

use async_trait::async_trait;
use uuid::Uuid;

use polkagent_core::ids::{AgentId, ConversationId};

use crate::error::ConversationResult;
use crate::types::{Conversation, ConversationSummary, Message};

/// Storage backend for conversations and their messages.
///
/// All methods are async and return [`ConversationResult`]. Implementations
/// must be `Send + Sync` so they can be shared across tasks.
#[async_trait]
pub trait ConversationStore: Send + Sync {
    /// Persist a new conversation and return its identifier.
    async fn create(&self, conversation: Conversation) -> ConversationResult<ConversationId>;

    /// Retrieve a conversation by its identifier.
    ///
    /// Returns [`ConversationError::NotFound`](crate::error::ConversationError::NotFound)
    /// if no conversation exists with the given ID.
    async fn get(&self, id: ConversationId) -> ConversationResult<Conversation>;

    /// List conversations for an agent, ordered by most recently updated first.
    ///
    /// `limit` caps the number of results; `offset` skips the first N results
    /// for pagination.
    async fn list(
        &self,
        agent_id: AgentId,
        limit: usize,
        offset: usize,
    ) -> ConversationResult<Vec<ConversationSummary>>;

    /// Delete a conversation and all its messages.
    ///
    /// Returns [`ConversationError::NotFound`](crate::error::ConversationError::NotFound)
    /// if no conversation exists with the given ID.
    async fn delete(&self, id: ConversationId) -> ConversationResult<()>;

    /// Append a message to a conversation and return the message's UUID.
    ///
    /// Implementations must also update the conversation's `message_count`
    /// and `updated_at` fields.
    async fn add_message(
        &self,
        conversation_id: ConversationId,
        message: Message,
    ) -> ConversationResult<Uuid>;

    /// Retrieve messages from a conversation with pagination.
    ///
    /// Messages are returned in chronological order (oldest first).
    /// `limit` caps the number of results; `offset` skips the first N.
    async fn get_messages(
        &self,
        conversation_id: ConversationId,
        limit: usize,
        offset: usize,
    ) -> ConversationResult<Vec<Message>>;

    /// Retrieve the most recent N messages from a conversation.
    ///
    /// Messages are returned in chronological order (oldest first), but
    /// only the last `limit` messages are included.
    async fn get_recent_messages(
        &self,
        conversation_id: ConversationId,
        limit: usize,
    ) -> ConversationResult<Vec<Message>>;

    /// Update the title of a conversation.
    ///
    /// Returns [`ConversationError::NotFound`](crate::error::ConversationError::NotFound)
    /// if no conversation exists with the given ID.
    async fn update_title(&self, id: ConversationId, title: String) -> ConversationResult<()>;
}
