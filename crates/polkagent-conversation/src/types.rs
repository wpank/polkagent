//! Core domain types for conversations and messages.
//!
//! A [`Conversation`] groups an ordered sequence of [`Message`]s between an
//! agent and its interlocutors (users, tools, system). Each message carries
//! typed [`MessageContent`] that can represent plain text, tool calls, tool
//! results, or mixed multi-part content.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use polkagent_core::ids::{AgentId, ConversationId};

// ---------------------------------------------------------------------------
// MessageRole
// ---------------------------------------------------------------------------

/// The sender role for a message in a conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    /// A message originating from the human user or external trigger.
    User,
    /// A message produced by the AI model.
    Assistant,
    /// An internal system message (instructions, context injection, etc.).
    System,
    /// A tool invocation or result message.
    Tool,
}

// ---------------------------------------------------------------------------
// ContentPart
// ---------------------------------------------------------------------------

/// A single block within a [`MessageContent::Mixed`] message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentPart {
    /// Plain text content.
    Text {
        /// The text value.
        text: String,
    },
    /// A tool use request.
    ToolUse {
        /// Unique identifier for this tool call.
        id: String,
        /// Name of the tool being invoked.
        name: String,
        /// JSON-encoded input arguments.
        input: Value,
    },
    /// A tool result returned after invocation.
    ToolResult {
        /// The tool call this result answers.
        id: String,
        /// JSON-encoded output content.
        content: Value,
    },
}

// ---------------------------------------------------------------------------
// MessageContent
// ---------------------------------------------------------------------------

/// The typed content of a [`Message`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MessageContent {
    /// Plain text message.
    Text {
        /// The text value.
        text: String,
    },
    /// A tool call request from the model.
    ToolCall {
        /// Name of the tool to invoke.
        name: String,
        /// JSON-encoded arguments.
        arguments: Value,
    },
    /// A tool result returned to the model.
    ToolResult {
        /// The tool call this result answers.
        tool_call_id: String,
        /// JSON-encoded output.
        output: Value,
    },
    /// A message composed of multiple content parts.
    Mixed {
        /// The content parts.
        parts: Vec<ContentPart>,
    },
}

// ---------------------------------------------------------------------------
// Message
// ---------------------------------------------------------------------------

/// A single message within a [`Conversation`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    /// Unique identifier for this message.
    pub id: Uuid,
    /// The conversation this message belongs to.
    pub conversation_id: ConversationId,
    /// The sender role.
    pub role: MessageRole,
    /// Typed content.
    pub content: MessageContent,
    /// When this message was created.
    pub created_at: DateTime<Utc>,
    /// Approximate token count (if known).
    pub token_count: Option<u32>,
}

impl Message {
    /// Estimate the token count for this message using the 4-chars-per-token
    /// heuristic. Returns the stored `token_count` if already set.
    #[must_use]
    pub fn estimated_tokens(&self) -> u32 {
        if let Some(count) = self.token_count {
            return count;
        }
        estimate_tokens_for_content(&self.content)
    }
}

/// Approximate token count for a [`MessageContent`] value.
///
/// Uses the simple heuristic of 4 characters per token.
#[must_use]
fn estimate_tokens_for_content(content: &MessageContent) -> u32 {
    let char_count = match content {
        MessageContent::Text { text } => text.len(),
        MessageContent::ToolCall { name, arguments } => {
            name.len() + arguments.to_string().len()
        }
        MessageContent::ToolResult { tool_call_id, output } => {
            tool_call_id.len() + output.to_string().len()
        }
        MessageContent::Mixed { parts } => {
            parts.iter().map(|p| match p {
                ContentPart::Text { text } => text.len(),
                ContentPart::ToolUse { id, name, input } => {
                    id.len() + name.len() + input.to_string().len()
                }
                ContentPart::ToolResult { id, content } => {
                    id.len() + content.to_string().len()
                }
            }).sum()
        }
    };
    // 4 chars ~= 1 token, rounding up
    #[allow(clippy::cast_possible_truncation)]
    let tokens = (char_count as f64 / 4.0).ceil() as u32;
    tokens.max(1) // every message is at least 1 token
}

// ---------------------------------------------------------------------------
// Conversation
// ---------------------------------------------------------------------------

/// A conversation session between an agent and its interlocutors.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Conversation {
    /// Unique identifier for this conversation.
    pub id: ConversationId,
    /// The agent that owns this conversation.
    pub agent_id: AgentId,
    /// Optional human-readable title.
    pub title: Option<String>,
    /// When this conversation was created.
    pub created_at: DateTime<Utc>,
    /// When this conversation was last updated.
    pub updated_at: DateTime<Utc>,
    /// Number of messages in this conversation.
    pub message_count: u32,
    /// Arbitrary key-value metadata attached to the conversation.
    pub metadata: HashMap<String, String>,
}

impl Conversation {
    /// Create a new conversation with no messages and no title.
    #[must_use]
    pub fn new(id: ConversationId, agent_id: AgentId) -> Self {
        let now = Utc::now();
        Self {
            id,
            agent_id,
            title: None,
            created_at: now,
            updated_at: now,
            message_count: 0,
            metadata: HashMap::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// ConversationSummary
// ---------------------------------------------------------------------------

/// A lightweight projection of a [`Conversation`] for listing views.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConversationSummary {
    /// Conversation identifier.
    pub id: ConversationId,
    /// Owning agent.
    pub agent_id: AgentId,
    /// Optional title.
    pub title: Option<String>,
    /// Number of messages.
    pub message_count: u32,
    /// Timestamp of the most recent message (if any).
    pub last_message_at: Option<DateTime<Utc>>,
    /// When the conversation was created.
    pub created_at: DateTime<Utc>,
}

impl From<&Conversation> for ConversationSummary {
    fn from(conv: &Conversation) -> Self {
        Self {
            id: conv.id,
            agent_id: conv.agent_id,
            title: conv.title.clone(),
            message_count: conv.message_count,
            last_message_at: if conv.message_count > 0 {
                Some(conv.updated_at)
            } else {
                None
            },
            created_at: conv.created_at,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_role_serde_round_trip() {
        for role in [MessageRole::User, MessageRole::Assistant, MessageRole::System, MessageRole::Tool] {
            let json = serde_json::to_string(&role).expect("serialize");
            let back: MessageRole = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(role, back);
        }
    }

    #[test]
    fn message_role_serializes_snake_case() {
        assert_eq!(serde_json::to_string(&MessageRole::User).expect("serialize"), r#""user""#);
        assert_eq!(serde_json::to_string(&MessageRole::Assistant).expect("serialize"), r#""assistant""#);
        assert_eq!(serde_json::to_string(&MessageRole::System).expect("serialize"), r#""system""#);
        assert_eq!(serde_json::to_string(&MessageRole::Tool).expect("serialize"), r#""tool""#);
    }

    #[test]
    fn message_content_text_serde() {
        let content = MessageContent::Text { text: "hello world".into() };
        let json = serde_json::to_string(&content).expect("serialize");
        let back: MessageContent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(content, back);
    }

    #[test]
    fn message_content_tool_call_serde() {
        let content = MessageContent::ToolCall {
            name: "file_read".into(),
            arguments: serde_json::json!({"path": "/tmp/test.txt"}),
        };
        let json = serde_json::to_string(&content).expect("serialize");
        assert!(json.contains("file_read"));
        let back: MessageContent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(content, back);
    }

    #[test]
    fn message_content_tool_result_serde() {
        let content = MessageContent::ToolResult {
            tool_call_id: "call-123".into(),
            output: serde_json::json!({"content": "file contents here"}),
        };
        let json = serde_json::to_string(&content).expect("serialize");
        let back: MessageContent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(content, back);
    }

    #[test]
    fn message_content_mixed_serde() {
        let content = MessageContent::Mixed {
            parts: vec![
                ContentPart::Text { text: "Hello".into() },
                ContentPart::ToolUse {
                    id: "tu-1".into(),
                    name: "calculator".into(),
                    input: serde_json::json!({"expr": "2+2"}),
                },
                ContentPart::ToolResult {
                    id: "tu-1".into(),
                    content: serde_json::json!(4),
                },
            ],
        };
        let json = serde_json::to_string(&content).expect("serialize");
        let back: MessageContent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(content, back);
    }

    #[test]
    fn estimated_tokens_text() {
        let msg = Message {
            id: Uuid::now_v7(),
            conversation_id: ConversationId::new(),
            role: MessageRole::User,
            content: MessageContent::Text { text: "hello world!".into() }, // 12 chars => 3 tokens
            created_at: Utc::now(),
            token_count: None,
        };
        assert_eq!(msg.estimated_tokens(), 3);
    }

    #[test]
    fn estimated_tokens_uses_stored_count_when_set() {
        let msg = Message {
            id: Uuid::now_v7(),
            conversation_id: ConversationId::new(),
            role: MessageRole::User,
            content: MessageContent::Text { text: "hello world!".into() },
            created_at: Utc::now(),
            token_count: Some(42),
        };
        assert_eq!(msg.estimated_tokens(), 42);
    }

    #[test]
    fn estimated_tokens_minimum_one() {
        let msg = Message {
            id: Uuid::now_v7(),
            conversation_id: ConversationId::new(),
            role: MessageRole::User,
            content: MessageContent::Text { text: String::new() },
            created_at: Utc::now(),
            token_count: None,
        };
        assert!(msg.estimated_tokens() >= 1);
    }

    #[test]
    fn conversation_new_defaults() {
        let id = ConversationId::new();
        let agent_id = AgentId::new();
        let conv = Conversation::new(id, agent_id);
        assert_eq!(conv.id, id);
        assert_eq!(conv.agent_id, agent_id);
        assert!(conv.title.is_none());
        assert_eq!(conv.message_count, 0);
        assert!(conv.metadata.is_empty());
    }

    #[test]
    fn conversation_serde_round_trip() {
        let conv = Conversation::new(ConversationId::new(), AgentId::new());
        let json = serde_json::to_string(&conv).expect("serialize");
        let back: Conversation = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(conv.id, back.id);
        assert_eq!(conv.agent_id, back.agent_id);
    }

    #[test]
    fn conversation_summary_from_conversation() {
        let mut conv = Conversation::new(ConversationId::new(), AgentId::new());
        conv.title = Some("Test Chat".into());
        conv.message_count = 5;
        let summary = ConversationSummary::from(&conv);
        assert_eq!(summary.id, conv.id);
        assert_eq!(summary.agent_id, conv.agent_id);
        assert_eq!(summary.title, Some("Test Chat".into()));
        assert_eq!(summary.message_count, 5);
        assert!(summary.last_message_at.is_some());
    }

    #[test]
    fn conversation_summary_no_messages_has_no_last_message() {
        let conv = Conversation::new(ConversationId::new(), AgentId::new());
        let summary = ConversationSummary::from(&conv);
        assert!(summary.last_message_at.is_none());
    }

    #[test]
    fn message_serde_round_trip() {
        let msg = Message {
            id: Uuid::now_v7(),
            conversation_id: ConversationId::new(),
            role: MessageRole::Assistant,
            content: MessageContent::Text { text: "I can help with that.".into() },
            created_at: Utc::now(),
            token_count: Some(10),
        };
        let json = serde_json::to_string(&msg).expect("serialize");
        let back: Message = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(msg.id, back.id);
        assert_eq!(msg.role, back.role);
        assert_eq!(msg.token_count, back.token_count);
    }
}
