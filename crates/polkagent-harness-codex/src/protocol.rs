//! Codex app-server JSON-RPC protocol types.
//!
//! The Codex app-server communicates over JSONL on stdio using a JSON-RPC-like
//! protocol. Notably, Codex omits the standard `"jsonrpc": "2.0"` field from
//! all messages.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Outgoing requests (client -> Codex)
// ---------------------------------------------------------------------------

/// A JSON-RPC request sent to the Codex app-server.
///
/// Note: no `jsonrpc` field -- Codex doesn't include it.
#[derive(Debug, Clone, Serialize)]
pub struct RpcRequest<P: Serialize> {
    /// A unique request identifier.
    pub id: u64,
    /// The RPC method name (e.g. `"initialize"`, `"thread/start"`).
    pub method: &'static str,
    /// The method parameters.
    pub params: P,
}

/// A JSON-RPC notification (no `id`, no response expected).
#[derive(Debug, Clone, Serialize)]
pub struct RpcNotification<P: Serialize> {
    /// The notification method name (e.g. `"initialized"`).
    pub method: &'static str,
    /// The notification parameters.
    pub params: P,
}

// ---------------------------------------------------------------------------
// Initialize handshake
// ---------------------------------------------------------------------------

/// Parameters for the `initialize` request.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeParams {
    /// Client information (name + version).
    pub client_info: ClientInfo,
    /// Client capabilities.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<ClientCapabilities>,
}

/// Client identity sent during initialization.
#[derive(Debug, Clone, Serialize)]
pub struct ClientInfo {
    /// Client name.
    pub name: String,
    /// Client version.
    pub version: String,
}

/// Client capabilities sent during initialization.
#[derive(Debug, Clone, Serialize, Default)]
pub struct ClientCapabilities {}

/// Response to the `initialize` request.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    /// The server name.
    #[serde(default)]
    pub name: String,
    /// The server version.
    #[serde(default)]
    pub version: String,
    /// Protocol version accepted by the server.
    #[serde(default)]
    pub protocol_version: String,
}

// ---------------------------------------------------------------------------
// Thread / Turn lifecycle
// ---------------------------------------------------------------------------

/// Parameters for `thread/start`.
///
/// All fields are optional per the Codex v2 schema.
#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ThreadStartParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval_policy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// Parameters for `thread/resume`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadResumeParams {
    /// The thread ID to resume.
    pub thread_id: String,
}

/// A single user input entry for `turn/start`.
#[derive(Debug, Clone, Serialize)]
pub struct UserInputText {
    /// Must be `"text"`.
    pub r#type: String,
    /// The text content.
    pub text: String,
}

/// Parameters for `turn/start`.
///
/// Requires `threadId` and `input` per the Codex v2 schema.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnStartParams {
    /// The thread ID from thread/start response.
    pub thread_id: String,
    /// The user input (array of UserInput objects).
    pub input: Vec<UserInputText>,
}

// ---------------------------------------------------------------------------
// Approval flow
// ---------------------------------------------------------------------------

/// Parameters for responding to a `commandExecution/requestApproval` or
/// `fileChange/requestApproval` server request.
#[derive(Debug, Clone, Serialize)]
pub struct ApprovalResponse {
    /// Whether the action is approved.
    pub approved: bool,
}

// ---------------------------------------------------------------------------
// Incoming messages (Codex -> client)
// ---------------------------------------------------------------------------

/// A raw incoming JSONL message from Codex.
///
/// We parse this generically first, then dispatch based on whether it has
/// an `id` (response), `method` (notification/request), or is unknown.
#[derive(Debug, Clone, Deserialize)]
pub struct RawIncoming {
    /// Present on responses and server-initiated requests.
    pub id: Option<serde_json::Value>,
    /// Present on notifications and server-initiated requests.
    pub method: Option<String>,
    /// Present on successful responses.
    pub result: Option<serde_json::Value>,
    /// Present on error responses.
    pub error: Option<RpcError>,
    /// Present on notifications and server-initiated requests.
    pub params: Option<serde_json::Value>,
}

/// A JSON-RPC error object.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RpcError {
    /// Numeric error code.
    pub code: i64,
    /// Human-readable error message.
    pub message: String,
    /// Optional additional data.
    pub data: Option<serde_json::Value>,
}

/// Codex backpressure error code.
pub const BACKPRESSURE_ERROR_CODE: i64 = -32001;

// ---------------------------------------------------------------------------
// Parsed notification events
// ---------------------------------------------------------------------------

/// Typed notification events from the Codex server.
#[derive(Debug, Clone)]
pub enum CodexNotification {
    /// `turn/started` -- a new turn has begun.
    TurnStarted {
        /// The turn identifier.
        turn_id: Option<String>,
    },
    /// `item/started` -- an item (tool call, message, etc.) has started.
    ItemStarted {
        /// The item identifier.
        item_id: Option<String>,
        /// The item type (e.g. `"agentMessage"`, `"commandExecution"`).
        item_type: Option<String>,
    },
    /// `item/agentMessage/delta` -- a chunk of streaming agent message text.
    AgentMessageDelta {
        /// The text chunk.
        delta: String,
    },
    /// `item/completed` -- an item has completed.
    ItemCompleted {
        /// The item identifier.
        item_id: Option<String>,
        /// The item type.
        item_type: Option<String>,
        /// Full text content if available (for agent messages).
        text: Option<String>,
        /// Command for tool calls.
        command: Option<String>,
        /// Arguments JSON for tool calls.
        arguments_json: Option<String>,
    },
    /// `turn/completed` -- the current turn has finished.
    TurnCompleted,
    /// `commandExecution/requestApproval` -- Codex requests approval to run a command.
    CommandApprovalRequested {
        /// Server request ID for the response.
        request_id: serde_json::Value,
        /// The command to be executed.
        command: String,
    },
    /// `fileChange/requestApproval` -- Codex requests approval for a file change.
    FileChangeApprovalRequested {
        /// Server request ID for the response.
        request_id: serde_json::Value,
        /// Path of the file to be changed.
        file_path: String,
    },
    /// An unknown notification method.
    Unknown {
        /// The method name.
        method: String,
    },
}

impl CodexNotification {
    /// Parse a notification from a `RawIncoming` message that has a `method`.
    pub fn from_raw(method: &str, params: Option<&serde_json::Value>, id: Option<&serde_json::Value>) -> Self {
        match method {
            "turn/started" => {
                let turn_id = params
                    .and_then(|p| p.get("turnId"))
                    .and_then(serde_json::Value::as_str)
                    .map(String::from);
                Self::TurnStarted { turn_id }
            }
            "item/started" => {
                let item_id = params
                    .and_then(|p| p.get("itemId"))
                    .and_then(serde_json::Value::as_str)
                    .map(String::from);
                let item_type = params
                    .and_then(|p| p.get("itemType"))
                    .and_then(serde_json::Value::as_str)
                    .map(String::from);
                Self::ItemStarted { item_id, item_type }
            }
            "item/agentMessage/delta" => {
                let delta = params
                    .and_then(|p| p.get("delta"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_owned();
                Self::AgentMessageDelta { delta }
            }
            "item/completed" => {
                let item_id = params
                    .and_then(|p| p.get("itemId"))
                    .and_then(serde_json::Value::as_str)
                    .map(String::from);
                let item_type = params
                    .and_then(|p| p.get("itemType"))
                    .and_then(serde_json::Value::as_str)
                    .map(String::from);
                let text = params
                    .and_then(|p| p.get("text"))
                    .and_then(serde_json::Value::as_str)
                    .map(String::from);
                let command = params
                    .and_then(|p| p.get("command"))
                    .and_then(serde_json::Value::as_str)
                    .map(String::from);
                let arguments_json = params
                    .and_then(|p| p.get("arguments"))
                    .map(|v| v.to_string());
                Self::ItemCompleted {
                    item_id,
                    item_type,
                    text,
                    command,
                    arguments_json,
                }
            }
            "turn/completed" => Self::TurnCompleted,
            "commandExecution/requestApproval" => {
                let command = params
                    .and_then(|p| p.get("command"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_owned();
                let request_id = id.cloned().unwrap_or(serde_json::Value::Null);
                Self::CommandApprovalRequested {
                    request_id,
                    command,
                }
            }
            "fileChange/requestApproval" => {
                let file_path = params
                    .and_then(|p| p.get("filePath"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_owned();
                let request_id = id.cloned().unwrap_or(serde_json::Value::Null);
                Self::FileChangeApprovalRequested {
                    request_id,
                    file_path,
                }
            }
            other => Self::Unknown {
                method: other.to_owned(),
            },
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
    fn rpc_request_serializes_without_jsonrpc_field() {
        let req = RpcRequest {
            id: 1,
            method: "initialize",
            params: InitializeParams {
                client_info: ClientInfo {
                    name: "polkagent".into(),
                    version: "0.1.0".into(),
                },
                capabilities: None,
            },
        };
        let json = serde_json::to_string(&req).expect("serialize");
        assert!(!json.contains("jsonrpc"));
        assert!(json.contains("\"id\":1"));
        assert!(json.contains("\"method\":\"initialize\""));
        assert!(json.contains("polkagent"));
    }

    #[test]
    fn rpc_notification_serializes_without_id() {
        let notif = RpcNotification {
            method: "initialized",
            params: serde_json::json!({}),
        };
        let json = serde_json::to_string(&notif).expect("serialize");
        assert!(!json.contains("\"id\""));
        assert!(json.contains("\"method\":\"initialized\""));
    }

    #[test]
    fn raw_incoming_parses_response() {
        let json = r#"{"id":1,"result":{"name":"codex","version":"0.1"}}"#;
        let raw: RawIncoming = serde_json::from_str(json).expect("parse");
        assert!(raw.id.is_some());
        assert!(raw.result.is_some());
        assert!(raw.method.is_none());
    }

    #[test]
    fn raw_incoming_parses_notification() {
        let json = r#"{"method":"turn/started","params":{"turnId":"t1"}}"#;
        let raw: RawIncoming = serde_json::from_str(json).expect("parse");
        assert!(raw.method.is_some());
        assert!(raw.id.is_none());
    }

    #[test]
    fn raw_incoming_parses_error_response() {
        let json = r#"{"id":2,"error":{"code":-32001,"message":"backpressure"}}"#;
        let raw: RawIncoming = serde_json::from_str(json).expect("parse");
        assert!(raw.error.is_some());
        let err = raw.error.as_ref().expect("error");
        assert_eq!(err.code, BACKPRESSURE_ERROR_CODE);
    }

    #[test]
    fn raw_incoming_parses_server_request() {
        let json = r#"{"id":99,"method":"commandExecution/requestApproval","params":{"command":"rm -rf /tmp/foo"}}"#;
        let raw: RawIncoming = serde_json::from_str(json).expect("parse");
        assert!(raw.id.is_some());
        assert!(raw.method.is_some());
    }

    #[test]
    fn codex_notification_turn_started() {
        let params = serde_json::json!({"turnId": "t42"});
        let notif = CodexNotification::from_raw("turn/started", Some(&params), None);
        match notif {
            CodexNotification::TurnStarted { turn_id } => {
                assert_eq!(turn_id.as_deref(), Some("t42"));
            }
            other => panic!("expected TurnStarted, got {other:?}"),
        }
    }

    #[test]
    fn codex_notification_item_started() {
        let params = serde_json::json!({"itemId": "i1", "itemType": "agentMessage"});
        let notif = CodexNotification::from_raw("item/started", Some(&params), None);
        match notif {
            CodexNotification::ItemStarted { item_id, item_type } => {
                assert_eq!(item_id.as_deref(), Some("i1"));
                assert_eq!(item_type.as_deref(), Some("agentMessage"));
            }
            other => panic!("expected ItemStarted, got {other:?}"),
        }
    }

    #[test]
    fn codex_notification_agent_message_delta() {
        let params = serde_json::json!({"delta": "Hello "});
        let notif = CodexNotification::from_raw("item/agentMessage/delta", Some(&params), None);
        match notif {
            CodexNotification::AgentMessageDelta { delta } => {
                assert_eq!(delta, "Hello ");
            }
            other => panic!("expected AgentMessageDelta, got {other:?}"),
        }
    }

    #[test]
    fn codex_notification_item_completed() {
        let params = serde_json::json!({
            "itemId": "i1",
            "itemType": "commandExecution",
            "command": "ls -la",
            "arguments": {"path": "/tmp"}
        });
        let notif = CodexNotification::from_raw("item/completed", Some(&params), None);
        match notif {
            CodexNotification::ItemCompleted {
                item_id,
                item_type,
                text,
                command,
                arguments_json,
            } => {
                assert_eq!(item_id.as_deref(), Some("i1"));
                assert_eq!(item_type.as_deref(), Some("commandExecution"));
                assert!(text.is_none());
                assert_eq!(command.as_deref(), Some("ls -la"));
                assert!(arguments_json.is_some());
            }
            other => panic!("expected ItemCompleted, got {other:?}"),
        }
    }

    #[test]
    fn codex_notification_turn_completed() {
        let notif = CodexNotification::from_raw("turn/completed", None, None);
        assert!(matches!(notif, CodexNotification::TurnCompleted));
    }

    #[test]
    fn codex_notification_command_approval() {
        let params = serde_json::json!({"command": "npm install"});
        let id = serde_json::json!(42);
        let notif = CodexNotification::from_raw(
            "commandExecution/requestApproval",
            Some(&params),
            Some(&id),
        );
        match notif {
            CodexNotification::CommandApprovalRequested {
                request_id,
                command,
            } => {
                assert_eq!(request_id, serde_json::json!(42));
                assert_eq!(command, "npm install");
            }
            other => panic!("expected CommandApprovalRequested, got {other:?}"),
        }
    }

    #[test]
    fn codex_notification_file_change_approval() {
        let params = serde_json::json!({"filePath": "/tmp/foo.rs"});
        let id = serde_json::json!(43);
        let notif = CodexNotification::from_raw(
            "fileChange/requestApproval",
            Some(&params),
            Some(&id),
        );
        match notif {
            CodexNotification::FileChangeApprovalRequested {
                request_id,
                file_path,
            } => {
                assert_eq!(request_id, serde_json::json!(43));
                assert_eq!(file_path, "/tmp/foo.rs");
            }
            other => panic!("expected FileChangeApprovalRequested, got {other:?}"),
        }
    }

    #[test]
    fn codex_notification_unknown() {
        let notif = CodexNotification::from_raw("custom/event", None, None);
        match notif {
            CodexNotification::Unknown { method } => {
                assert_eq!(method, "custom/event");
            }
            other => panic!("expected Unknown, got {other:?}"),
        }
    }

    #[test]
    fn approval_response_serializes() {
        let resp = ApprovalResponse { approved: true };
        let json = serde_json::to_string(&resp).expect("serialize");
        assert!(json.contains("\"approved\":true"));
    }

    #[test]
    fn initialize_result_deserializes_with_defaults() {
        let json = r#"{}"#;
        let result: InitializeResult = serde_json::from_str(json).expect("parse");
        assert_eq!(result.name, "");
        assert_eq!(result.version, "");
    }

    #[test]
    fn initialize_result_deserializes_full() {
        let json = r#"{"name":"codex","version":"1.0","protocolVersion":"1.0"}"#;
        let result: InitializeResult = serde_json::from_str(json).expect("parse");
        assert_eq!(result.name, "codex");
        assert_eq!(result.version, "1.0");
        assert_eq!(result.protocol_version, "1.0");
    }

    #[test]
    fn turn_start_params_serializes() {
        let params = TurnStartParams {
            thread_id: "t-123".into(),
            input: vec![UserInputText {
                r#type: "text".into(),
                text: "Hello".into(),
            }],
        };
        let json = serde_json::to_string(&params).expect("serialize");
        assert!(json.contains("\"threadId\":\"t-123\""));
        assert!(json.contains("\"text\":\"Hello\""));
        assert!(json.contains("\"type\":\"text\""));
    }
}
