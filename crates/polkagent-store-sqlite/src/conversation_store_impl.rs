//! [`ConversationStore`] trait implementation for [`SqlitePool`].
//!
//! Wraps synchronous `rusqlite` calls in [`tokio::task::spawn_blocking`] to
//! satisfy the async trait interface. The writer connection is protected by a
//! `parking_lot::Mutex` inside `SqlitePool`, so each method acquires it briefly
//! within the blocking closure.

use async_trait::async_trait;
use chrono::DateTime;
use uuid::Uuid;

use polkagent_conversation::{
    ConversationError, ConversationResult, ConversationStore,
    types::{Conversation, ConversationSummary, Message, MessageContent, MessageRole},
};
use polkagent_core::ids::{AgentId, ConversationId};

use crate::pool::SqlitePool;

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Map a `rusqlite::Error` to `ConversationError`.
fn map_err(e: rusqlite::Error) -> ConversationError {
    ConversationError::Internal(format!("sqlite error: {e}"))
}

/// Parse an ISO-8601 timestamp string.
fn parse_ts(s: &str) -> ConversationResult<chrono::DateTime<chrono::Utc>> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .map_err(|e| ConversationError::Internal(format!("invalid timestamp '{s}': {e}")))
}

/// Encode a `MessageRole` as its snake_case string.
fn encode_role(role: MessageRole) -> &'static str {
    match role {
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::System => "system",
        MessageRole::Tool => "tool",
    }
}

/// Decode a `MessageRole` from its string representation.
fn decode_role(s: &str) -> ConversationResult<MessageRole> {
    match s {
        "user" => Ok(MessageRole::User),
        "assistant" => Ok(MessageRole::Assistant),
        "system" => Ok(MessageRole::System),
        "tool" => Ok(MessageRole::Tool),
        other => Err(ConversationError::Internal(format!(
            "unknown message role: '{other}'"
        ))),
    }
}

// ---------------------------------------------------------------------------
// ConversationStore implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl ConversationStore for SqlitePool {
    async fn create(&self, conversation: Conversation) -> ConversationResult<ConversationId> {
        let pool = self.clone();
        let id_str = conversation.id.to_string();
        let agent_id_str = conversation.agent_id.to_string();
        let metadata_json = serde_json::to_string(&conversation.metadata)
            .map_err(ConversationError::Json)?;

        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();
            writer
                .execute(
                    "INSERT INTO conversations
                         (id, agent_id, title, message_count, metadata_json,
                          created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    rusqlite::params![
                        id_str,
                        agent_id_str,
                        conversation.title,
                        conversation.message_count as i64,
                        metadata_json,
                        conversation.created_at.to_rfc3339(),
                        conversation.updated_at.to_rfc3339(),
                    ],
                )
                .map_err(|e| match &e {
                    rusqlite::Error::SqliteFailure(
                        rusqlite::ffi::Error {
                            code: rusqlite::ffi::ErrorCode::ConstraintViolation,
                            ..
                        },
                        _,
                    ) => ConversationError::InvalidOperation(format!(
                        "conversation '{}' already exists",
                        conversation.id
                    )),
                    _ => map_err(e),
                })?;
            Ok(conversation.id)
        })
        .await
        .map_err(|e| ConversationError::Internal(format!("blocking task panicked: {e}")))?
    }

    async fn get(&self, id: ConversationId) -> ConversationResult<Conversation> {
        let pool = self.clone();
        let id_str = id.to_string();

        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();
            let row = writer
                .query_row(
                    "SELECT id, agent_id, title, message_count, metadata_json,
                             created_at, updated_at
                      FROM conversations
                      WHERE id = ?1",
                    [&id_str],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, Option<String>>(2)?,
                            row.get::<_, i64>(3)?,
                            row.get::<_, String>(4)?,
                            row.get::<_, String>(5)?,
                            row.get::<_, String>(6)?,
                        ))
                    },
                )
                .map_err(|e| match e {
                    rusqlite::Error::QueryReturnedNoRows => {
                        ConversationError::NotFound(id_str.clone())
                    }
                    other => map_err(other),
                })?;

            let (id_s, agent_id_s, title, message_count, metadata_json_s, created_at_s, updated_at_s) = row;

            let conv_id: ConversationId = id_s
                .parse()
                .map_err(ConversationError::InvalidId)?;
            let agent_id: AgentId = agent_id_s
                .parse()
                .map_err(ConversationError::InvalidId)?;
            let metadata: std::collections::HashMap<String, String> =
                serde_json::from_str(&metadata_json_s).map_err(ConversationError::Json)?;
            let created_at = parse_ts(&created_at_s)?;
            let updated_at = parse_ts(&updated_at_s)?;

            Ok(Conversation {
                id: conv_id,
                agent_id,
                title,
                created_at,
                updated_at,
                message_count: message_count as u32,
                metadata,
            })
        })
        .await
        .map_err(|e| ConversationError::Internal(format!("blocking task panicked: {e}")))?
    }

    async fn list(
        &self,
        agent_id: AgentId,
        limit: usize,
        offset: usize,
    ) -> ConversationResult<Vec<ConversationSummary>> {
        let pool = self.clone();
        let agent_id_str = agent_id.to_string();

        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();
            let mut stmt = writer
                .prepare(
                    "SELECT c.id, c.agent_id, c.title, c.message_count, c.updated_at, c.created_at
                     FROM conversations c
                     WHERE c.agent_id = ?1
                     ORDER BY c.updated_at DESC
                     LIMIT ?2 OFFSET ?3",
                )
                .map_err(map_err)?;

            let rows = stmt
                .query_map(
                    rusqlite::params![agent_id_str, limit as i64, offset as i64],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, Option<String>>(2)?,
                            row.get::<_, i64>(3)?,
                            row.get::<_, String>(4)?,
                            row.get::<_, String>(5)?,
                        ))
                    },
                )
                .map_err(map_err)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(map_err)?;

            rows.into_iter()
                .map(|(id_s, agent_s, title, message_count, updated_at_s, created_at_s)| {
                    let conv_id: ConversationId = id_s.parse().map_err(ConversationError::InvalidId)?;
                    let ag_id: AgentId = agent_s.parse().map_err(ConversationError::InvalidId)?;
                    let updated_at = parse_ts(&updated_at_s)?;
                    let created_at = parse_ts(&created_at_s)?;
                    let last_message_at = if message_count > 0 { Some(updated_at) } else { None };
                    Ok(ConversationSummary {
                        id: conv_id,
                        agent_id: ag_id,
                        title,
                        message_count: message_count as u32,
                        last_message_at,
                        created_at,
                    })
                })
                .collect()
        })
        .await
        .map_err(|e| ConversationError::Internal(format!("blocking task panicked: {e}")))?
    }

    async fn delete(&self, id: ConversationId) -> ConversationResult<()> {
        let pool = self.clone();
        let id_str = id.to_string();

        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();
            // Messages are deleted via ON DELETE CASCADE on the FK.
            let n = writer
                .execute("DELETE FROM conversations WHERE id = ?1", [&id_str])
                .map_err(map_err)?;
            if n == 0 {
                return Err(ConversationError::NotFound(id_str));
            }
            Ok(())
        })
        .await
        .map_err(|e| ConversationError::Internal(format!("blocking task panicked: {e}")))?
    }

    async fn add_message(
        &self,
        conversation_id: ConversationId,
        message: Message,
    ) -> ConversationResult<Uuid> {
        let pool = self.clone();
        let conv_id_str = conversation_id.to_string();
        let msg_id = message.id;
        let msg_id_str = msg_id.to_string();
        let role_str = encode_role(message.role).to_string();
        let content_json = serde_json::to_string(&message.content).map_err(ConversationError::Json)?;

        tokio::task::spawn_blocking(move || {
            let now = chrono::Utc::now().to_rfc3339();
            let writer = pool.writer();

            // First check the conversation exists.
            let exists: bool = writer
                .query_row(
                    "SELECT COUNT(*) FROM conversations WHERE id = ?1",
                    [&conv_id_str],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(map_err)
                .map(|n| n > 0)?;

            if !exists {
                return Err(ConversationError::NotFound(conv_id_str));
            }

            // Insert the message.
            writer
                .execute(
                    "INSERT INTO conversation_messages
                         (id, conversation_id, role, content_json, token_count, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    rusqlite::params![
                        msg_id_str,
                        conv_id_str,
                        role_str,
                        content_json,
                        message.token_count.map(|c| c as i64),
                        message.created_at.to_rfc3339(),
                    ],
                )
                .map_err(map_err)?;

            // Update conversation: bump message_count and updated_at.
            writer
                .execute(
                    "UPDATE conversations
                     SET message_count = message_count + 1, updated_at = ?1
                     WHERE id = ?2",
                    rusqlite::params![now, conv_id_str],
                )
                .map_err(map_err)?;

            Ok(msg_id)
        })
        .await
        .map_err(|e| ConversationError::Internal(format!("blocking task panicked: {e}")))?
    }

    async fn get_messages(
        &self,
        conversation_id: ConversationId,
        limit: usize,
        offset: usize,
    ) -> ConversationResult<Vec<Message>> {
        let pool = self.clone();
        let conv_id_str = conversation_id.to_string();

        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();
            let mut stmt = writer
                .prepare(
                    "SELECT id, conversation_id, role, content_json, token_count, created_at
                     FROM conversation_messages
                     WHERE conversation_id = ?1
                     ORDER BY created_at ASC
                     LIMIT ?2 OFFSET ?3",
                )
                .map_err(map_err)?;

            let rows = stmt
                .query_map(
                    rusqlite::params![conv_id_str, limit as i64, offset as i64],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, Option<i64>>(4)?,
                            row.get::<_, String>(5)?,
                        ))
                    },
                )
                .map_err(map_err)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(map_err)?;

            rows.into_iter()
                .map(|(id_s, conv_s, role_s, content_s, token_count, created_at_s)| {
                    let msg_id = id_s.parse::<Uuid>().map_err(ConversationError::InvalidId)?;
                    let conv_id_parsed: ConversationId =
                        conv_s.parse().map_err(ConversationError::InvalidId)?;
                    let role = decode_role(&role_s)?;
                    let content: MessageContent =
                        serde_json::from_str(&content_s).map_err(ConversationError::Json)?;
                    let created_at = parse_ts(&created_at_s)?;
                    Ok(Message {
                        id: msg_id,
                        conversation_id: conv_id_parsed,
                        role,
                        content,
                        created_at,
                        token_count: token_count.map(|n| n as u32),
                    })
                })
                .collect()
        })
        .await
        .map_err(|e| ConversationError::Internal(format!("blocking task panicked: {e}")))?
    }

    async fn get_recent_messages(
        &self,
        conversation_id: ConversationId,
        limit: usize,
    ) -> ConversationResult<Vec<Message>> {
        let pool = self.clone();
        let conv_id_str = conversation_id.to_string();

        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();
            // Fetch the last `limit` rows ordered by created_at DESC, then re-sort ASC.
            let mut stmt = writer
                .prepare(
                    "SELECT id, conversation_id, role, content_json, token_count, created_at
                     FROM (
                         SELECT id, conversation_id, role, content_json, token_count, created_at
                         FROM conversation_messages
                         WHERE conversation_id = ?1
                         ORDER BY created_at DESC
                         LIMIT ?2
                     ) sub
                     ORDER BY created_at ASC",
                )
                .map_err(map_err)?;

            let rows = stmt
                .query_map(
                    rusqlite::params![conv_id_str, limit as i64],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, Option<i64>>(4)?,
                            row.get::<_, String>(5)?,
                        ))
                    },
                )
                .map_err(map_err)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(map_err)?;

            rows.into_iter()
                .map(|(id_s, conv_s, role_s, content_s, token_count, created_at_s)| {
                    let msg_id = id_s.parse::<Uuid>().map_err(ConversationError::InvalidId)?;
                    let conv_id_parsed: ConversationId =
                        conv_s.parse().map_err(ConversationError::InvalidId)?;
                    let role = decode_role(&role_s)?;
                    let content: MessageContent =
                        serde_json::from_str(&content_s).map_err(ConversationError::Json)?;
                    let created_at = parse_ts(&created_at_s)?;
                    Ok(Message {
                        id: msg_id,
                        conversation_id: conv_id_parsed,
                        role,
                        content,
                        created_at,
                        token_count: token_count.map(|n| n as u32),
                    })
                })
                .collect()
        })
        .await
        .map_err(|e| ConversationError::Internal(format!("blocking task panicked: {e}")))?
    }

    async fn update_title(
        &self,
        id: ConversationId,
        title: String,
    ) -> ConversationResult<()> {
        let pool = self.clone();
        let id_str = id.to_string();

        tokio::task::spawn_blocking(move || {
            let now = chrono::Utc::now().to_rfc3339();
            let writer = pool.writer();
            let n = writer
                .execute(
                    "UPDATE conversations SET title = ?1, updated_at = ?2 WHERE id = ?3",
                    rusqlite::params![title, now, id_str],
                )
                .map_err(map_err)?;
            if n == 0 {
                return Err(ConversationError::NotFound(id_str));
            }
            Ok(())
        })
        .await
        .map_err(|e| ConversationError::Internal(format!("blocking task panicked: {e}")))?
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migrations;
    use chrono::Utc;
    use polkagent_conversation::types::MessageContent;
    use polkagent_core::ids::{AgentId, ConversationId};

    fn test_pool() -> SqlitePool {
        let pool = SqlitePool::open_in_memory().expect("open in-memory pool");
        {
            let writer = pool.writer();
            migrations::migrate(&writer).expect("migrate");
        }
        pool
    }

    fn make_conversation(agent_id: AgentId) -> Conversation {
        Conversation::new(ConversationId::new(), agent_id)
    }

    fn make_message(conv_id: ConversationId, role: MessageRole, text: &str) -> Message {
        Message {
            id: Uuid::now_v7(),
            conversation_id: conv_id,
            role,
            content: MessageContent::Text { text: text.to_string() },
            created_at: Utc::now(),
            token_count: None,
        }
    }

    fn make_message_with_tokens(conv_id: ConversationId, role: MessageRole, text: &str, tokens: u32) -> Message {
        Message {
            id: Uuid::now_v7(),
            conversation_id: conv_id,
            role,
            content: MessageContent::Text { text: text.to_string() },
            created_at: Utc::now(),
            token_count: Some(tokens),
        }
    }

    // --- Conversation CRUD ---

    #[tokio::test]
    async fn create_and_get_conversation() {
        let pool = test_pool();
        let agent_id = AgentId::new();
        let conv = make_conversation(agent_id);
        let conv_id = conv.id;

        let returned_id = ConversationStore::create(&pool, conv)
            .await
            .expect("create conversation");
        assert_eq!(returned_id, conv_id);

        let fetched = ConversationStore::get(&pool, conv_id)
            .await
            .expect("get conversation");
        assert_eq!(fetched.id, conv_id);
        assert_eq!(fetched.agent_id, agent_id);
        assert_eq!(fetched.message_count, 0);
        assert!(fetched.title.is_none());
        assert!(fetched.metadata.is_empty());
    }

    #[tokio::test]
    async fn get_conversation_not_found() {
        let pool = test_pool();
        let err = ConversationStore::get(&pool, ConversationId::new())
            .await
            .expect_err("should not find non-existent conversation");
        assert!(matches!(err, ConversationError::NotFound(_)));
    }

    #[tokio::test]
    async fn create_conversation_with_title() {
        let pool = test_pool();
        let agent_id = AgentId::new();
        let mut conv = make_conversation(agent_id);
        conv.title = Some("My Conversation".to_string());
        let conv_id = conv.id;

        ConversationStore::create(&pool, conv).await.expect("create");

        let fetched = ConversationStore::get(&pool, conv_id)
            .await
            .expect("get");
        assert_eq!(fetched.title, Some("My Conversation".to_string()));
    }

    #[tokio::test]
    async fn create_conversation_with_metadata() {
        let pool = test_pool();
        let agent_id = AgentId::new();
        let mut conv = make_conversation(agent_id);
        conv.metadata.insert("source".to_string(), "api".to_string());
        conv.metadata.insert("version".to_string(), "2".to_string());
        let conv_id = conv.id;

        ConversationStore::create(&pool, conv).await.expect("create");

        let fetched = ConversationStore::get(&pool, conv_id)
            .await
            .expect("get");
        assert_eq!(fetched.metadata.get("source").map(String::as_str), Some("api"));
        assert_eq!(fetched.metadata.get("version").map(String::as_str), Some("2"));
    }

    // --- List conversations ---

    #[tokio::test]
    async fn list_conversations_empty() {
        let pool = test_pool();
        let agent_id = AgentId::new();
        let result = ConversationStore::list(&pool, agent_id, 10, 0)
            .await
            .expect("list empty");
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn list_conversations_filters_by_agent() {
        let pool = test_pool();
        let agent_a = AgentId::new();
        let agent_b = AgentId::new();

        ConversationStore::create(&pool, make_conversation(agent_a))
            .await
            .expect("create for agent_a");
        ConversationStore::create(&pool, make_conversation(agent_b))
            .await
            .expect("create for agent_b");

        let result = ConversationStore::list(&pool, agent_a, 10, 0)
            .await
            .expect("list agent_a");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].agent_id, agent_a);
    }

    #[tokio::test]
    async fn list_conversations_pagination() {
        let pool = test_pool();
        let agent_id = AgentId::new();

        for _ in 0..5 {
            ConversationStore::create(&pool, make_conversation(agent_id))
                .await
                .expect("create");
        }

        let page1 = ConversationStore::list(&pool, agent_id, 3, 0)
            .await
            .expect("page 1");
        assert_eq!(page1.len(), 3);

        let page2 = ConversationStore::list(&pool, agent_id, 3, 3)
            .await
            .expect("page 2");
        assert_eq!(page2.len(), 2);

        let page3 = ConversationStore::list(&pool, agent_id, 3, 6)
            .await
            .expect("page 3");
        assert!(page3.is_empty());
    }

    // --- Delete ---

    #[tokio::test]
    async fn delete_conversation() {
        let pool = test_pool();
        let agent_id = AgentId::new();
        let conv = make_conversation(agent_id);
        let conv_id = conv.id;
        ConversationStore::create(&pool, conv).await.expect("create");

        ConversationStore::delete(&pool, conv_id)
            .await
            .expect("delete");

        let err = ConversationStore::get(&pool, conv_id)
            .await
            .expect_err("should be gone");
        assert!(matches!(err, ConversationError::NotFound(_)));
    }

    #[tokio::test]
    async fn delete_not_found() {
        let pool = test_pool();
        let err = ConversationStore::delete(&pool, ConversationId::new())
            .await
            .expect_err("should fail for missing conversation");
        assert!(matches!(err, ConversationError::NotFound(_)));
    }

    #[tokio::test]
    async fn delete_cascades_to_messages() {
        let pool = test_pool();
        let agent_id = AgentId::new();
        let conv = make_conversation(agent_id);
        let conv_id = conv.id;
        ConversationStore::create(&pool, conv).await.expect("create");

        let msg = make_message(conv_id, MessageRole::User, "hello");
        ConversationStore::add_message(&pool, conv_id, msg)
            .await
            .expect("add message");

        ConversationStore::delete(&pool, conv_id)
            .await
            .expect("delete conversation");

        // Verify the message was deleted via cascade.
        let count: i64 = {
            let writer = pool.writer();
            writer
                .query_row(
                    "SELECT COUNT(*) FROM conversation_messages WHERE conversation_id = ?1",
                    [&conv_id.to_string()],
                    |r| r.get(0),
                )
                .expect("count messages")
        };
        assert_eq!(count, 0);
    }

    // --- Messages ---

    #[tokio::test]
    async fn add_and_get_message() {
        let pool = test_pool();
        let agent_id = AgentId::new();
        let conv = make_conversation(agent_id);
        let conv_id = conv.id;
        ConversationStore::create(&pool, conv).await.expect("create");

        let msg = make_message(conv_id, MessageRole::User, "hello world");
        let msg_id = msg.id;
        let returned_id = ConversationStore::add_message(&pool, conv_id, msg)
            .await
            .expect("add message");
        assert_eq!(returned_id, msg_id);

        let messages = ConversationStore::get_messages(&pool, conv_id, 10, 0)
            .await
            .expect("get messages");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].id, msg_id);
        assert_eq!(messages[0].role, MessageRole::User);
        match &messages[0].content {
            MessageContent::Text { text } => assert_eq!(text, "hello world"),
            other => panic!("unexpected content: {other:?}"),
        }
    }

    #[tokio::test]
    async fn add_message_to_nonexistent_conversation() {
        let pool = test_pool();
        let conv_id = ConversationId::new();
        let msg = make_message(conv_id, MessageRole::User, "hello");
        let err = ConversationStore::add_message(&pool, conv_id, msg)
            .await
            .expect_err("should fail for missing conversation");
        assert!(matches!(err, ConversationError::NotFound(_)));
    }

    #[tokio::test]
    async fn add_message_increments_message_count() {
        let pool = test_pool();
        let agent_id = AgentId::new();
        let conv = make_conversation(agent_id);
        let conv_id = conv.id;
        ConversationStore::create(&pool, conv).await.expect("create");

        for i in 0..3 {
            let msg = make_message(conv_id, MessageRole::User, &format!("message {i}"));
            ConversationStore::add_message(&pool, conv_id, msg)
                .await
                .expect("add");
        }

        let fetched = ConversationStore::get(&pool, conv_id)
            .await
            .expect("get");
        assert_eq!(fetched.message_count, 3);
    }

    #[tokio::test]
    async fn message_ordering_is_chronological() {
        let pool = test_pool();
        let agent_id = AgentId::new();
        let conv = make_conversation(agent_id);
        let conv_id = conv.id;
        ConversationStore::create(&pool, conv).await.expect("create");

        // Insert messages in order; using manually spaced timestamps to avoid
        // sub-millisecond collisions in in-memory tests.
        let base = Utc::now();
        for i in 0..5i64 {
            let msg = Message {
                id: Uuid::now_v7(),
                conversation_id: conv_id,
                role: MessageRole::User,
                content: MessageContent::Text { text: format!("msg {i}") },
                created_at: base + chrono::Duration::seconds(i),
                token_count: None,
            };
            ConversationStore::add_message(&pool, conv_id, msg)
                .await
                .expect("add");
        }

        let messages = ConversationStore::get_messages(&pool, conv_id, 10, 0)
            .await
            .expect("get messages");
        assert_eq!(messages.len(), 5);
        for i in 0..4 {
            assert!(
                messages[i].created_at <= messages[i + 1].created_at,
                "messages not in chronological order at index {i}"
            );
        }
    }

    #[tokio::test]
    async fn get_messages_pagination() {
        let pool = test_pool();
        let agent_id = AgentId::new();
        let conv = make_conversation(agent_id);
        let conv_id = conv.id;
        ConversationStore::create(&pool, conv).await.expect("create");

        let base = Utc::now();
        for i in 0..10i64 {
            let msg = Message {
                id: Uuid::now_v7(),
                conversation_id: conv_id,
                role: MessageRole::User,
                content: MessageContent::Text { text: format!("msg {i}") },
                created_at: base + chrono::Duration::seconds(i),
                token_count: None,
            };
            ConversationStore::add_message(&pool, conv_id, msg).await.expect("add");
        }

        let page1 = ConversationStore::get_messages(&pool, conv_id, 4, 0)
            .await
            .expect("page 1");
        assert_eq!(page1.len(), 4);

        let page2 = ConversationStore::get_messages(&pool, conv_id, 4, 4)
            .await
            .expect("page 2");
        assert_eq!(page2.len(), 4);

        let page3 = ConversationStore::get_messages(&pool, conv_id, 4, 8)
            .await
            .expect("page 3");
        assert_eq!(page3.len(), 2);
    }

    #[tokio::test]
    async fn get_messages_empty_for_unknown_conversation() {
        let pool = test_pool();
        let messages = ConversationStore::get_messages(&pool, ConversationId::new(), 10, 0)
            .await
            .expect("empty result for unknown conversation");
        assert!(messages.is_empty());
    }

    #[tokio::test]
    async fn get_recent_messages_returns_last_n() {
        let pool = test_pool();
        let agent_id = AgentId::new();
        let conv = make_conversation(agent_id);
        let conv_id = conv.id;
        ConversationStore::create(&pool, conv).await.expect("create");

        let base = Utc::now();
        for i in 0..10i64 {
            let msg = Message {
                id: Uuid::now_v7(),
                conversation_id: conv_id,
                role: MessageRole::User,
                content: MessageContent::Text { text: format!("msg {i}") },
                created_at: base + chrono::Duration::seconds(i),
                token_count: None,
            };
            ConversationStore::add_message(&pool, conv_id, msg).await.expect("add");
        }

        let recent = ConversationStore::get_recent_messages(&pool, conv_id, 3)
            .await
            .expect("get recent");
        assert_eq!(recent.len(), 3);
        // Should be in chronological order, but the last 3.
        match &recent[0].content {
            MessageContent::Text { text } => assert_eq!(text, "msg 7"),
            _ => panic!("unexpected"),
        }
        match &recent[2].content {
            MessageContent::Text { text } => assert_eq!(text, "msg 9"),
            _ => panic!("unexpected"),
        }
    }

    #[tokio::test]
    async fn get_recent_messages_fewer_than_limit() {
        let pool = test_pool();
        let agent_id = AgentId::new();
        let conv = make_conversation(agent_id);
        let conv_id = conv.id;
        ConversationStore::create(&pool, conv).await.expect("create");

        for i in 0..3i64 {
            let msg = Message {
                id: Uuid::now_v7(),
                conversation_id: conv_id,
                role: MessageRole::User,
                content: MessageContent::Text { text: format!("msg {i}") },
                created_at: Utc::now() + chrono::Duration::seconds(i),
                token_count: None,
            };
            ConversationStore::add_message(&pool, conv_id, msg).await.expect("add");
        }

        let recent = ConversationStore::get_recent_messages(&pool, conv_id, 10)
            .await
            .expect("get recent");
        assert_eq!(recent.len(), 3);
    }

    // --- Update title ---

    #[tokio::test]
    async fn update_title() {
        let pool = test_pool();
        let agent_id = AgentId::new();
        let conv = make_conversation(agent_id);
        let conv_id = conv.id;
        ConversationStore::create(&pool, conv).await.expect("create");

        ConversationStore::update_title(&pool, conv_id, "New Title".to_string())
            .await
            .expect("update title");

        let fetched = ConversationStore::get(&pool, conv_id)
            .await
            .expect("get");
        assert_eq!(fetched.title, Some("New Title".to_string()));
    }

    #[tokio::test]
    async fn update_title_not_found() {
        let pool = test_pool();
        let err = ConversationStore::update_title(&pool, ConversationId::new(), "Title".to_string())
            .await
            .expect_err("should fail for missing conversation");
        assert!(matches!(err, ConversationError::NotFound(_)));
    }

    // --- Token count tracking ---

    #[tokio::test]
    async fn message_token_count_persisted() {
        let pool = test_pool();
        let agent_id = AgentId::new();
        let conv = make_conversation(agent_id);
        let conv_id = conv.id;
        ConversationStore::create(&pool, conv).await.expect("create");

        let msg = make_message_with_tokens(conv_id, MessageRole::Assistant, "Hello!", 42);
        ConversationStore::add_message(&pool, conv_id, msg)
            .await
            .expect("add");

        let messages = ConversationStore::get_messages(&pool, conv_id, 10, 0)
            .await
            .expect("get");
        assert_eq!(messages[0].token_count, Some(42));
    }

    #[tokio::test]
    async fn message_with_no_token_count() {
        let pool = test_pool();
        let agent_id = AgentId::new();
        let conv = make_conversation(agent_id);
        let conv_id = conv.id;
        ConversationStore::create(&pool, conv).await.expect("create");

        let msg = make_message(conv_id, MessageRole::User, "hello");
        ConversationStore::add_message(&pool, conv_id, msg)
            .await
            .expect("add");

        let messages = ConversationStore::get_messages(&pool, conv_id, 10, 0)
            .await
            .expect("get");
        assert_eq!(messages[0].token_count, None);
    }

    // --- Large conversation ---

    #[tokio::test]
    async fn large_conversation_with_many_messages() {
        let pool = test_pool();
        let agent_id = AgentId::new();
        let conv = make_conversation(agent_id);
        let conv_id = conv.id;
        ConversationStore::create(&pool, conv).await.expect("create");

        let base = Utc::now();
        for i in 0..100i64 {
            let role = if i % 2 == 0 { MessageRole::User } else { MessageRole::Assistant };
            let msg = Message {
                id: Uuid::now_v7(),
                conversation_id: conv_id,
                role,
                content: MessageContent::Text { text: format!("message number {i}") },
                created_at: base + chrono::Duration::milliseconds(i),
                token_count: Some(u32::try_from(i).unwrap_or(0) + 1),
            };
            ConversationStore::add_message(&pool, conv_id, msg)
                .await
                .expect("add message");
        }

        let fetched = ConversationStore::get(&pool, conv_id)
            .await
            .expect("get conversation");
        assert_eq!(fetched.message_count, 100);

        let all = ConversationStore::get_messages(&pool, conv_id, 200, 0)
            .await
            .expect("get all messages");
        assert_eq!(all.len(), 100);

        // Verify ordering.
        for i in 0..99 {
            assert!(all[i].created_at <= all[i + 1].created_at);
        }
    }

    // --- Mixed content types ---

    #[tokio::test]
    async fn message_with_tool_call_content() {
        let pool = test_pool();
        let agent_id = AgentId::new();
        let conv = make_conversation(agent_id);
        let conv_id = conv.id;
        ConversationStore::create(&pool, conv).await.expect("create");

        let content = MessageContent::ToolCall {
            name: "file_read".to_string(),
            arguments: serde_json::json!({"path": "/tmp/test.txt"}),
        };
        let msg = Message {
            id: Uuid::now_v7(),
            conversation_id: conv_id,
            role: MessageRole::Tool,
            content,
            created_at: Utc::now(),
            token_count: Some(15),
        };
        ConversationStore::add_message(&pool, conv_id, msg)
            .await
            .expect("add tool call message");

        let messages = ConversationStore::get_messages(&pool, conv_id, 10, 0)
            .await
            .expect("get");
        assert_eq!(messages.len(), 1);
        assert!(matches!(&messages[0].content, MessageContent::ToolCall { name, .. } if name == "file_read"));
    }

    // --- Concurrent access ---

    #[tokio::test]
    async fn concurrent_message_insertion() {
        use std::sync::Arc;
        let pool = Arc::new(test_pool());
        let agent_id = AgentId::new();
        let conv = make_conversation(agent_id);
        let conv_id = conv.id;
        ConversationStore::create(pool.as_ref(), conv)
            .await
            .expect("create");

        let mut handles = Vec::new();
        let base = Utc::now();
        for i in 0i64..20 {
            let pool_clone = pool.clone();
            let handle = tokio::spawn(async move {
                let msg = Message {
                    id: Uuid::now_v7(),
                    conversation_id: conv_id,
                    role: MessageRole::User,
                    content: MessageContent::Text { text: format!("concurrent {i}") },
                    created_at: base + chrono::Duration::milliseconds(i),
                    token_count: None,
                };
                ConversationStore::add_message(pool_clone.as_ref(), conv_id, msg)
                    .await
                    .expect("concurrent add message");
            });
            handles.push(handle);
        }

        for h in handles {
            h.await.expect("task completed");
        }

        let fetched = ConversationStore::get(pool.as_ref(), conv_id)
            .await
            .expect("get conversation");
        assert_eq!(fetched.message_count, 20);
    }

    // --- All message roles ---

    #[tokio::test]
    async fn all_message_roles_round_trip() {
        let pool = test_pool();
        let agent_id = AgentId::new();
        let conv = make_conversation(agent_id);
        let conv_id = conv.id;
        ConversationStore::create(&pool, conv).await.expect("create");

        let base = Utc::now();
        for (i, role) in [MessageRole::User, MessageRole::Assistant, MessageRole::System, MessageRole::Tool]
            .iter()
            .enumerate()
        {
            let msg = Message {
                id: Uuid::now_v7(),
                conversation_id: conv_id,
                role: *role,
                content: MessageContent::Text { text: format!("{role:?} message") },
                created_at: base + chrono::Duration::seconds(i as i64),
                token_count: None,
            };
            ConversationStore::add_message(&pool, conv_id, msg)
                .await
                .expect("add");
        }

        let messages = ConversationStore::get_messages(&pool, conv_id, 10, 0)
            .await
            .expect("get");
        assert_eq!(messages.len(), 4);
        assert_eq!(messages[0].role, MessageRole::User);
        assert_eq!(messages[1].role, MessageRole::Assistant);
        assert_eq!(messages[2].role, MessageRole::System);
        assert_eq!(messages[3].role, MessageRole::Tool);
    }
}
