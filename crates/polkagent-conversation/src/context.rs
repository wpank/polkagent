//! Context window management for conversations.
//!
//! [`ContextWindow`] maintains a sliding window of messages that fits within a
//! model's token budget. When new messages are added and the budget is
//! exceeded, the oldest messages are evicted automatically.
//!
//! Token counting uses a simple heuristic (4 characters per token) that is
//! good enough for budget enforcement. Exact counts can be provided via
//! [`Message::token_count`](crate::types::Message::token_count).

use tracing::debug;

use crate::types::{ContentPart, Message, MessageContent, MessageRole};

// ---------------------------------------------------------------------------
// ContextWindow
// ---------------------------------------------------------------------------

/// A token-budget-aware sliding window of conversation messages.
///
/// Messages are stored in chronological order. When the token budget is
/// exceeded, the oldest non-system messages are evicted first. System
/// messages are evicted only as a last resort.
#[derive(Debug, Clone)]
pub struct ContextWindow {
    /// Maximum token budget for the context window.
    max_tokens: u32,
    /// Messages currently in the window, in chronological order.
    messages: Vec<Message>,
    /// Running total of estimated tokens across all messages.
    current_tokens: u32,
}

impl ContextWindow {
    /// Create a new context window with the given token budget.
    #[must_use]
    pub fn new(max_tokens: u32) -> Self {
        Self {
            max_tokens,
            messages: Vec::new(),
            current_tokens: 0,
        }
    }

    /// Add a message to the context window.
    ///
    /// If adding the message would exceed the token budget, the oldest
    /// non-system messages are evicted first. If the single message is
    /// larger than the entire budget, it is still added (after clearing
    /// all other messages).
    pub fn add_message(&mut self, msg: Message) {
        let msg_tokens = msg.estimated_tokens();
        self.messages.push(msg);
        self.current_tokens += msg_tokens;
        self.evict_if_over_budget();
    }

    /// Return the messages currently in the context window.
    #[must_use]
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// Convert the context window into a list of inference messages suitable
    /// for use with the executor trait's `InferenceMessage` type.
    ///
    /// Returns `(role_str, content_text)` tuples that can be mapped into
    /// the provider-specific format by the caller.
    #[must_use]
    pub fn to_inference_messages(&self) -> Vec<InferenceMessage> {
        self.messages
            .iter()
            .map(|msg| {
                let role = match msg.role {
                    MessageRole::Assistant => InferenceMessageRole::Assistant,
                    // System and Tool messages are mapped to User role for
                    // compatibility with provider APIs that only support
                    // user/assistant.
                    MessageRole::User | MessageRole::System | MessageRole::Tool => {
                        InferenceMessageRole::User
                    }
                };
                let content = match &msg.content {
                    MessageContent::Text { text } => {
                        vec![InferenceContentBlock::Text { text: text.clone() }]
                    }
                    MessageContent::ToolCall { name, arguments } => {
                        vec![InferenceContentBlock::ToolUse {
                            tool_call_id: msg.id.to_string(),
                            tool_name: name.clone(),
                            arguments_json: arguments.to_string(),
                        }]
                    }
                    MessageContent::ToolResult {
                        tool_call_id,
                        output,
                    } => {
                        vec![InferenceContentBlock::ToolResult {
                            tool_call_id: tool_call_id.clone(),
                            content: output.to_string(),
                            is_error: false,
                        }]
                    }
                    MessageContent::Mixed { parts } => parts
                        .iter()
                        .map(|part| match part {
                            ContentPart::Text { text } => {
                                InferenceContentBlock::Text { text: text.clone() }
                            }
                            ContentPart::ToolUse { id, name, input } => {
                                InferenceContentBlock::ToolUse {
                                    tool_call_id: id.clone(),
                                    tool_name: name.clone(),
                                    arguments_json: input.to_string(),
                                }
                            }
                            ContentPart::ToolResult { id, content } => {
                                InferenceContentBlock::ToolResult {
                                    tool_call_id: id.clone(),
                                    content: content.to_string(),
                                    is_error: false,
                                }
                            }
                        })
                        .collect(),
                };
                InferenceMessage { role, content }
            })
            .collect()
    }

    /// Return the number of tokens remaining before the budget is exhausted.
    #[must_use]
    pub fn remaining_tokens(&self) -> u32 {
        self.max_tokens.saturating_sub(self.current_tokens)
    }

    /// Return the current estimated token count.
    #[must_use]
    pub fn current_tokens(&self) -> u32 {
        self.current_tokens
    }

    /// Return the maximum token budget.
    #[must_use]
    pub fn max_tokens(&self) -> u32 {
        self.max_tokens
    }

    /// Truncate the oldest messages until the total token count fits within
    /// the given budget.
    ///
    /// System messages are preserved as long as possible; non-system messages
    /// are evicted first.
    pub fn truncate_to_fit(&mut self, max_tokens: u32) {
        self.max_tokens = max_tokens;
        self.evict_if_over_budget();
    }

    /// Return the number of messages currently in the window.
    #[must_use]
    pub fn message_count(&self) -> usize {
        self.messages.len()
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    /// Evict the oldest non-system messages until the budget is met.
    fn evict_if_over_budget(&mut self) {
        while self.current_tokens > self.max_tokens && self.messages.len() > 1 {
            // Find the first non-system message to evict.
            let evict_idx = self
                .messages
                .iter()
                .position(|m| m.role != MessageRole::System)
                .unwrap_or(0); // fall back to evicting system messages

            let evicted = self.messages.remove(evict_idx);
            let evicted_tokens = evicted.estimated_tokens();
            self.current_tokens = self.current_tokens.saturating_sub(evicted_tokens);
            debug!(
                evicted_tokens,
                remaining_tokens = self.current_tokens,
                max_tokens = self.max_tokens,
                "evicted message from context window"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Inference message types (lightweight, no executor-trait dependency)
// ---------------------------------------------------------------------------

/// Role for an inference message.
///
/// This is a local type mirroring the executor trait's `MessageRole` to avoid
/// coupling the conversation crate to the executor crate. Callers convert
/// these to the executor's types at the boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InferenceMessageRole {
    /// A human or ingress-layer message.
    User,
    /// A model-generated message.
    Assistant,
}

/// A content block within an inference message.
///
/// Mirrors the executor trait's `ContentBlock` so the conversation crate does
/// not depend on `polkagent-executor-trait`.
#[derive(Debug, Clone, PartialEq)]
pub enum InferenceContentBlock {
    /// Plain text content.
    Text {
        /// The text value.
        text: String,
    },
    /// A tool result returned to the model.
    ToolResult {
        /// The tool call this result answers.
        tool_call_id: String,
        /// Serialized result content.
        content: String,
        /// Whether the tool invocation produced an error.
        is_error: bool,
    },
    /// A tool use request from the model.
    ToolUse {
        /// Unique identifier for this tool call.
        tool_call_id: String,
        /// The tool that was called.
        tool_name: String,
        /// JSON-serialized arguments.
        arguments_json: String,
    },
}

/// A message ready for inference, produced by [`ContextWindow::to_inference_messages`].
#[derive(Debug, Clone, PartialEq)]
pub struct InferenceMessage {
    /// The sender role.
    pub role: InferenceMessageRole,
    /// Content blocks.
    pub content: Vec<InferenceContentBlock>,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use polkagent_core::ids::ConversationId;
    use uuid::Uuid;

    fn text_message(role: MessageRole, text: &str) -> Message {
        Message {
            id: Uuid::now_v7(),
            conversation_id: ConversationId::new(),
            role,
            content: MessageContent::Text { text: text.into() },
            created_at: Utc::now(),
            token_count: None,
        }
    }

    fn text_message_with_tokens(role: MessageRole, text: &str, tokens: u32) -> Message {
        Message {
            id: Uuid::now_v7(),
            conversation_id: ConversationId::new(),
            role,
            content: MessageContent::Text { text: text.into() },
            created_at: Utc::now(),
            token_count: Some(tokens),
        }
    }

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
        // "Hello world" = 11 chars => ceil(11/4) = 3 tokens
        let msg = text_message(MessageRole::User, "Hello world");
        cw.add_message(msg);
        assert_eq!(cw.message_count(), 1);
        assert_eq!(cw.current_tokens(), 3);
        assert_eq!(cw.remaining_tokens(), 997);
    }

    #[test]
    fn add_message_with_explicit_token_count() {
        let mut cw = ContextWindow::new(1000);
        let msg = text_message_with_tokens(MessageRole::User, "Hello", 50);
        cw.add_message(msg);
        assert_eq!(cw.current_tokens(), 50);
    }

    #[test]
    fn evicts_oldest_when_over_budget() {
        let mut cw = ContextWindow::new(100);
        cw.add_message(text_message_with_tokens(MessageRole::User, "First", 40));
        cw.add_message(text_message_with_tokens(
            MessageRole::Assistant,
            "Second",
            40,
        ));
        assert_eq!(cw.message_count(), 2);
        assert_eq!(cw.current_tokens(), 80);

        // This should push us over and evict the first message.
        cw.add_message(text_message_with_tokens(MessageRole::User, "Third", 40));
        assert_eq!(cw.message_count(), 2);
        assert_eq!(cw.current_tokens(), 80);
    }

    #[test]
    fn system_messages_evicted_last() {
        let mut cw = ContextWindow::new(100);
        cw.add_message(text_message_with_tokens(
            MessageRole::System,
            "System prompt",
            30,
        ));
        cw.add_message(text_message_with_tokens(MessageRole::User, "User msg", 30));
        cw.add_message(text_message_with_tokens(
            MessageRole::Assistant,
            "Asst msg",
            30,
        ));

        // Adding a message that pushes over budget should evict User first.
        cw.add_message(text_message_with_tokens(MessageRole::User, "New msg", 30));

        // System message should still be present.
        let roles: Vec<MessageRole> = cw.messages().iter().map(|m| m.role).collect();
        assert!(roles.contains(&MessageRole::System));
        assert_eq!(cw.message_count(), 3);
    }

    #[test]
    fn truncate_to_fit_reduces_messages() {
        let mut cw = ContextWindow::new(1000);
        for i in 0..10 {
            cw.add_message(text_message_with_tokens(
                MessageRole::User,
                &format!("Msg {i}"),
                20,
            ));
        }
        assert_eq!(cw.current_tokens(), 200);

        cw.truncate_to_fit(60);
        assert!(cw.current_tokens() <= 60);
        assert_eq!(cw.max_tokens(), 60);
    }

    #[test]
    fn single_message_larger_than_budget_is_kept() {
        let mut cw = ContextWindow::new(10);
        cw.add_message(text_message_with_tokens(MessageRole::User, "Huge", 50));
        // Even though it exceeds the budget, we keep it (last message).
        assert_eq!(cw.message_count(), 1);
        assert_eq!(cw.current_tokens(), 50);
    }

    #[test]
    fn to_inference_messages_maps_roles() {
        let mut cw = ContextWindow::new(1000);
        cw.add_message(text_message(MessageRole::User, "Hello"));
        cw.add_message(text_message(MessageRole::Assistant, "Hi"));
        cw.add_message(text_message(MessageRole::System, "Context"));
        cw.add_message(text_message(MessageRole::Tool, "Result"));

        let inferred = cw.to_inference_messages();
        assert_eq!(inferred.len(), 4);
        assert_eq!(inferred[0].role, InferenceMessageRole::User);
        assert_eq!(inferred[1].role, InferenceMessageRole::Assistant);
        // System and Tool are mapped to User.
        assert_eq!(inferred[2].role, InferenceMessageRole::User);
        assert_eq!(inferred[3].role, InferenceMessageRole::User);
    }

    #[test]
    fn to_inference_messages_converts_text() {
        let mut cw = ContextWindow::new(1000);
        cw.add_message(text_message(MessageRole::User, "Hello world"));
        let inferred = cw.to_inference_messages();
        assert_eq!(inferred.len(), 1);
        assert_eq!(
            inferred[0].content,
            vec![InferenceContentBlock::Text {
                text: "Hello world".into()
            }]
        );
    }

    #[test]
    fn to_inference_messages_converts_tool_call() {
        let mut cw = ContextWindow::new(1000);
        let msg = Message {
            id: Uuid::now_v7(),
            conversation_id: ConversationId::new(),
            role: MessageRole::Assistant,
            content: MessageContent::ToolCall {
                name: "file_read".into(),
                arguments: serde_json::json!({"path": "/tmp/test"}),
            },
            created_at: Utc::now(),
            token_count: None,
        };
        let msg_id = msg.id.to_string();
        cw.add_message(msg);

        let inferred = cw.to_inference_messages();
        assert_eq!(inferred.len(), 1);
        match &inferred[0].content[0] {
            InferenceContentBlock::ToolUse {
                tool_call_id,
                tool_name,
                ..
            } => {
                assert_eq!(tool_call_id, &msg_id);
                assert_eq!(tool_name, "file_read");
            }
            other => panic!("expected ToolUse, got {other:?}"),
        }
    }

    #[test]
    fn to_inference_messages_converts_tool_result() {
        let mut cw = ContextWindow::new(1000);
        let msg = Message {
            id: Uuid::now_v7(),
            conversation_id: ConversationId::new(),
            role: MessageRole::Tool,
            content: MessageContent::ToolResult {
                tool_call_id: "call-abc".into(),
                output: serde_json::json!("file contents"),
            },
            created_at: Utc::now(),
            token_count: None,
        };
        cw.add_message(msg);

        let inferred = cw.to_inference_messages();
        match &inferred[0].content[0] {
            InferenceContentBlock::ToolResult {
                tool_call_id,
                content,
                is_error,
            } => {
                assert_eq!(tool_call_id, "call-abc");
                assert!(content.contains("file contents"));
                assert!(!is_error);
            }
            other => panic!("expected ToolResult, got {other:?}"),
        }
    }

    #[test]
    fn to_inference_messages_converts_mixed() {
        let mut cw = ContextWindow::new(1000);
        let msg = Message {
            id: Uuid::now_v7(),
            conversation_id: ConversationId::new(),
            role: MessageRole::Assistant,
            content: MessageContent::Mixed {
                parts: vec![
                    ContentPart::Text {
                        text: "Let me check.".into(),
                    },
                    ContentPart::ToolUse {
                        id: "tu-1".into(),
                        name: "search".into(),
                        input: serde_json::json!({"query": "test"}),
                    },
                ],
            },
            created_at: Utc::now(),
            token_count: None,
        };
        cw.add_message(msg);

        let inferred = cw.to_inference_messages();
        assert_eq!(inferred[0].content.len(), 2);
    }

    #[test]
    fn remaining_tokens_never_underflows() {
        let mut cw = ContextWindow::new(10);
        cw.add_message(text_message_with_tokens(MessageRole::User, "Big", 100));
        // remaining_tokens uses saturating_sub so should be 0, not underflow.
        assert_eq!(cw.remaining_tokens(), 0);
    }

    #[test]
    fn messages_accessor_returns_slice() {
        let mut cw = ContextWindow::new(1000);
        cw.add_message(text_message(MessageRole::User, "Hi"));
        let msgs = cw.messages();
        assert_eq!(msgs.len(), 1);
    }
}
