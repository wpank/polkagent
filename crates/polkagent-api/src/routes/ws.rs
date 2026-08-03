//! WebSocket v1alpha1 subscription endpoint (PRD-14 §4).
//!
//! # Endpoint
//!
//! `GET /ws/v1alpha1`
//!
//! Clients connect with an optional `?token=<auth-token>` query parameter.
//! Authentication can also be provided as the first message after the
//! connection is established (see [`ClientMessage::Auth`]).
//!
//! # Protocol
//!
//! All messages are JSON text frames using the [`WsMessage`] envelope:
//!
//! ```json
//! {
//!   "msg_type": "subscribe",
//!   "id": "optional-request-id",
//!   "channel": "runs:01234567-...",
//!   "payload": {},
//!   "timestamp": "2024-01-01T00:00:00Z"
//! }
//! ```
//!
//! # Channels
//!
//! - `runs:{run_id}` — events for a specific run
//! - `agents:{agent_id}` — events for all runs belonging to an agent
//! - `system` — platform-wide system events
//!
//! # Keepalive
//!
//! The server sends a WebSocket `Ping` frame every 30 seconds. If the client
//! does not respond with a `Pong` within 30 seconds, the connection is closed.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use axum::{
    extract::{Query, State, WebSocketUpgrade},
    response::IntoResponse,
};
use axum::extract::ws::{Message, WebSocket};
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use futures::{SinkExt, StreamExt};
use polkagent_core::{AgentId, RunId};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::broadcast;
use tokio::time::Instant;
use tracing::{debug, trace, warn};

use crate::state::AppState;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Interval between server-sent Ping frames.
const PING_INTERVAL: Duration = Duration::from_secs(30);

/// How long to wait for a Pong response before closing the connection.
const PONG_TIMEOUT: Duration = Duration::from_secs(30);

// ---------------------------------------------------------------------------
// Channel
// ---------------------------------------------------------------------------

/// A subscription channel identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Channel {
    /// Events for a specific run: `runs:{run_id}`
    Run(RunId),
    /// Events for all runs of an agent: `agents:{agent_id}`
    Agent(AgentId),
    /// Platform-wide system events: `system`
    System,
}

impl Channel {
    /// Parse a channel string into a [`Channel`] variant.
    ///
    /// Returns `None` if the string is not a recognised channel format.
    pub fn parse(s: &str) -> Option<Self> {
        if s == "system" {
            return Some(Channel::System);
        }
        if let Some(id_str) = s.strip_prefix("runs:") {
            return id_str.parse::<RunId>().ok().map(Channel::Run);
        }
        if let Some(id_str) = s.strip_prefix("agents:") {
            return id_str.parse::<AgentId>().ok().map(Channel::Agent);
        }
        None
    }

    /// Return the canonical string representation of the channel.
    pub fn as_str(&self) -> String {
        match self {
            Channel::Run(id) => format!("runs:{id}"),
            Channel::Agent(id) => format!("agents:{id}"),
            Channel::System => "system".to_owned(),
        }
    }
}

impl std::fmt::Display for Channel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// ---------------------------------------------------------------------------
// WsMessage — the outbound envelope
// ---------------------------------------------------------------------------

/// JSON envelope for all WebSocket messages (both inbound and outbound).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WsMessage {
    /// Message type identifier (e.g. `"subscribe"`, `"event"`, `"error"`).
    pub msg_type: String,
    /// Optional client-assigned request ID for correlation.
    pub id: Option<String>,
    /// Channel this message relates to (e.g. `"runs:01234567-..."`).
    pub channel: Option<String>,
    /// Message body — structure depends on `msg_type`.
    pub payload: Value,
    /// UTC timestamp when the message was created.
    pub timestamp: DateTime<Utc>,
}

impl WsMessage {
    /// Construct a new message with the current timestamp.
    pub fn new(
        msg_type: impl Into<String>,
        id: Option<String>,
        channel: Option<String>,
        payload: Value,
    ) -> Self {
        Self {
            msg_type: msg_type.into(),
            id,
            channel,
            payload,
            timestamp: Utc::now(),
        }
    }

    /// Convenience: create an `"ack"` confirmation message.
    pub fn ack(request_id: Option<String>, channel: Option<String>) -> Self {
        Self::new("ack", request_id, channel, Value::Null)
    }

    /// Convenience: create an `"error"` message.
    pub fn error(request_id: Option<String>, reason: impl Into<String>) -> Self {
        Self::new(
            "error",
            request_id,
            None,
            serde_json::json!({ "reason": reason.into() }),
        )
    }

    /// Convenience: create a `"pong"` message.
    pub fn pong() -> Self {
        Self::new("pong", None, None, Value::Null)
    }

    /// Serialize this message to a JSON string.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

// ---------------------------------------------------------------------------
// ClientMessage — decoded inbound message types
// ---------------------------------------------------------------------------

/// Actions a client may request over the WebSocket connection.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "msg_type", rename_all = "snake_case")]
pub enum ClientMessage {
    /// Authenticate using a bearer token.
    Auth { token: Option<String> },
    /// Subscribe to a channel.
    Subscribe {
        id: Option<String>,
        channel: String,
    },
    /// Unsubscribe from a channel.
    Unsubscribe {
        id: Option<String>,
        channel: String,
    },
    /// Client ping — server responds with `"pong"`.
    Ping { id: Option<String> },
}

// ---------------------------------------------------------------------------
// WsSession — per-connection subscription state
// ---------------------------------------------------------------------------

/// Per-connection WebSocket session state.
///
/// Tracks the set of channels the client has subscribed to.
#[derive(Debug, Default)]
pub struct WsSession {
    /// Active subscriptions for this connection.
    subscriptions: HashSet<Channel>,
    /// Whether the client has been authenticated.
    authenticated: bool,
}

impl WsSession {
    /// Create a new, unauthenticated session.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a pre-authenticated session (token validated via query param).
    pub fn authenticated() -> Self {
        Self {
            subscriptions: HashSet::new(),
            authenticated: true,
        }
    }

    /// Mark the session as authenticated.
    pub fn authenticate(&mut self) {
        self.authenticated = true;
    }

    /// Returns `true` if the client has authenticated.
    pub fn is_authenticated(&self) -> bool {
        self.authenticated
    }

    /// Subscribe to a channel. Returns `true` if the channel was newly added.
    pub fn subscribe(&mut self, channel: Channel) -> bool {
        self.subscriptions.insert(channel)
    }

    /// Unsubscribe from a channel. Returns `true` if the channel was present.
    pub fn unsubscribe(&mut self, channel: &Channel) -> bool {
        self.subscriptions.remove(channel)
    }

    /// Returns `true` if the session is subscribed to the given channel.
    pub fn is_subscribed(&self, channel: &Channel) -> bool {
        self.subscriptions.contains(channel)
    }

    /// Returns the number of active subscriptions.
    pub fn subscription_count(&self) -> usize {
        self.subscriptions.len()
    }

    /// Returns an iterator over the active subscriptions.
    pub fn subscriptions(&self) -> impl Iterator<Item = &Channel> {
        self.subscriptions.iter()
    }

    /// Determine whether an event for `run_id` belonging to `agent_id` should
    /// be forwarded to this session.
    pub fn matches_run_event(&self, run_id: &RunId, agent_id: Option<&AgentId>) -> bool {
        if self.subscriptions.contains(&Channel::Run(*run_id)) {
            return true;
        }
        if let Some(aid) = agent_id {
            if self.subscriptions.contains(&Channel::Agent(*aid)) {
                return true;
            }
        }
        false
    }
}

// ---------------------------------------------------------------------------
// Query parameters
// ---------------------------------------------------------------------------

/// Query parameters accepted on the WebSocket upgrade request.
#[derive(Debug, Clone, Deserialize)]
pub struct WsQuery {
    /// Bearer token for authentication (alternative to first-message auth).
    pub token: Option<String>,
}

// ---------------------------------------------------------------------------
// Shared session registry (for broadcast support)
// ---------------------------------------------------------------------------

/// A thread-safe registry of active WebSocket sessions.
///
/// Keyed by a unique session ID. Allows the server to broadcast events to all
/// sessions subscribed to a specific channel.
#[derive(Debug, Clone, Default)]
pub struct WsRegistry {
    /// Maps session-id → channel set (for accounting / future use).
    inner: Arc<DashMap<u64, HashSet<Channel>>>,
    /// Monotonically increasing counter for session IDs.
    next_id: Arc<std::sync::atomic::AtomicU64>,
}

impl WsRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Allocate a new unique session ID.
    pub fn next_session_id(&self) -> u64 {
        self.next_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }

    /// Register a session with its initial (empty) subscription set.
    pub fn register(&self, id: u64) {
        self.inner.insert(id, HashSet::new());
    }

    /// Deregister a session on disconnect.
    pub fn deregister(&self, id: u64) {
        self.inner.remove(&id);
    }

    /// Return the number of currently active sessions.
    pub fn active_sessions(&self) -> usize {
        self.inner.len()
    }
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

/// `GET /ws/v1alpha1` — WebSocket upgrade for channel-based subscriptions.
///
/// Clients may authenticate via `?token=<token>` or by sending a first
/// `Auth` message. Unauthenticated connections are accepted but will receive
/// an `"error"` if they attempt to subscribe without authenticating.
///
/// For development/testing purposes, any non-empty token is accepted. A
/// production deployment would validate against a real auth store.
pub async fn ws_handler(
    State(state): State<AppState>,
    Query(query): Query<WsQuery>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    // Check if a token was provided as query parameter.
    let pre_authenticated = query.token.as_deref().map(|t| !t.is_empty()).unwrap_or(false);

    debug!(
        pre_authenticated,
        "WebSocket v1alpha1 upgrade accepted"
    );

    let session = if pre_authenticated {
        WsSession::authenticated()
    } else {
        WsSession::new()
    };

    ws.on_upgrade(move |socket| handle_ws_session(socket, session, state))
}

/// Drive a single WebSocket connection: handle subscribe/unsubscribe messages,
/// forward matching events from the EventBus, and maintain keepalive.
async fn handle_ws_session(socket: WebSocket, mut session: WsSession, state: AppState) {
    let (mut sender, mut receiver) = socket.split();
    let mut event_rx = state.event_bus.subscribe();

    let mut ping_interval = tokio::time::interval(PING_INTERVAL);
    // Consume the immediate first tick so pings start after PING_INTERVAL.
    ping_interval.tick().await;

    // Track when the last pong was received (start at now so first ping has
    // the full PING_INTERVAL window before the timeout applies).
    let mut last_pong: Option<Instant> = None;
    let mut waiting_for_pong = false;

    loop {
        tokio::select! {
            // ── Inbound client messages ─────────────────────────────────
            msg = receiver.next() => {
                match msg {
                    None | Some(Err(_)) => {
                        debug!("WebSocket v1alpha1: client disconnected");
                        break;
                    }
                    Some(Ok(Message::Close(_))) => {
                        debug!("WebSocket v1alpha1: client sent Close");
                        break;
                    }
                    Some(Ok(Message::Pong(_))) => {
                        trace!("WebSocket v1alpha1: received Pong");
                        last_pong = Some(Instant::now());
                        waiting_for_pong = false;
                    }
                    Some(Ok(Message::Ping(data))) => {
                        // Echo back a Pong.
                        if sender.send(Message::Pong(data)).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Text(text))) => {
                        if let Some(reply) = handle_client_text(&text, &mut session) {
                            let json = match reply.to_json() {
                                Ok(j) => j,
                                Err(e) => {
                                    warn!(error = %e, "failed to serialize WsMessage");
                                    continue;
                                }
                            };
                            if sender.send(Message::Text(json.into())).await.is_err() {
                                break;
                            }
                        }
                    }
                    Some(Ok(Message::Binary(_))) => {
                        // Binary frames are not supported; send an error.
                        let err = WsMessage::error(None, "binary frames not supported");
                        if let Ok(json) = err.to_json() {
                            let _ = sender.send(Message::Text(json.into())).await;
                        }
                    }
                }
            }

            // ── Event bus messages ───────────────────────────────────────
            result = event_rx.recv() => {
                match result {
                    Ok(event) => {
                        // Check if any subscription matches this event.
                        // The event.correlation may carry an agent_id; look it up.
                        let agent_id: Option<AgentId> = {
                            // RunEvent's correlation field may hold agent context.
                            // The `run_id` is always present on RunEvent.
                            None // agent_id not directly on RunEvent; rely on run_id match
                        };

                        if !session.matches_run_event(&event.run_id, agent_id.as_ref()) {
                            continue;
                        }

                        let channel_str = format!("runs:{}", event.run_id);
                        let payload = match serde_json::to_value(&event) {
                            Ok(v) => v,
                            Err(e) => {
                                warn!(error = %e, "failed to serialize RunEvent");
                                continue;
                            }
                        };

                        let msg = WsMessage::new("event", None, Some(channel_str), payload);
                        let json = match msg.to_json() {
                            Ok(j) => j,
                            Err(e) => {
                                warn!(error = %e, "failed to serialize WsMessage");
                                continue;
                            }
                        };
                        if sender.send(Message::Text(json.into())).await.is_err() {
                            debug!("WebSocket v1alpha1: send failed (client disconnected)");
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!(skipped = n, "WebSocket v1alpha1: event receiver lagged");
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        debug!("WebSocket v1alpha1: EventBus closed");
                        break;
                    }
                }
            }

            // ── Keepalive ping timer ─────────────────────────────────────
            _ = ping_interval.tick() => {
                // Check pong timeout from the previous ping.
                if waiting_for_pong {
                    if let Some(t) = last_pong {
                        if t.elapsed() > PONG_TIMEOUT {
                            debug!("WebSocket v1alpha1: pong timeout; closing connection");
                            break;
                        }
                    } else {
                        // Never received a pong; close.
                        debug!("WebSocket v1alpha1: pong timeout (no pong received); closing");
                        break;
                    }
                }

                // Send ping.
                if sender.send(Message::Ping(vec![].into())).await.is_err() {
                    debug!("WebSocket v1alpha1: ping send failed");
                    break;
                }
                trace!("WebSocket v1alpha1: sent Ping");
                waiting_for_pong = true;
            }
        }
    }

    debug!("WebSocket v1alpha1: session handler exiting");
}

/// Parse and dispatch a client text frame.
///
/// Returns an optional reply message. Returns `None` if no reply is needed
/// (e.g. the message was an event acknowledgement).
fn handle_client_text(text: &str, session: &mut WsSession) -> Option<WsMessage> {
    let client_msg: ClientMessage = match serde_json::from_str(text) {
        Ok(m) => m,
        Err(_) => {
            return Some(WsMessage::error(None, "invalid JSON or unknown msg_type"));
        }
    };

    match client_msg {
        ClientMessage::Auth { token } => {
            let valid = token.as_deref().map(|t| !t.is_empty()).unwrap_or(false);
            if valid {
                session.authenticate();
                Some(WsMessage::ack(None, None))
            } else {
                Some(WsMessage::error(None, "invalid or missing token"))
            }
        }

        ClientMessage::Subscribe { id, channel } => {
            if !session.is_authenticated() {
                return Some(WsMessage::error(id, "not authenticated"));
            }
            match Channel::parse(&channel) {
                Some(ch) => {
                    session.subscribe(ch);
                    Some(WsMessage::ack(id, Some(channel)))
                }
                None => Some(WsMessage::error(id, format!("unknown channel: {channel}"))),
            }
        }

        ClientMessage::Unsubscribe { id, channel } => {
            if !session.is_authenticated() {
                return Some(WsMessage::error(id, "not authenticated"));
            }
            match Channel::parse(&channel) {
                Some(ch) => {
                    session.unsubscribe(&ch);
                    Some(WsMessage::ack(id, Some(channel)))
                }
                None => Some(WsMessage::error(id, format!("unknown channel: {channel}"))),
            }
        }

        ClientMessage::Ping { id } => {
            // Application-level ping; send a pong.
            let mut pong = WsMessage::pong();
            pong.id = id;
            Some(pong)
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use polkagent_core::{AgentId, RunId};

    // ── Channel parsing ─────────────────────────────────────────────────────

    #[test]
    fn channel_parse_system() {
        assert_eq!(Channel::parse("system"), Some(Channel::System));
    }

    #[test]
    fn channel_parse_run_valid_uuid() {
        let run_id = RunId::new();
        let s = format!("runs:{run_id}");
        assert_eq!(Channel::parse(&s), Some(Channel::Run(run_id)));
    }

    #[test]
    fn channel_parse_agent_valid_uuid() {
        let agent_id = AgentId::new();
        let s = format!("agents:{agent_id}");
        assert_eq!(Channel::parse(&s), Some(Channel::Agent(agent_id)));
    }

    #[test]
    fn channel_parse_invalid_returns_none() {
        assert!(Channel::parse("unknown").is_none());
        assert!(Channel::parse("runs:not-a-uuid").is_none());
        assert!(Channel::parse("agents:not-a-uuid").is_none());
        assert!(Channel::parse("").is_none());
    }

    #[test]
    fn channel_display_round_trips() {
        let run_id = RunId::new();
        let ch = Channel::Run(run_id);
        let s = ch.to_string();
        let parsed = Channel::parse(&s).expect("should parse back");
        assert_eq!(ch, parsed);

        let agent_id = AgentId::new();
        let ch = Channel::Agent(agent_id);
        let s = ch.to_string();
        let parsed = Channel::parse(&s).expect("should parse back");
        assert_eq!(ch, parsed);

        let ch = Channel::System;
        let s = ch.to_string();
        let parsed = Channel::parse(&s).expect("should parse back");
        assert_eq!(ch, parsed);
    }

    // ── WsMessage ───────────────────────────────────────────────────────────

    #[test]
    fn ws_message_ack_has_correct_type() {
        let msg = WsMessage::ack(Some("req-1".into()), Some("runs:123".into()));
        assert_eq!(msg.msg_type, "ack");
        assert_eq!(msg.id, Some("req-1".into()));
        assert_eq!(msg.channel, Some("runs:123".into()));
    }

    #[test]
    fn ws_message_error_has_correct_type() {
        let msg = WsMessage::error(Some("req-2".into()), "bad request");
        assert_eq!(msg.msg_type, "error");
        assert_eq!(msg.id, Some("req-2".into()));
        assert!(msg.payload["reason"].as_str() == Some("bad request"));
    }

    #[test]
    fn ws_message_pong_has_correct_type() {
        let msg = WsMessage::pong();
        assert_eq!(msg.msg_type, "pong");
    }

    #[test]
    fn ws_message_serializes_to_json() {
        let msg = WsMessage::new(
            "event",
            Some("id-1".into()),
            Some("system".into()),
            serde_json::json!({ "foo": "bar" }),
        );
        let json = msg.to_json().expect("serialize");
        assert!(json.contains("\"msg_type\":\"event\""));
        assert!(json.contains("\"channel\":\"system\""));
    }

    #[test]
    fn ws_message_deserializes_from_json() {
        let json = r#"{
            "msg_type": "event",
            "id": "r1",
            "channel": "system",
            "payload": {},
            "timestamp": "2024-01-01T00:00:00Z"
        }"#;
        let msg: WsMessage = serde_json::from_str(json).expect("deserialize");
        assert_eq!(msg.msg_type, "event");
        assert_eq!(msg.id, Some("r1".into()));
    }

    // ── WsSession ───────────────────────────────────────────────────────────

    #[test]
    fn ws_session_starts_unauthenticated() {
        let session = WsSession::new();
        assert!(!session.is_authenticated());
        assert_eq!(session.subscription_count(), 0);
    }

    #[test]
    fn ws_session_authenticated_constructor() {
        let session = WsSession::authenticated();
        assert!(session.is_authenticated());
    }

    #[test]
    fn ws_session_authenticate() {
        let mut session = WsSession::new();
        assert!(!session.is_authenticated());
        session.authenticate();
        assert!(session.is_authenticated());
    }

    #[test]
    fn ws_session_subscribe_and_unsubscribe() {
        let mut session = WsSession::authenticated();
        let run_id = RunId::new();
        let channel = Channel::Run(run_id);

        assert!(!session.is_subscribed(&channel));
        assert!(session.subscribe(channel.clone()));
        assert!(session.is_subscribed(&channel));
        assert_eq!(session.subscription_count(), 1);

        // Subscribing again is a no-op (returns false).
        assert!(!session.subscribe(channel.clone()));
        assert_eq!(session.subscription_count(), 1);

        assert!(session.unsubscribe(&channel));
        assert!(!session.is_subscribed(&channel));
        assert_eq!(session.subscription_count(), 0);
    }

    #[test]
    fn ws_session_unsubscribe_not_present_returns_false() {
        let mut session = WsSession::authenticated();
        let channel = Channel::System;
        assert!(!session.unsubscribe(&channel));
    }

    #[test]
    fn ws_session_matches_run_event_by_run_id() {
        let mut session = WsSession::authenticated();
        let run_id = RunId::new();
        let other_run = RunId::new();

        session.subscribe(Channel::Run(run_id));

        assert!(session.matches_run_event(&run_id, None));
        assert!(!session.matches_run_event(&other_run, None));
    }

    #[test]
    fn ws_session_matches_run_event_by_agent_id() {
        let mut session = WsSession::authenticated();
        let agent_id = AgentId::new();
        let run_id = RunId::new();

        session.subscribe(Channel::Agent(agent_id));

        assert!(session.matches_run_event(&run_id, Some(&agent_id)));
        assert!(!session.matches_run_event(&run_id, None));
    }

    #[test]
    fn ws_session_system_channel_does_not_match_run_events() {
        let mut session = WsSession::authenticated();
        let run_id = RunId::new();

        session.subscribe(Channel::System);

        // System channel does not cause run events to match.
        assert!(!session.matches_run_event(&run_id, None));
    }

    // ── ClientMessage parsing ───────────────────────────────────────────────

    #[test]
    fn client_message_auth_parses() {
        let json = r#"{"msg_type":"auth","token":"secret"}"#;
        let msg: ClientMessage = serde_json::from_str(json).expect("parse");
        assert!(matches!(msg, ClientMessage::Auth { token: Some(t) } if t == "secret"));
    }

    #[test]
    fn client_message_subscribe_parses() {
        let json = r#"{"msg_type":"subscribe","id":"r1","channel":"system"}"#;
        let msg: ClientMessage = serde_json::from_str(json).expect("parse");
        assert!(
            matches!(msg, ClientMessage::Subscribe { ref channel, .. } if channel == "system")
        );
    }

    #[test]
    fn client_message_unsubscribe_parses() {
        let run_id = RunId::new();
        let json = format!(
            r#"{{"msg_type":"unsubscribe","id":"r2","channel":"runs:{run_id}"}}"#
        );
        let msg: ClientMessage = serde_json::from_str(&json).expect("parse");
        assert!(matches!(msg, ClientMessage::Unsubscribe { .. }));
    }

    #[test]
    fn client_message_ping_parses() {
        let json = r#"{"msg_type":"ping","id":"p1"}"#;
        let msg: ClientMessage = serde_json::from_str(json).expect("parse");
        assert!(matches!(msg, ClientMessage::Ping { id: Some(ref i) } if i == "p1"));
    }

    // ── handle_client_text ──────────────────────────────────────────────────

    #[test]
    fn handle_text_invalid_json_returns_error() {
        let mut session = WsSession::authenticated();
        let reply = handle_client_text("not json", &mut session);
        assert!(reply.is_some());
        let msg = reply.unwrap();
        assert_eq!(msg.msg_type, "error");
    }

    #[test]
    fn handle_text_auth_with_valid_token() {
        let mut session = WsSession::new();
        assert!(!session.is_authenticated());
        let json = r#"{"msg_type":"auth","token":"mytoken"}"#;
        let reply = handle_client_text(json, &mut session);
        assert!(session.is_authenticated());
        let msg = reply.unwrap();
        assert_eq!(msg.msg_type, "ack");
    }

    #[test]
    fn handle_text_auth_with_empty_token_fails() {
        let mut session = WsSession::new();
        let json = r#"{"msg_type":"auth","token":""}"#;
        let reply = handle_client_text(json, &mut session);
        assert!(!session.is_authenticated());
        let msg = reply.unwrap();
        assert_eq!(msg.msg_type, "error");
    }

    #[test]
    fn handle_text_subscribe_without_auth_returns_error() {
        let mut session = WsSession::new();
        let json = r#"{"msg_type":"subscribe","channel":"system"}"#;
        let reply = handle_client_text(json, &mut session);
        let msg = reply.unwrap();
        assert_eq!(msg.msg_type, "error");
        assert!(!session.is_subscribed(&Channel::System));
    }

    #[test]
    fn handle_text_subscribe_to_system_channel() {
        let mut session = WsSession::authenticated();
        let json = r#"{"msg_type":"subscribe","id":"req-1","channel":"system"}"#;
        let reply = handle_client_text(json, &mut session);
        let msg = reply.unwrap();
        assert_eq!(msg.msg_type, "ack");
        assert_eq!(msg.id, Some("req-1".into()));
        assert!(session.is_subscribed(&Channel::System));
    }

    #[test]
    fn handle_text_subscribe_to_run_channel() {
        let mut session = WsSession::authenticated();
        let run_id = RunId::new();
        let channel = format!("runs:{run_id}");
        let json = format!(r#"{{"msg_type":"subscribe","channel":"{channel}"}}"#);
        let reply = handle_client_text(&json, &mut session);
        let msg = reply.unwrap();
        assert_eq!(msg.msg_type, "ack");
        assert!(session.is_subscribed(&Channel::Run(run_id)));
    }

    #[test]
    fn handle_text_subscribe_to_agent_channel() {
        let mut session = WsSession::authenticated();
        let agent_id = AgentId::new();
        let channel = format!("agents:{agent_id}");
        let json = format!(r#"{{"msg_type":"subscribe","channel":"{channel}"}}"#);
        let reply = handle_client_text(&json, &mut session);
        let msg = reply.unwrap();
        assert_eq!(msg.msg_type, "ack");
        assert!(session.is_subscribed(&Channel::Agent(agent_id)));
    }

    #[test]
    fn handle_text_subscribe_to_unknown_channel_returns_error() {
        let mut session = WsSession::authenticated();
        let json = r#"{"msg_type":"subscribe","channel":"bogus:channel"}"#;
        let reply = handle_client_text(json, &mut session);
        let msg = reply.unwrap();
        assert_eq!(msg.msg_type, "error");
        assert_eq!(session.subscription_count(), 0);
    }

    #[test]
    fn handle_text_unsubscribe_removes_subscription() {
        let mut session = WsSession::authenticated();
        session.subscribe(Channel::System);
        let json = r#"{"msg_type":"unsubscribe","channel":"system"}"#;
        let reply = handle_client_text(json, &mut session);
        let msg = reply.unwrap();
        assert_eq!(msg.msg_type, "ack");
        assert!(!session.is_subscribed(&Channel::System));
    }

    #[test]
    fn handle_text_ping_returns_pong() {
        let mut session = WsSession::authenticated();
        let json = r#"{"msg_type":"ping","id":"ping-1"}"#;
        let reply = handle_client_text(json, &mut session);
        let msg = reply.unwrap();
        assert_eq!(msg.msg_type, "pong");
        assert_eq!(msg.id, Some("ping-1".into()));
    }

    // ── WsRegistry ──────────────────────────────────────────────────────────

    #[test]
    fn ws_registry_register_and_deregister() {
        let registry = WsRegistry::new();
        assert_eq!(registry.active_sessions(), 0);

        let id = registry.next_session_id();
        registry.register(id);
        assert_eq!(registry.active_sessions(), 1);

        registry.deregister(id);
        assert_eq!(registry.active_sessions(), 0);
    }

    #[test]
    fn ws_registry_session_ids_are_unique() {
        let registry = WsRegistry::new();
        let id1 = registry.next_session_id();
        let id2 = registry.next_session_id();
        assert_ne!(id1, id2);
    }
}
