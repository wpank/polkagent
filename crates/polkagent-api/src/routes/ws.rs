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
//! Durable event envelopes include an opaque `cursor` such as `"v1:42"`.
//! Reconnect with a valid query token and `?cursor=v1:42`, send every intended
//! subscribe command, then send `{"msg_type":"ready"}`. The explicit barrier
//! prevents a connection-global replay from advancing before the complete
//! initial subscription set is installed. The server then replays matching
//! durable events strictly after that checkpoint before following live
//! delivery. Omitting `cursor` preserves the legacy live-only behavior and
//! does not require `ready`.
//!
//! # Channels
//!
//! - `runs:{run_id}` — events for a specific run
//! - `agents:{agent_id}` — events for all runs belonging to an agent
//!
//! # Keepalive
//!
//! The server sends a WebSocket `Ping` frame every 30 seconds. If the client
//! does not respond with a `Pong` within 30 seconds, the connection is closed.

use std::collections::{HashSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{close_code, CloseFrame, Message, WebSocket};
use axum::{
    extract::{Query, State, WebSocketUpgrade},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use futures::{SinkExt, StreamExt};
use polkagent_core::{event::Durability, AgentId, RunId};
use polkagent_store_trait::event::{EventStore, StoredEvent};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::sync::broadcast;
use tokio::time::Instant;
use tracing::{debug, trace, warn};

use crate::{
    error::ApiError,
    routes::events::{stored_event_to_run_event, REPLAY_PAGE_SIZE},
    run::RunManagerTrait,
    state::AppState,
};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Interval between server-sent Ping frames.
const PING_INTERVAL: Duration = Duration::from_secs(30);

/// How long to wait for a Pong response before closing the connection.
const PONG_TIMEOUT: Duration = Duration::from_secs(30);

/// Maximum number of distinct channels retained by one connection.
const MAX_SUBSCRIPTIONS: usize = 256;

/// Version prefix for opaque reconnect checkpoint tokens.
const RECONNECT_CURSOR_VERSION: &str = "v1";

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
}

impl Channel {
    /// Parse a channel string into a [`Channel`] variant.
    ///
    /// Returns `None` if the string is not a recognised channel format.
    pub fn parse(s: &str) -> Option<Self> {
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
    /// Opaque reconnect checkpoint on durable event messages.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
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
            cursor: None,
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

/// Validated opaque reconnect checkpoint for the command WebSocket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ReconnectCursor(u64);

impl ReconnectCursor {
    fn parse(value: &str) -> Result<Self, ApiError> {
        let Some(sequence) = value.strip_prefix(&format!("{RECONNECT_CURSOR_VERSION}:")) else {
            return Err(invalid_reconnect_cursor());
        };
        if sequence.is_empty()
            || (sequence.len() > 1 && sequence.starts_with('0'))
            || !sequence.bytes().all(|byte| byte.is_ascii_digit())
        {
            return Err(invalid_reconnect_cursor());
        }
        sequence
            .parse::<u64>()
            .map(Self)
            .map_err(|_| invalid_reconnect_cursor())
    }

    fn encode(sequence: u64) -> String {
        format!("{RECONNECT_CURSOR_VERSION}:{sequence}")
    }

    const fn sequence(self) -> u64 {
        self.0
    }
}

fn invalid_reconnect_cursor() -> ApiError {
    ApiError::ValidationError("reconnect cursor is malformed, stale, or from the future".to_owned())
}

async fn validate_reconnect_cursor(
    store: &dyn EventStore,
    cursor: ReconnectCursor,
) -> Result<(), ApiError> {
    let sequence = cursor.sequence();
    if sequence == 0 {
        return Ok(());
    }
    let page = store
        .read_from_cursor(sequence - 1, 1)
        .await
        .map_err(|error| {
            warn!(
                error = %error,
                "WebSocket reconnect cursor validation failed"
            );
            ApiError::Unavailable("durable reconnect cursor validation unavailable".to_owned())
        })?;
    if page.len() > 1 {
        warn!(
            returned = page.len(),
            "WebSocket reconnect cursor validation exceeded its page bound"
        );
        return Err(ApiError::InternalError(
            "durable reconnect cursor validation failed".to_owned(),
        ));
    }
    if page
        .first()
        .is_none_or(|event| event.global_sequence != sequence)
    {
        return Err(invalid_reconnect_cursor());
    }
    Ok(())
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
    Subscribe { id: Option<String>, channel: String },
    /// Unsubscribe from a channel.
    Unsubscribe { id: Option<String>, channel: String },
    /// Seal the reconnect subscription set and start durable replay.
    Ready { id: Option<String> },
    /// Client ping — server responds with `"pong"`.
    Ping { id: Option<String> },
}

// ---------------------------------------------------------------------------
// WsSession — per-connection subscription state
// ---------------------------------------------------------------------------

/// Per-connection WebSocket session state.
///
/// Tracks the set of channels the client has subscribed to.
#[derive(Debug)]
pub struct WsSession {
    /// Active subscriptions for this connection.
    subscriptions: HashSet<Channel>,
    /// Whether the client has been authenticated.
    authenticated: bool,
    /// Whether event delivery may advance the connection-global checkpoint.
    event_delivery_ready: bool,
    /// Whether a reconnect cursor requires an explicit subscription barrier.
    reconnect_barrier: bool,
}

impl Default for WsSession {
    fn default() -> Self {
        Self {
            subscriptions: HashSet::new(),
            authenticated: false,
            event_delivery_ready: true,
            reconnect_barrier: false,
        }
    }
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
            event_delivery_ready: true,
            reconnect_barrier: false,
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

    fn require_reconnect_ready(&mut self) {
        self.event_delivery_ready = false;
        self.reconnect_barrier = true;
    }

    fn mark_event_delivery_ready(&mut self) {
        self.event_delivery_ready = true;
    }

    const fn is_event_delivery_ready(&self) -> bool {
        self.event_delivery_ready
    }

    fn reconnect_subscription_set_is_sealed(&self) -> bool {
        self.reconnect_barrier && self.event_delivery_ready
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

    /// Clone the small per-session subscription set for one event-loop poll.
    fn subscription_snapshot(&self) -> HashSet<Channel> {
        self.subscriptions.clone()
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
// Durable in-session recovery
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CommandStreamFailure {
    Backend,
    CheckpointUnavailable,
    InvalidProjection,
    Closed,
}

impl CommandStreamFailure {
    fn code(self) -> &'static str {
        match self {
            Self::Backend => "event_store_backend",
            Self::CheckpointUnavailable => "durable_checkpoint_unavailable",
            Self::InvalidProjection => "invalid_event_projection",
            Self::Closed => "event_bus_closed",
        }
    }

    fn safe_reason(self) -> &'static str {
        match self {
            Self::Backend => "durable event recovery unavailable",
            Self::CheckpointUnavailable => "durable event checkpoint unavailable",
            Self::InvalidProjection => "durable event recovery invalid",
            Self::Closed => "event stream closed",
        }
    }
}

struct CommandOutboundEvent {
    event: polkagent_core::event::RunEvent,
    durable_checkpoint: Option<u64>,
}

/// Tracks one command socket's durable position without changing its wire
/// protocol. A legacy connection without a reconnect cursor remains live-only:
/// its first successfully observed durable bus event establishes the in-session
/// checkpoint.
struct CommandEventFollower {
    store: Arc<dyn EventStore>,
    run_manager: Arc<dyn RunManagerTrait>,
    receiver: polkagent_event::EventReceiver,
    durable_checkpoint: Option<u64>,
    pending: VecDeque<StoredEvent>,
    live_pending: Option<polkagent_core::event::RunEvent>,
    replay_required: bool,
    recovery_target: Option<u64>,
}

impl CommandEventFollower {
    fn new(
        store: Arc<dyn EventStore>,
        run_manager: Arc<dyn RunManagerTrait>,
        receiver: polkagent_event::EventReceiver,
        reconnect_cursor: Option<ReconnectCursor>,
    ) -> Self {
        Self {
            store,
            run_manager,
            receiver,
            durable_checkpoint: reconnect_cursor.map(ReconnectCursor::sequence),
            pending: VecDeque::new(),
            live_pending: None,
            replay_required: reconnect_cursor.is_some(),
            recovery_target: None,
        }
    }

    fn acknowledge(&mut self, checkpoint: Option<u64>) {
        if let Some(checkpoint) = checkpoint {
            self.durable_checkpoint = Some(checkpoint);
        }
    }

    async fn next(
        &mut self,
        subscriptions: &HashSet<Channel>,
    ) -> Result<CommandOutboundEvent, CommandStreamFailure> {
        loop {
            if let Some(stored) = self.pending.front().cloned() {
                let checkpoint = stored.global_sequence;
                if self.matches(subscriptions, &stored.run_id).await? {
                    let event = stored_event_to_run_event(stored)
                        .map_err(|_| CommandStreamFailure::InvalidProjection)?;
                    self.pending.pop_front();
                    return Ok(CommandOutboundEvent {
                        event,
                        durable_checkpoint: Some(checkpoint),
                    });
                }
                self.pending.pop_front();
                self.durable_checkpoint = Some(checkpoint);
                self.clear_satisfied_target();
                continue;
            }

            if let Some(event) = self.live_pending.clone() {
                if event.durability == Durability::Durable {
                    let stored = self
                        .store
                        .get_event_by_id(&event.id.to_string())
                        .await
                        .map_err(|_| CommandStreamFailure::Backend)?;
                    let checkpoint = stored.global_sequence;
                    if checkpoint == 0 {
                        return Err(CommandStreamFailure::InvalidProjection);
                    }
                    let projected = stored_event_to_run_event(stored)
                        .map_err(|_| CommandStreamFailure::InvalidProjection)?;
                    if projected.id != event.id
                        || projected.run_id != event.run_id
                        || projected.sequence != event.sequence
                        || projected.kind != event.kind
                    {
                        return Err(CommandStreamFailure::InvalidProjection);
                    }

                    if let Some(current) = self.durable_checkpoint {
                        self.live_pending = None;
                        if checkpoint <= current {
                            continue;
                        }
                        self.recovery_target = Some(checkpoint);
                        self.replay_required = true;
                        continue;
                    }

                    let matches = self
                        .matches(subscriptions, &event.run_id.to_string())
                        .await?;
                    self.live_pending = None;
                    if matches {
                        return Ok(CommandOutboundEvent {
                            event,
                            durable_checkpoint: Some(checkpoint),
                        });
                    }
                    self.durable_checkpoint = Some(checkpoint);
                    continue;
                }

                if self
                    .matches(subscriptions, &event.run_id.to_string())
                    .await?
                {
                    self.live_pending = None;
                    return Ok(CommandOutboundEvent {
                        event,
                        durable_checkpoint: None,
                    });
                }
                self.live_pending = None;
                continue;
            }

            self.clear_satisfied_target();
            if self.replay_required {
                self.load_replay_page().await?;
                if !self.pending.is_empty() || self.replay_required {
                    continue;
                }
                if self.recovery_target.is_some() {
                    return Err(CommandStreamFailure::InvalidProjection);
                }
            }

            match self.receiver.recv().await {
                Ok(event) => {
                    self.live_pending = Some(event);
                }
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    let Some(checkpoint) = self.durable_checkpoint else {
                        warn!(
                            skipped,
                            "WebSocket v1alpha1 lagged before a durable checkpoint"
                        );
                        return Err(CommandStreamFailure::CheckpointUnavailable);
                    };
                    warn!(
                        skipped,
                        durable_checkpoint = checkpoint,
                        "WebSocket v1alpha1 lagged; replaying durable events"
                    );
                    self.replay_required = true;
                }
                Err(broadcast::error::RecvError::Closed) => {
                    return Err(CommandStreamFailure::Closed);
                }
            }
        }
    }

    async fn load_replay_page(&mut self) -> Result<(), CommandStreamFailure> {
        let checkpoint = self
            .durable_checkpoint
            .ok_or(CommandStreamFailure::CheckpointUnavailable)?;
        let page = self
            .store
            .read_from_cursor(checkpoint, REPLAY_PAGE_SIZE)
            .await
            .map_err(|_| CommandStreamFailure::Backend)?;
        if page.len() > REPLAY_PAGE_SIZE {
            return Err(CommandStreamFailure::InvalidProjection);
        }

        let mut previous = checkpoint;
        for event in &page {
            if event.global_sequence <= previous {
                return Err(CommandStreamFailure::InvalidProjection);
            }
            previous = event.global_sequence;
        }
        self.replay_required = page.len() == REPLAY_PAGE_SIZE;
        self.pending.extend(page);
        Ok(())
    }

    fn clear_satisfied_target(&mut self) {
        if self
            .recovery_target
            .is_some_and(|target| self.durable_checkpoint.is_some_and(|seen| seen >= target))
        {
            self.recovery_target = None;
        }
    }

    async fn matches(
        &mut self,
        subscriptions: &HashSet<Channel>,
        run_id: &str,
    ) -> Result<bool, CommandStreamFailure> {
        if subscriptions.is_empty() {
            return Ok(false);
        }
        let run_id = run_id
            .parse::<RunId>()
            .map_err(|_| CommandStreamFailure::InvalidProjection)?;
        if subscriptions.contains(&Channel::Run(run_id)) {
            return Ok(true);
        }
        if !subscriptions
            .iter()
            .any(|channel| matches!(channel, Channel::Agent(_)))
        {
            return Ok(false);
        }

        let agent_id = self
            .run_manager
            .get_run(run_id)
            .await
            .map_err(|_| CommandStreamFailure::InvalidProjection)?
            .agent_id;
        Ok(subscriptions.contains(&Channel::Agent(agent_id)))
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
    /// Opaque durable checkpoint emitted by an earlier event envelope.
    pub cursor: Option<String>,
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
// Token validation
// ---------------------------------------------------------------------------

/// Validate `token` against the auth configuration stored in `state`.
///
/// When auth is **disabled** (`config.auth.enabled = false`), any non-empty
/// token is accepted (development / testing convenience).
///
/// When auth is **enabled**, the token is SHA-256 hashed and compared against
/// the pre-hashed entries in `config.auth.api_keys`.  An empty token is always
/// rejected.
fn validate_ws_token(token: &str, state: &AppState) -> bool {
    if token.is_empty() {
        return false;
    }

    let auth = &state.config.auth;

    if !auth.enabled {
        // Auth disabled — accept any non-empty token.
        return true;
    }

    // Auth enabled — hash the token and compare against stored digests.
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    let digest = format!("{:x}", hasher.finalize());
    auth.api_keys.iter().any(|h| h == &digest)
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
/// When `config.auth.enabled` is `true`, the token is validated by SHA-256
/// hash against `config.auth.api_keys`. When disabled, any non-empty token
/// is accepted for development convenience. A reconnect cursor always requires
/// a valid query token so unauthenticated callers cannot probe durable history;
/// first-message authentication remains available for no-cursor connections.
pub async fn ws_handler(
    State(state): State<AppState>,
    Query(query): Query<WsQuery>,
    ws: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    let query_token_valid = query
        .token
        .as_deref()
        .is_some_and(|token| validate_ws_token(token, &state));
    if query.cursor.is_some() && !query_token_valid {
        let body = serde_json::json!({
            "error": {
                "code": "UNAUTHORIZED",
                "message": "valid query token required for reconnect cursor"
            }
        });
        return Ok((StatusCode::UNAUTHORIZED, Json(body)).into_response());
    }
    let store = state
        .event_store
        .clone()
        .ok_or_else(|| ApiError::NotImplemented("event store not configured".to_owned()))?;
    let reconnect_cursor = query
        .cursor
        .as_deref()
        .map(ReconnectCursor::parse)
        .transpose()?;
    if let Some(cursor) = reconnect_cursor {
        validate_reconnect_cursor(store.as_ref(), cursor).await?;
    }
    // Check if a token was provided as query parameter and validate it.
    let pre_authenticated = query_token_valid;

    debug!(pre_authenticated, "WebSocket v1alpha1 upgrade accepted");

    let mut session = if pre_authenticated {
        WsSession::authenticated()
    } else {
        WsSession::new()
    };
    if reconnect_cursor.is_some() {
        session.require_reconnect_ready();
    }
    // Attach before the upgrade task begins. Any durable event that races with
    // subscription commands remains observable for checkpointing or recovery.
    let event_rx = state.event_bus.subscribe();
    let run_manager = state.run_manager.clone();

    Ok(ws
        .on_upgrade(move |socket| {
            handle_ws_session(
                socket,
                session,
                state,
                CommandEventFollower::new(store, run_manager, event_rx, reconnect_cursor),
            )
        })
        .into_response())
}

/// Drive a single WebSocket connection: handle subscribe/unsubscribe messages,
/// forward matching events from the `EventBus`, and maintain keepalive.
async fn handle_ws_session(
    socket: WebSocket,
    mut session: WsSession,
    state: AppState,
    mut events: CommandEventFollower,
) {
    let (mut sender, mut receiver) = socket.split();

    let mut ping_interval = tokio::time::interval(PING_INTERVAL);
    // Consume the immediate first tick so pings start after PING_INTERVAL.
    ping_interval.tick().await;

    // Track when the last ping was sent so we can detect pong timeouts.
    let mut ping_sent_at: Option<Instant> = None;
    let mut waiting_for_pong = false;

    loop {
        let subscriptions = session.subscription_snapshot();
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
                        waiting_for_pong = false;
                    }
                    Some(Ok(Message::Ping(data))) => {
                        // Echo back a Pong.
                        if sender.send(Message::Pong(data)).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Text(text))) => {
                        let reply = handle_client_text(&text, &mut session, &|t| {
                            validate_ws_token(t, &state)
                        });
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
            result = events.next(&subscriptions), if session.is_event_delivery_ready() && !subscriptions.is_empty() => {
                match result {
                    Ok(outbound) => {
                        let channel_str = format!("runs:{}", outbound.event.run_id);
                        let Ok(payload) = serde_json::to_value(&outbound.event) else {
                            let error = CommandStreamFailure::InvalidProjection;
                            warn!(
                                error_code = error.code(),
                                "WebSocket v1alpha1 event serialization failed"
                            );
                            send_stream_failure(&mut sender, error).await;
                            break;
                        };

                        let mut msg = WsMessage::new("event", None, Some(channel_str), payload);
                        msg.cursor = outbound
                            .durable_checkpoint
                            .map(ReconnectCursor::encode);
                        let Ok(json) = msg.to_json() else {
                            let error = CommandStreamFailure::InvalidProjection;
                            warn!(
                                error_code = error.code(),
                                "WebSocket v1alpha1 envelope serialization failed"
                            );
                            send_stream_failure(&mut sender, error).await;
                            break;
                        };
                        if sender.send(Message::Text(json.into())).await.is_err() {
                            debug!("WebSocket v1alpha1: send failed (client disconnected)");
                            break;
                        }
                        events.acknowledge(outbound.durable_checkpoint);
                    }
                    Err(CommandStreamFailure::Closed) => {
                        debug!("WebSocket v1alpha1: EventBus closed");
                        break;
                    }
                    Err(error) => {
                        warn!(
                            error_code = error.code(),
                            durable_checkpoint = ?events.durable_checkpoint,
                            "WebSocket v1alpha1 durable event recovery failed"
                        );
                        send_stream_failure(&mut sender, error).await;
                        break;
                    }
                }
            }

            // ── Keepalive ping timer ─────────────────────────────────────
            _ = ping_interval.tick() => {
                // Check pong timeout from the previous ping.
                if waiting_for_pong {
                    if let Some(t) = ping_sent_at {
                        if t.elapsed() > PONG_TIMEOUT {
                            debug!("WebSocket v1alpha1: pong timeout; closing connection");
                            break;
                        }
                    }
                }

                // Send ping.
                if sender.send(Message::Ping(vec![].into())).await.is_err() {
                    debug!("WebSocket v1alpha1: ping send failed");
                    break;
                }
                trace!("WebSocket v1alpha1: sent Ping");
                ping_sent_at = Some(Instant::now());
                waiting_for_pong = true;
            }
        }
    }

    debug!("WebSocket v1alpha1: session handler exiting");
}

async fn send_stream_failure(
    sender: &mut futures::stream::SplitSink<WebSocket, Message>,
    error: CommandStreamFailure,
) {
    let protocol_error = WsMessage::error(None, error.safe_reason());
    if let Ok(json) = protocol_error.to_json() {
        let _ = sender.send(Message::Text(json.into())).await;
    }
    let _ = sender
        .send(Message::Close(Some(CloseFrame {
            code: close_code::ERROR,
            reason: error.safe_reason().into(),
        })))
        .await;
}

/// Parse and dispatch a client text frame.
///
/// Returns the reply message to send to the client.
///
/// `token_valid` is a predicate that returns `true` when a given token string
/// should be accepted.  Pass `|t| validate_ws_token(t, &state)` in production
/// and a simple closure in tests.
fn handle_client_text(
    text: &str,
    session: &mut WsSession,
    token_valid: &dyn Fn(&str) -> bool,
) -> WsMessage {
    let client_msg: ClientMessage = match serde_json::from_str(text) {
        Ok(m) => m,
        Err(e) => {
            debug!(error = %e, "WS: failed to parse client message");
            return WsMessage::error(None, "invalid JSON or unknown msg_type");
        }
    };

    match client_msg {
        ClientMessage::Auth { token } => {
            let valid = token.as_deref().is_some_and(token_valid);
            if valid {
                session.authenticate();
                WsMessage::ack(None, None)
            } else {
                WsMessage::error(None, "invalid or missing token")
            }
        }

        ClientMessage::Subscribe { id, channel } => {
            if !session.is_authenticated() {
                return WsMessage::error(id, "not authenticated");
            }
            match Channel::parse(&channel) {
                Some(ch) => {
                    if session.reconnect_subscription_set_is_sealed() && !session.is_subscribed(&ch)
                    {
                        return WsMessage::error(
                            id,
                            "reconnect subscription set is sealed after ready",
                        );
                    }
                    if !session.is_subscribed(&ch)
                        && session.subscription_count() >= MAX_SUBSCRIPTIONS
                    {
                        return WsMessage::error(
                            id,
                            format!("subscription limit exceeded (max {MAX_SUBSCRIPTIONS})"),
                        );
                    }
                    session.subscribe(ch);
                    WsMessage::ack(id, Some(channel))
                }
                None => WsMessage::error(id, format!("unknown channel: {channel}")),
            }
        }

        ClientMessage::Ready { id } => {
            if !session.is_authenticated() {
                return WsMessage::error(id, "not authenticated");
            }
            if session.subscription_count() == 0 {
                return WsMessage::error(id, "at least one subscription required before ready");
            }
            session.mark_event_delivery_ready();
            WsMessage::ack(id, None)
        }

        ClientMessage::Unsubscribe { id, channel } => {
            if !session.is_authenticated() {
                return WsMessage::error(id, "not authenticated");
            }
            match Channel::parse(&channel) {
                Some(ch) => {
                    session.unsubscribe(&ch);
                    WsMessage::ack(id, Some(channel))
                }
                None => WsMessage::error(id, format!("unknown channel: {channel}")),
            }
        }

        ClientMessage::Ping { id } => {
            // Application-level ping; send a pong.
            let mut pong = WsMessage::pong();
            pong.id = id;
            pong
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "WebSocket protocol tests intentionally fail fast on malformed fixture data"
)]
mod tests {
    use super::*;
    use polkagent_core::{AgentId, RunId};

    // ── Channel parsing ─────────────────────────────────────────────────────

    #[test]
    fn channel_rejects_producer_less_system_channel() {
        assert_eq!(Channel::parse("system"), None);
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
    }

    #[test]
    fn reconnect_cursor_is_versioned_and_canonical() {
        assert_eq!(
            ReconnectCursor::parse("v1:0").expect("zero cursor"),
            ReconnectCursor(0)
        );
        assert_eq!(
            ReconnectCursor::parse("v1:42").expect("positive cursor"),
            ReconnectCursor(42)
        );
        assert_eq!(ReconnectCursor::encode(42), "v1:42");
        for invalid in ["", "42", "v2:42", "v1:", "v1:-1", "v1:+1", "v1:01"] {
            assert!(ReconnectCursor::parse(invalid).is_err(), "{invalid}");
        }
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
        let run_id = RunId::new();
        let msg = WsMessage::new(
            "event",
            Some("id-1".into()),
            Some(format!("runs:{run_id}")),
            serde_json::json!({ "foo": "bar" }),
        );
        let json = msg.to_json().expect("serialize");
        assert!(json.contains("\"msg_type\":\"event\""));
        assert!(json.contains(&format!("\"channel\":\"runs:{run_id}\"")));
        assert!(!json.contains("\"cursor\""));
    }

    #[test]
    fn ws_message_deserializes_from_json() {
        let json = r#"{
            "msg_type": "event",
            "id": "r1",
            "channel": null,
            "payload": {},
            "timestamp": "2024-01-01T00:00:00Z"
        }"#;
        let msg: WsMessage = serde_json::from_str(json).expect("deserialize");
        assert_eq!(msg.msg_type, "event");
        assert_eq!(msg.id, Some("r1".into()));
        assert!(msg.cursor.is_none());
    }

    #[test]
    fn ws_message_serializes_versioned_cursor_additively() {
        let mut msg = WsMessage::new("event", None, None, Value::Null);
        msg.cursor = Some(ReconnectCursor::encode(7));
        let json = msg.to_json().expect("serialize cursor envelope");
        assert!(json.contains("\"cursor\":\"v1:7\""));
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
        let channel = Channel::Run(RunId::new());
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
    fn ws_session_unrelated_agent_channel_does_not_match_run_events() {
        let mut session = WsSession::authenticated();
        let run_id = RunId::new();

        session.subscribe(Channel::Agent(AgentId::new()));

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
        let run_id = RunId::new();
        let json = format!(r#"{{"msg_type":"subscribe","id":"r1","channel":"runs:{run_id}"}}"#);
        let msg: ClientMessage = serde_json::from_str(&json).expect("parse");
        assert!(
            matches!(msg, ClientMessage::Subscribe { ref channel, .. } if channel == &format!("runs:{run_id}"))
        );
    }

    #[test]
    fn client_message_unsubscribe_parses() {
        let run_id = RunId::new();
        let json = format!(r#"{{"msg_type":"unsubscribe","id":"r2","channel":"runs:{run_id}"}}"#);
        let msg: ClientMessage = serde_json::from_str(&json).expect("parse");
        assert!(matches!(msg, ClientMessage::Unsubscribe { .. }));
    }

    #[test]
    fn client_message_ready_parses() {
        let json = r#"{"msg_type":"ready","id":"r3"}"#;
        let msg: ClientMessage = serde_json::from_str(json).expect("parse");
        assert!(matches!(msg, ClientMessage::Ready { id: Some(ref i) } if i == "r3"));
    }

    #[test]
    fn client_message_ping_parses() {
        let json = r#"{"msg_type":"ping","id":"p1"}"#;
        let msg: ClientMessage = serde_json::from_str(json).expect("parse");
        assert!(matches!(msg, ClientMessage::Ping { id: Some(ref i) } if i == "p1"));
    }

    // ── handle_client_text ──────────────────────────────────────────────────

    /// Convenience: a token validator that accepts any non-empty token (auth disabled).
    fn any_nonempty(t: &str) -> bool {
        !t.is_empty()
    }

    /// Convenience: a token validator that only accepts the literal "mytoken".
    fn only_mytoken(t: &str) -> bool {
        t == "mytoken"
    }

    #[test]
    fn handle_text_invalid_json_returns_error() {
        let mut session = WsSession::authenticated();
        let msg = handle_client_text("not json", &mut session, &any_nonempty);
        assert_eq!(msg.msg_type, "error");
    }

    #[test]
    fn handle_text_auth_with_valid_token() {
        let mut session = WsSession::new();
        assert!(!session.is_authenticated());
        let json = r#"{"msg_type":"auth","token":"mytoken"}"#;
        let msg = handle_client_text(json, &mut session, &only_mytoken);
        assert!(session.is_authenticated());
        assert_eq!(msg.msg_type, "ack");
    }

    #[test]
    fn handle_text_auth_with_empty_token_fails() {
        let mut session = WsSession::new();
        let json = r#"{"msg_type":"auth","token":""}"#;
        let msg = handle_client_text(json, &mut session, &any_nonempty);
        assert!(!session.is_authenticated());
        assert_eq!(msg.msg_type, "error");
    }

    #[test]
    fn handle_text_auth_with_wrong_token_fails() {
        let mut session = WsSession::new();
        let json = r#"{"msg_type":"auth","token":"wrong-token"}"#;
        // Validator only accepts "mytoken"; "wrong-token" should fail.
        let msg = handle_client_text(json, &mut session, &only_mytoken);
        assert!(!session.is_authenticated());
        assert_eq!(msg.msg_type, "error");
    }

    #[test]
    fn handle_text_subscribe_without_auth_returns_error() {
        let mut session = WsSession::new();
        let run_id = RunId::new();
        let json = format!(r#"{{"msg_type":"subscribe","channel":"runs:{run_id}"}}"#);
        let msg = handle_client_text(&json, &mut session, &any_nonempty);
        assert_eq!(msg.msg_type, "error");
        assert!(!session.is_subscribed(&Channel::Run(run_id)));
    }

    #[test]
    fn handle_text_rejects_producer_less_system_channel() {
        let mut session = WsSession::authenticated();
        let json = r#"{"msg_type":"subscribe","id":"req-1","channel":"system"}"#;
        let msg = handle_client_text(json, &mut session, &any_nonempty);
        assert_eq!(msg.msg_type, "error");
        assert_eq!(msg.id, Some("req-1".into()));
        assert_eq!(msg.payload["reason"], "unknown channel: system");
        assert_eq!(session.subscription_count(), 0);
    }

    #[test]
    fn handle_text_subscribe_to_run_channel() {
        let mut session = WsSession::authenticated();
        let run_id = RunId::new();
        let channel = format!("runs:{run_id}");
        let json = format!(r#"{{"msg_type":"subscribe","channel":"{channel}"}}"#);
        let msg = handle_client_text(&json, &mut session, &any_nonempty);
        assert_eq!(msg.msg_type, "ack");
        assert!(session.is_subscribed(&Channel::Run(run_id)));
    }

    #[test]
    fn handle_text_subscribe_to_agent_channel() {
        let mut session = WsSession::authenticated();
        let agent_id = AgentId::new();
        let channel = format!("agents:{agent_id}");
        let json = format!(r#"{{"msg_type":"subscribe","channel":"{channel}"}}"#);
        let msg = handle_client_text(&json, &mut session, &any_nonempty);
        assert_eq!(msg.msg_type, "ack");
        assert!(session.is_subscribed(&Channel::Agent(agent_id)));
    }

    #[test]
    fn handle_text_ready_requires_auth_and_subscription_then_seals_reconnect_set() {
        let mut unauthenticated = WsSession::new();
        let ready = r#"{"msg_type":"ready","id":"ready"}"#;
        let error = handle_client_text(ready, &mut unauthenticated, &any_nonempty);
        assert_eq!(error.payload["reason"], "not authenticated");

        let mut session = WsSession::authenticated();
        session.require_reconnect_ready();
        let error = handle_client_text(ready, &mut session, &any_nonempty);
        assert_eq!(
            error.payload["reason"],
            "at least one subscription required before ready"
        );
        assert!(!session.is_event_delivery_ready());

        let run_id = RunId::new();
        let subscribe = format!(r#"{{"msg_type":"subscribe","channel":"runs:{run_id}"}}"#);
        assert_eq!(
            handle_client_text(&subscribe, &mut session, &any_nonempty).msg_type,
            "ack"
        );
        assert_eq!(
            handle_client_text(ready, &mut session, &any_nonempty).msg_type,
            "ack"
        );
        assert!(session.is_event_delivery_ready());

        let late = format!(
            r#"{{"msg_type":"subscribe","channel":"runs:{}"}}"#,
            RunId::new()
        );
        let error = handle_client_text(&late, &mut session, &any_nonempty);
        assert_eq!(error.msg_type, "error");
        assert_eq!(
            error.payload["reason"],
            "reconnect subscription set is sealed after ready"
        );
    }

    #[test]
    fn handle_text_subscribe_to_unknown_channel_returns_error() {
        let mut session = WsSession::authenticated();
        let json = r#"{"msg_type":"subscribe","channel":"bogus:channel"}"#;
        let msg = handle_client_text(json, &mut session, &any_nonempty);
        assert_eq!(msg.msg_type, "error");
        assert_eq!(session.subscription_count(), 0);
    }

    #[test]
    fn handle_text_enforces_subscription_limit_but_allows_duplicate() {
        let mut session = WsSession::authenticated();
        let mut first_channel = None;
        for index in 0..MAX_SUBSCRIPTIONS {
            let run_id = RunId::new();
            let channel = format!("runs:{run_id}");
            first_channel.get_or_insert_with(|| channel.clone());
            let json = format!(r#"{{"msg_type":"subscribe","channel":"{channel}"}}"#);
            let message = handle_client_text(&json, &mut session, &any_nonempty);
            assert_eq!(message.msg_type, "ack", "subscription {index}");
        }

        let duplicate = format!(
            r#"{{"msg_type":"subscribe","channel":"{}"}}"#,
            first_channel.expect("first channel")
        );
        assert_eq!(
            handle_client_text(&duplicate, &mut session, &any_nonempty).msg_type,
            "ack"
        );

        let overflow = format!(
            r#"{{"msg_type":"subscribe","id":"overflow","channel":"runs:{}"}}"#,
            RunId::new()
        );
        let message = handle_client_text(&overflow, &mut session, &any_nonempty);
        assert_eq!(message.msg_type, "error");
        assert_eq!(message.id.as_deref(), Some("overflow"));
        assert_eq!(
            message.payload["reason"],
            format!("subscription limit exceeded (max {MAX_SUBSCRIPTIONS})")
        );
        assert_eq!(session.subscription_count(), MAX_SUBSCRIPTIONS);
    }

    #[test]
    fn handle_text_unsubscribe_removes_subscription() {
        let mut session = WsSession::authenticated();
        let run_id = RunId::new();
        let channel = Channel::Run(run_id);
        session.subscribe(channel.clone());
        let json = format!(r#"{{"msg_type":"unsubscribe","channel":"runs:{run_id}"}}"#);
        let msg = handle_client_text(&json, &mut session, &any_nonempty);
        assert_eq!(msg.msg_type, "ack");
        assert!(!session.is_subscribed(&channel));
    }

    #[test]
    fn handle_text_ping_returns_pong() {
        let mut session = WsSession::authenticated();
        let json = r#"{"msg_type":"ping","id":"ping-1"}"#;
        let msg = handle_client_text(json, &mut session, &any_nonempty);
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
