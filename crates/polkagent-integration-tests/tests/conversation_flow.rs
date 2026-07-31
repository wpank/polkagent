//! Conversation flow integration tests.
//!
//! Exercises InMemoryConversationStore (create / add messages / ordering),
//! ContextWindow (truncation, token counting, system-message eviction),
//! and the full conversation lifecycle.

use chrono::Utc;
use uuid::Uuid;

use polkagent_conversation::context::{ContextWindow, InferenceContentBlock, InferenceMessageRole};
use polkagent_conversation::memory::InMemoryConversationStore;
use polkagent_conversation::store::ConversationStore;
use polkagent_conversation::types::{
    Conversation, ConversationSummary, Message, MessageContent, MessageRole,
};
use polkagent_core::ids::{AgentId, ConversationId};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_text_message(conv_id: ConversationId, role: MessageRole, text: &str) -> Message {
    Message {
        id: Uuid::now_v7(),
        conversation_id: conv_id,
        role,
        content: MessageContent::Text { text: text.into() },
        created_at: Utc::now(),
        token_count: None,
    }
}

fn make_message_with_tokens(
    conv_id: ConversationId,
    role: MessageRole,
    text: &str,
    tokens: u32,
) -> Message {
    Message {
        id: Uuid::now_v7(),
        conversation_id: conv_id,
        role,
        content: MessageContent::Text { text: text.into() },
        created_at: Utc::now(),
        token_count: Some(tokens),
    }
}

fn context_text_msg(role: MessageRole, text: &str) -> Message {
    make_text_message(ConversationId::new(), role, text)
}

fn context_msg_with_tokens(role: MessageRole, text: &str, tokens: u32) -> Message {
    make_message_with_tokens(ConversationId::new(), role, text, tokens)
}

// ---------------------------------------------------------------------------
// Conversation create / get
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_conversation_and_retrieve_it() {
    let store = InMemoryConversationStore::new();
    let agent_id = AgentId::new();
    let conv = Conversation::new(ConversationId::new(), agent_id);

    let id = store.create(conv.clone()).await.expect("create");
    let fetched = store.get(id).await.expect("get");

    assert_eq!(fetched.id, conv.id);
    assert_eq!(fetched.agent_id, agent_id);
    assert!(fetched.title.is_none());
    assert_eq!(fetched.message_count, 0);
}

#[tokio::test]
async fn get_nonexistent_conversation_returns_not_found() {
    let store = InMemoryConversationStore::new();
    let result = store.get(ConversationId::new()).await;
    assert!(result.is_err(), "getting nonexistent conversation must fail");
}

#[tokio::test]
async fn delete_conversation_removes_it() {
    let store = InMemoryConversationStore::new();
    let conv = Conversation::new(ConversationId::new(), AgentId::new());
    let id = store.create(conv).await.expect("create");

    store.delete(id).await.expect("delete");

    let result = store.get(id).await;
    assert!(result.is_err(), "deleted conversation must not be retrievable");
}

#[tokio::test]
async fn delete_nonexistent_conversation_returns_error() {
    let store = InMemoryConversationStore::new();
    let result = store.delete(ConversationId::new()).await;
    assert!(result.is_err(), "deleting nonexistent conversation must fail");
}

#[tokio::test]
async fn update_title_modifies_stored_title() {
    let store = InMemoryConversationStore::new();
    let conv = Conversation::new(ConversationId::new(), AgentId::new());
    let id = store.create(conv).await.expect("create");

    store
        .update_title(id, "My Conversation".into())
        .await
        .expect("update_title");

    let fetched = store.get(id).await.expect("get");
    assert_eq!(fetched.title, Some("My Conversation".into()));
}

#[tokio::test]
async fn update_title_on_nonexistent_conversation_fails() {
    let store = InMemoryConversationStore::new();
    let result = store.update_title(ConversationId::new(), "title".into()).await;
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// Messages — add / order
// ---------------------------------------------------------------------------

#[tokio::test]
async fn add_messages_and_retrieve_in_insertion_order() {
    let store = InMemoryConversationStore::new();
    let conv = Conversation::new(ConversationId::new(), AgentId::new());
    let conv_id = store.create(conv).await.expect("create");

    let m1 = make_text_message(conv_id, MessageRole::User, "Hello");
    let m2 = make_text_message(conv_id, MessageRole::Assistant, "Hi there");
    let m3 = make_text_message(conv_id, MessageRole::User, "What can you do?");

    store.add_message(conv_id, m1.clone()).await.expect("add m1");
    store.add_message(conv_id, m2.clone()).await.expect("add m2");
    store.add_message(conv_id, m3.clone()).await.expect("add m3");

    let messages = store.get_messages(conv_id, 10, 0).await.expect("get_messages");
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0].id, m1.id);
    assert_eq!(messages[1].id, m2.id);
    assert_eq!(messages[2].id, m3.id);
}

#[tokio::test]
async fn message_count_updates_after_each_add() {
    let store = InMemoryConversationStore::new();
    let conv = Conversation::new(ConversationId::new(), AgentId::new());
    let conv_id = store.create(conv).await.expect("create");

    for i in 1..=5u32 {
        let msg = make_text_message(conv_id, MessageRole::User, &format!("msg {i}"));
        store.add_message(conv_id, msg).await.expect("add");

        let fetched = store.get(conv_id).await.expect("get");
        assert_eq!(fetched.message_count, i, "message_count must equal {i} after adding {i} messages");
    }
}

#[tokio::test]
async fn add_message_to_nonexistent_conversation_fails() {
    let store = InMemoryConversationStore::new();
    let conv_id = ConversationId::new();
    let msg = make_text_message(conv_id, MessageRole::User, "orphan");
    let result = store.add_message(conv_id, msg).await;
    assert!(result.is_err(), "adding message to nonexistent conversation must fail");
}

#[tokio::test]
async fn get_messages_pagination_returns_correct_window() {
    let store = InMemoryConversationStore::new();
    let conv = Conversation::new(ConversationId::new(), AgentId::new());
    let conv_id = store.create(conv).await.expect("create");

    for i in 0..10 {
        let msg = make_text_message(conv_id, MessageRole::User, &format!("msg {i}"));
        store.add_message(conv_id, msg).await.expect("add");
    }

    // Get 3 messages starting from offset 2.
    let page = store.get_messages(conv_id, 3, 2).await.expect("paginate");
    assert_eq!(page.len(), 3, "must return exactly 3 messages");

    if let MessageContent::Text { text } = &page[0].content {
        assert_eq!(text, "msg 2", "first page item must be at offset 2");
    } else {
        panic!("expected text content");
    }
}

#[tokio::test]
async fn get_recent_messages_returns_last_n() {
    let store = InMemoryConversationStore::new();
    let conv = Conversation::new(ConversationId::new(), AgentId::new());
    let conv_id = store.create(conv).await.expect("create");

    for i in 0..10 {
        let msg = make_text_message(conv_id, MessageRole::User, &format!("msg {i}"));
        store.add_message(conv_id, msg).await.expect("add");
    }

    let recent = store.get_recent_messages(conv_id, 3).await.expect("recent");
    assert_eq!(recent.len(), 3, "must return 3 most recent messages");

    if let MessageContent::Text { text } = &recent[0].content {
        assert_eq!(text, "msg 7", "first recent must be msg 7 (8th message, 0-indexed)");
    } else {
        panic!("expected text content");
    }
}

#[tokio::test]
async fn get_recent_messages_on_nonexistent_conversation_fails() {
    let store = InMemoryConversationStore::new();
    let result = store.get_recent_messages(ConversationId::new(), 5).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn delete_conversation_also_removes_its_messages() {
    let store = InMemoryConversationStore::new();
    let conv = Conversation::new(ConversationId::new(), AgentId::new());
    let conv_id = store.create(conv).await.expect("create");

    store
        .add_message(conv_id, make_text_message(conv_id, MessageRole::User, "hello"))
        .await
        .expect("add");

    store.delete(conv_id).await.expect("delete");

    // Messages must be gone too.
    let result = store.get_messages(conv_id, 10, 0).await;
    assert!(result.is_err(), "messages must be removed when conversation is deleted");
}

// ---------------------------------------------------------------------------
// List conversations
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_returns_only_conversations_for_given_agent() {
    let store = InMemoryConversationStore::new();
    let agent_a = AgentId::new();
    let agent_b = AgentId::new();

    for _ in 0..3 {
        let conv = Conversation::new(ConversationId::new(), agent_a);
        store.create(conv).await.expect("create");
    }
    let conv_b = Conversation::new(ConversationId::new(), agent_b);
    store.create(conv_b).await.expect("create b");

    let summaries = store.list(agent_a, 10, 0).await.expect("list");
    assert_eq!(summaries.len(), 3, "must return only agent_a's conversations");

    for s in &summaries {
        assert_eq!(s.agent_id, agent_a);
    }
}

#[tokio::test]
async fn list_with_pagination_limits_results() {
    let store = InMemoryConversationStore::new();
    let agent = AgentId::new();

    for _ in 0..5 {
        let conv = Conversation::new(ConversationId::new(), agent);
        store.create(conv).await.expect("create");
    }

    let page = store.list(agent, 2, 0).await.expect("list");
    assert_eq!(page.len(), 2, "limit must restrict to 2 results");
}

#[tokio::test]
async fn list_with_offset_skips_results() {
    let store = InMemoryConversationStore::new();
    let agent = AgentId::new();

    for _ in 0..5 {
        let conv = Conversation::new(ConversationId::new(), agent);
        store.create(conv).await.expect("create");
    }

    let page = store.list(agent, 10, 3).await.expect("list offset");
    assert_eq!(page.len(), 2, "offset 3 of 5 must return 2 remaining results");
}

#[tokio::test]
async fn list_returns_conversation_summaries_with_correct_fields() {
    let store = InMemoryConversationStore::new();
    let agent = AgentId::new();
    let conv_id = ConversationId::new();
    let conv = Conversation::new(conv_id, agent);
    store.create(conv).await.expect("create");

    let msg = make_text_message(conv_id, MessageRole::User, "hello");
    store.add_message(conv_id, msg).await.expect("add");

    let summaries = store.list(agent, 10, 0).await.expect("list");
    assert_eq!(summaries.len(), 1);

    let s = &summaries[0];
    assert_eq!(s.id, conv_id);
    assert_eq!(s.agent_id, agent);
    assert_eq!(s.message_count, 1);
    assert!(s.last_message_at.is_some(), "summary must have last_message_at after a message is added");
}

// ---------------------------------------------------------------------------
// Clone shares state
// ---------------------------------------------------------------------------

#[tokio::test]
async fn clone_of_store_shares_underlying_data() {
    let store = InMemoryConversationStore::new();
    let store2 = store.clone();

    let conv = Conversation::new(ConversationId::new(), AgentId::new());
    let id = store.create(conv).await.expect("create");

    let fetched = store2.get(id).await.expect("get from clone");
    assert_eq!(fetched.id, id, "clone must see data created in original");
}

// ---------------------------------------------------------------------------
// ContextWindow — basic operation
// ---------------------------------------------------------------------------

#[test]
fn new_context_window_is_empty() {
    let cw = ContextWindow::new(1000);
    assert_eq!(cw.message_count(), 0);
    assert_eq!(cw.current_tokens(), 0);
    assert_eq!(cw.remaining_tokens(), 1000);
    assert_eq!(cw.max_tokens(), 1000);
}

#[test]
fn add_message_updates_token_count() {
    let mut cw = ContextWindow::new(1000);
    let msg = context_msg_with_tokens(MessageRole::User, "hello", 25);
    cw.add_message(msg);

    assert_eq!(cw.message_count(), 1);
    assert_eq!(cw.current_tokens(), 25);
    assert_eq!(cw.remaining_tokens(), 975);
}

#[test]
fn add_message_with_estimated_tokens_uses_heuristic() {
    let mut cw = ContextWindow::new(1000);
    // "hello world" = 11 chars => ceil(11/4) = 3 tokens
    let msg = context_text_msg(MessageRole::User, "hello world");
    cw.add_message(msg);

    assert_eq!(cw.current_tokens(), 3);
}

// ---------------------------------------------------------------------------
// ContextWindow — token budget eviction
// ---------------------------------------------------------------------------

#[test]
fn context_window_evicts_oldest_when_over_budget() {
    let mut cw = ContextWindow::new(100);
    cw.add_message(context_msg_with_tokens(MessageRole::User, "first", 40));
    cw.add_message(context_msg_with_tokens(MessageRole::Assistant, "second", 40));
    assert_eq!(cw.message_count(), 2);

    // Adding third message pushes total to 120 > 100; first must be evicted.
    cw.add_message(context_msg_with_tokens(MessageRole::User, "third", 40));
    assert_eq!(cw.message_count(), 2, "eviction must bring count back to 2");
    assert!(cw.current_tokens() <= 100, "total tokens must fit within budget after eviction");
}

#[test]
fn context_window_system_messages_evicted_last() {
    let mut cw = ContextWindow::new(100);
    cw.add_message(context_msg_with_tokens(MessageRole::System, "system", 30));
    cw.add_message(context_msg_with_tokens(MessageRole::User, "user msg", 30));
    cw.add_message(context_msg_with_tokens(MessageRole::Assistant, "asst msg", 30));

    // Adding a fourth message (30 tokens) pushes to 120; a non-system must be evicted.
    cw.add_message(context_msg_with_tokens(MessageRole::User, "new msg", 30));

    let roles: Vec<MessageRole> = cw.messages().iter().map(|m| m.role).collect();
    assert!(
        roles.contains(&MessageRole::System),
        "system message must survive eviction when non-system messages can be evicted instead"
    );
    assert_eq!(cw.message_count(), 3, "must have 3 messages after eviction");
}

#[test]
fn context_window_single_message_larger_than_budget_is_kept() {
    let mut cw = ContextWindow::new(10);
    // A message larger than the entire budget should still be kept (min 1 message).
    cw.add_message(context_msg_with_tokens(MessageRole::User, "huge", 500));
    assert_eq!(cw.message_count(), 1, "oversized single message must be kept");
    assert_eq!(cw.current_tokens(), 500);
}

#[test]
fn truncate_to_fit_evicts_until_within_budget() {
    let mut cw = ContextWindow::new(1000);
    for i in 0..10 {
        cw.add_message(context_msg_with_tokens(
            MessageRole::User,
            &format!("msg {i}"),
            20,
        ));
    }
    assert_eq!(cw.current_tokens(), 200);

    cw.truncate_to_fit(60);
    assert!(cw.current_tokens() <= 60, "must truncate to fit new budget");
    assert_eq!(cw.max_tokens(), 60, "max_tokens must be updated");
}

#[test]
fn remaining_tokens_does_not_underflow() {
    let mut cw = ContextWindow::new(10);
    cw.add_message(context_msg_with_tokens(MessageRole::User, "big", 100));
    // remaining_tokens uses saturating_sub.
    assert_eq!(cw.remaining_tokens(), 0, "remaining tokens must be 0 when over budget (not negative)");
}

// ---------------------------------------------------------------------------
// ContextWindow — to_inference_messages
// ---------------------------------------------------------------------------

#[test]
fn to_inference_messages_maps_user_role() {
    let mut cw = ContextWindow::new(1000);
    cw.add_message(context_text_msg(MessageRole::User, "hello"));

    let inferred = cw.to_inference_messages();
    assert_eq!(inferred.len(), 1);
    assert_eq!(inferred[0].role, InferenceMessageRole::User);
}

#[test]
fn to_inference_messages_maps_assistant_role() {
    let mut cw = ContextWindow::new(1000);
    cw.add_message(context_text_msg(MessageRole::Assistant, "hi"));

    let inferred = cw.to_inference_messages();
    assert_eq!(inferred[0].role, InferenceMessageRole::Assistant);
}

#[test]
fn to_inference_messages_maps_system_and_tool_to_user() {
    let mut cw = ContextWindow::new(1000);
    cw.add_message(context_text_msg(MessageRole::System, "system context"));
    cw.add_message(context_text_msg(MessageRole::Tool, "tool result"));

    let inferred = cw.to_inference_messages();
    // Both System and Tool map to User.
    assert_eq!(inferred[0].role, InferenceMessageRole::User);
    assert_eq!(inferred[1].role, InferenceMessageRole::User);
}

#[test]
fn to_inference_messages_converts_text_content() {
    let mut cw = ContextWindow::new(1000);
    cw.add_message(context_text_msg(MessageRole::User, "test message"));

    let inferred = cw.to_inference_messages();
    assert_eq!(inferred.len(), 1);
    assert_eq!(
        inferred[0].content,
        vec![InferenceContentBlock::Text {
            text: "test message".into()
        }]
    );
}

#[test]
fn to_inference_messages_converts_tool_call_content() {
    let mut cw = ContextWindow::new(1000);
    let msg = Message {
        id: Uuid::now_v7(),
        conversation_id: ConversationId::new(),
        role: MessageRole::Assistant,
        content: MessageContent::ToolCall {
            name: "my.tool".into(),
            arguments: serde_json::json!({"key": "value"}),
        },
        created_at: Utc::now(),
        token_count: None,
    };
    let msg_id = msg.id.to_string();
    cw.add_message(msg);

    let inferred = cw.to_inference_messages();
    assert_eq!(inferred.len(), 1);
    match &inferred[0].content[0] {
        InferenceContentBlock::ToolUse { tool_call_id, tool_name, .. } => {
            assert_eq!(tool_call_id, &msg_id);
            assert_eq!(tool_name, "my.tool");
        }
        other => panic!("expected ToolUse, got {other:?}"),
    }
}

#[test]
fn to_inference_messages_converts_tool_result_content() {
    let mut cw = ContextWindow::new(1000);
    let msg = Message {
        id: Uuid::now_v7(),
        conversation_id: ConversationId::new(),
        role: MessageRole::Tool,
        content: MessageContent::ToolResult {
            tool_call_id: "call-xyz".into(),
            output: serde_json::json!("result text"),
        },
        created_at: Utc::now(),
        token_count: None,
    };
    cw.add_message(msg);

    let inferred = cw.to_inference_messages();
    match &inferred[0].content[0] {
        InferenceContentBlock::ToolResult { tool_call_id, content, is_error } => {
            assert_eq!(tool_call_id, "call-xyz");
            assert!(content.contains("result text"));
            assert!(!is_error);
        }
        other => panic!("expected ToolResult, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// ConversationSummary projection
// ---------------------------------------------------------------------------

#[test]
fn conversation_summary_has_no_last_message_when_empty() {
    let conv = Conversation::new(ConversationId::new(), AgentId::new());
    let summary = ConversationSummary::from(&conv);
    assert!(
        summary.last_message_at.is_none(),
        "new conversation must have no last_message_at"
    );
}

#[test]
fn conversation_summary_has_last_message_after_message_added() {
    let mut conv = Conversation::new(ConversationId::new(), AgentId::new());
    conv.message_count = 1;
    conv.updated_at = Utc::now();

    let summary = ConversationSummary::from(&conv);
    assert!(
        summary.last_message_at.is_some(),
        "conversation with messages must have last_message_at"
    );
}
