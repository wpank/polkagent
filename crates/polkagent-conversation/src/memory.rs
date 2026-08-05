//! In-memory implementation of [`ConversationStore`].
//!
//! [`InMemoryConversationStore`] is backed by `RwLock<HashMap>` structures and
//! is fully thread-safe. It is intended for tests and lightweight use cases
//! where persistence is not required.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use tokio::sync::RwLock;
use uuid::Uuid;

use polkagent_core::ids::{AgentId, ConversationId};

use crate::error::{ConversationError, ConversationResult};
use crate::store::ConversationStore;
use crate::types::{Conversation, ConversationSummary, Message};

/// Thread-safe, in-memory conversation store.
///
/// Data lives only for the lifetime of the store instance. Useful for unit
/// tests and development scenarios that do not need durability.
#[derive(Debug, Clone)]
pub struct InMemoryConversationStore {
    /// Conversation records keyed by conversation ID.
    conversations: Arc<RwLock<HashMap<ConversationId, Conversation>>>,
    /// Messages keyed by conversation ID, stored in insertion order.
    messages: Arc<RwLock<HashMap<ConversationId, Vec<Message>>>>,
}

impl InMemoryConversationStore {
    /// Create a new, empty in-memory store.
    #[must_use]
    pub fn new() -> Self {
        Self {
            conversations: Arc::new(RwLock::new(HashMap::new())),
            messages: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

impl Default for InMemoryConversationStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ConversationStore for InMemoryConversationStore {
    async fn create(&self, conversation: Conversation) -> ConversationResult<ConversationId> {
        let id = conversation.id;
        let mut convs = self.conversations.write().await;
        let mut msgs = self.messages.write().await;
        convs.insert(id, conversation);
        msgs.insert(id, Vec::new());
        Ok(id)
    }

    async fn get(&self, id: ConversationId) -> ConversationResult<Conversation> {
        let convs = self.conversations.read().await;
        convs
            .get(&id)
            .cloned()
            .ok_or_else(|| ConversationError::NotFound(id.to_string()))
    }

    async fn list(
        &self,
        agent_id: AgentId,
        limit: usize,
        offset: usize,
    ) -> ConversationResult<Vec<ConversationSummary>> {
        let convs = self.conversations.read().await;
        let mut summaries: Vec<ConversationSummary> = convs
            .values()
            .filter(|c| c.agent_id == agent_id)
            .map(ConversationSummary::from)
            .collect();
        // Sort by updated_at descending (most recent first).
        summaries.sort_by_key(|summary| std::cmp::Reverse(summary.created_at));
        // Re-sort by last_message_at when available for better ordering.
        summaries.sort_by(|a, b| {
            let a_time = a.last_message_at.unwrap_or(a.created_at);
            let b_time = b.last_message_at.unwrap_or(b.created_at);
            b_time.cmp(&a_time)
        });
        let result = summaries.into_iter().skip(offset).take(limit).collect();
        Ok(result)
    }

    async fn delete(&self, id: ConversationId) -> ConversationResult<()> {
        let mut convs = self.conversations.write().await;
        let mut msgs = self.messages.write().await;
        if convs.remove(&id).is_none() {
            return Err(ConversationError::NotFound(id.to_string()));
        }
        msgs.remove(&id);
        Ok(())
    }

    async fn add_message(
        &self,
        conversation_id: ConversationId,
        message: Message,
    ) -> ConversationResult<Uuid> {
        let msg_id = message.id;
        let mut convs = self.conversations.write().await;
        let mut msgs = self.messages.write().await;

        let conv = convs
            .get_mut(&conversation_id)
            .ok_or_else(|| ConversationError::NotFound(conversation_id.to_string()))?;

        conv.message_count += 1;
        conv.updated_at = Utc::now();

        msgs.entry(conversation_id).or_default().push(message);

        Ok(msg_id)
    }

    async fn get_messages(
        &self,
        conversation_id: ConversationId,
        limit: usize,
        offset: usize,
    ) -> ConversationResult<Vec<Message>> {
        let convs = self.conversations.read().await;
        if !convs.contains_key(&conversation_id) {
            return Err(ConversationError::NotFound(conversation_id.to_string()));
        }
        drop(convs);

        let msgs = self.messages.read().await;
        let result = msgs
            .get(&conversation_id)
            .map(|m| m.iter().skip(offset).take(limit).cloned().collect())
            .unwrap_or_default();
        Ok(result)
    }

    async fn get_recent_messages(
        &self,
        conversation_id: ConversationId,
        limit: usize,
    ) -> ConversationResult<Vec<Message>> {
        let convs = self.conversations.read().await;
        if !convs.contains_key(&conversation_id) {
            return Err(ConversationError::NotFound(conversation_id.to_string()));
        }
        drop(convs);

        let msgs = self.messages.read().await;
        let result = msgs
            .get(&conversation_id)
            .map(|m| {
                let start = m.len().saturating_sub(limit);
                m[start..].to_vec()
            })
            .unwrap_or_default();
        Ok(result)
    }

    async fn update_title(&self, id: ConversationId, title: String) -> ConversationResult<()> {
        let mut convs = self.conversations.write().await;
        let conv = convs
            .get_mut(&id)
            .ok_or_else(|| ConversationError::NotFound(id.to_string()))?;
        conv.title = Some(title);
        conv.updated_at = Utc::now();
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{MessageContent, MessageRole};

    fn make_message(conversation_id: ConversationId, role: MessageRole, text: &str) -> Message {
        Message {
            id: Uuid::now_v7(),
            conversation_id,
            role,
            content: MessageContent::Text { text: text.into() },
            created_at: Utc::now(),
            token_count: None,
        }
    }

    #[tokio::test]
    async fn create_and_get_conversation() {
        let store = InMemoryConversationStore::new();
        let conv = Conversation::new(ConversationId::new(), AgentId::new());
        let id = store.create(conv.clone()).await.expect("create");
        let fetched = store.get(id).await.expect("get");
        assert_eq!(fetched.id, conv.id);
        assert_eq!(fetched.agent_id, conv.agent_id);
    }

    #[tokio::test]
    async fn get_nonexistent_returns_not_found() {
        let store = InMemoryConversationStore::new();
        let result = store.get(ConversationId::new()).await;
        assert!(matches!(result, Err(ConversationError::NotFound(_))));
    }

    #[tokio::test]
    async fn delete_conversation() {
        let store = InMemoryConversationStore::new();
        let conv = Conversation::new(ConversationId::new(), AgentId::new());
        let id = store.create(conv).await.expect("create");
        store.delete(id).await.expect("delete");
        let result = store.get(id).await;
        assert!(matches!(result, Err(ConversationError::NotFound(_))));
    }

    #[tokio::test]
    async fn delete_nonexistent_returns_not_found() {
        let store = InMemoryConversationStore::new();
        let result = store.delete(ConversationId::new()).await;
        assert!(matches!(result, Err(ConversationError::NotFound(_))));
    }

    #[tokio::test]
    async fn add_and_get_messages() {
        let store = InMemoryConversationStore::new();
        let conv = Conversation::new(ConversationId::new(), AgentId::new());
        let conv_id = store.create(conv).await.expect("create");

        let m1 = make_message(conv_id, MessageRole::User, "Hello");
        let m2 = make_message(conv_id, MessageRole::Assistant, "Hi there!");
        store
            .add_message(conv_id, m1.clone())
            .await
            .expect("add m1");
        store
            .add_message(conv_id, m2.clone())
            .await
            .expect("add m2");

        let messages = store
            .get_messages(conv_id, 10, 0)
            .await
            .expect("get messages");
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].id, m1.id);
        assert_eq!(messages[1].id, m2.id);

        // Verify conversation message count was updated.
        let fetched = store.get(conv_id).await.expect("get");
        assert_eq!(fetched.message_count, 2);
    }

    #[tokio::test]
    async fn add_message_to_nonexistent_conversation() {
        let store = InMemoryConversationStore::new();
        let conv_id = ConversationId::new();
        let msg = make_message(conv_id, MessageRole::User, "Hello");
        let result = store.add_message(conv_id, msg).await;
        assert!(matches!(result, Err(ConversationError::NotFound(_))));
    }

    #[tokio::test]
    async fn get_messages_with_pagination() {
        let store = InMemoryConversationStore::new();
        let conv = Conversation::new(ConversationId::new(), AgentId::new());
        let conv_id = store.create(conv).await.expect("create");

        for i in 0..5 {
            let msg = make_message(conv_id, MessageRole::User, &format!("Message {i}"));
            store.add_message(conv_id, msg).await.expect("add");
        }

        let page = store
            .get_messages(conv_id, 2, 1)
            .await
            .expect("get messages");
        assert_eq!(page.len(), 2);
    }

    #[tokio::test]
    async fn get_recent_messages() {
        let store = InMemoryConversationStore::new();
        let conv = Conversation::new(ConversationId::new(), AgentId::new());
        let conv_id = store.create(conv).await.expect("create");

        for i in 0..10 {
            let msg = make_message(conv_id, MessageRole::User, &format!("Message {i}"));
            store.add_message(conv_id, msg).await.expect("add");
        }

        let recent = store.get_recent_messages(conv_id, 3).await.expect("recent");
        assert_eq!(recent.len(), 3);
        // Verify they are the last 3 messages (chronological order).
        if let MessageContent::Text { text } = &recent[0].content {
            assert_eq!(text, "Message 7");
        } else {
            panic!("expected text content");
        }
    }

    #[tokio::test]
    async fn get_recent_messages_nonexistent_conversation() {
        let store = InMemoryConversationStore::new();
        let result = store.get_recent_messages(ConversationId::new(), 5).await;
        assert!(matches!(result, Err(ConversationError::NotFound(_))));
    }

    #[tokio::test]
    async fn update_title() {
        let store = InMemoryConversationStore::new();
        let conv = Conversation::new(ConversationId::new(), AgentId::new());
        let id = store.create(conv).await.expect("create");

        store
            .update_title(id, "My Chat".into())
            .await
            .expect("update title");

        let fetched = store.get(id).await.expect("get");
        assert_eq!(fetched.title, Some("My Chat".into()));
    }

    #[tokio::test]
    async fn update_title_nonexistent() {
        let store = InMemoryConversationStore::new();
        let result = store
            .update_title(ConversationId::new(), "title".into())
            .await;
        assert!(matches!(result, Err(ConversationError::NotFound(_))));
    }

    #[tokio::test]
    async fn list_conversations_for_agent() {
        let store = InMemoryConversationStore::new();
        let agent_a = AgentId::new();
        let agent_b = AgentId::new();

        for _ in 0..3 {
            let conv = Conversation::new(ConversationId::new(), agent_a);
            store.create(conv).await.expect("create");
        }
        let conv_b = Conversation::new(ConversationId::new(), agent_b);
        store.create(conv_b).await.expect("create");

        let summaries = store.list(agent_a, 10, 0).await.expect("list");
        assert_eq!(summaries.len(), 3);

        // All should belong to agent_a.
        for s in &summaries {
            assert_eq!(s.agent_id, agent_a);
        }
    }

    #[tokio::test]
    async fn list_with_pagination() {
        let store = InMemoryConversationStore::new();
        let agent = AgentId::new();

        for _ in 0..5 {
            let conv = Conversation::new(ConversationId::new(), agent);
            store.create(conv).await.expect("create");
        }

        let page = store.list(agent, 2, 1).await.expect("list");
        assert_eq!(page.len(), 2);
    }

    #[tokio::test]
    async fn delete_removes_messages_too() {
        let store = InMemoryConversationStore::new();
        let conv = Conversation::new(ConversationId::new(), AgentId::new());
        let conv_id = store.create(conv).await.expect("create");

        let msg = make_message(conv_id, MessageRole::User, "Hello");
        store.add_message(conv_id, msg).await.expect("add");

        store.delete(conv_id).await.expect("delete");

        // Messages should also be gone.
        let result = store.get_messages(conv_id, 10, 0).await;
        assert!(matches!(result, Err(ConversationError::NotFound(_))));
    }

    #[tokio::test]
    async fn clone_shares_state() {
        let store = InMemoryConversationStore::new();
        let store2 = store.clone();
        let conv = Conversation::new(ConversationId::new(), AgentId::new());
        let id = store.create(conv).await.expect("create");

        // The clone should see the same data.
        let fetched = store2.get(id).await.expect("get from clone");
        assert_eq!(fetched.id, id);
    }
}
