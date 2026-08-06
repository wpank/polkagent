//! Black-box coverage for checkpoint-aware durable run-event `WebSocket`s.

#![allow(
    clippy::expect_used,
    clippy::too_many_lines,
    reason = "integration fixtures fail at explicit WebSocket and durability boundaries"
)]

use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};
use std::time::Duration;

use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use polkagent_api::{ApiServer, AppState, InMemoryAgentStore, InMemoryRunManager};
use polkagent_config::Config;
use polkagent_core::{
    event::{EventCorrelation, EventKind, RunEvent},
    EventId, RunId,
};
use polkagent_event::{types::EventType, EventBus};
use polkagent_store_sqlite::{migrations, SqlitePool};
use polkagent_store_trait::event::{EventFilter, EventStore, EventStoreError, StoredEvent};
use sha2::{Digest, Sha256};
use tokio::sync::{Notify, RwLock};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{
        client::IntoClientRequest,
        http::{header::AUTHORIZATION, HeaderValue, StatusCode},
        protocol::frame::coding::CloseCode,
        Error as WebSocketError, Message,
    },
    MaybeTlsStream, WebSocketStream,
};

const PRIVATE_BACKEND_SENTINEL: &str = "PRIVATE_EVENT_STREAM_BACKEND_SENTINEL";

type ClientSocket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

struct LiveServer {
    ws_base_url: String,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for LiveServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn spawn_server(
    config: Config,
    bus: EventBus,
    store: Option<Arc<TestEventStore>>,
) -> LiveServer {
    let pool = SqlitePool::open_in_memory().expect("open effect-store fixture");
    migrations::migrate(&pool.writer()).expect("migrate effect-store fixture");
    let mut state = AppState::new(
        config,
        Arc::new(InMemoryAgentStore::new()),
        Arc::new(InMemoryRunManager::new()),
        Arc::new(pool),
        bus,
    );
    if let Some(store) = store {
        state = state.with_event_store(store);
    }

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind event stream fixture");
    let address = listener.local_addr().expect("event stream address");
    let router = ApiServer::from_state(state).into_router();
    let task = tokio::spawn(async move {
        axum::serve(listener, router)
            .await
            .expect("serve event stream fixture");
    });
    LiveServer {
        ws_base_url: format!("ws://{address}"),
        task,
    }
}

async fn connect(server: &LiveServer, query: &str, bearer: Option<&str>) -> ClientSocket {
    let mut request = format!("{}/api/v1alpha1/events/stream{query}", server.ws_base_url)
        .into_client_request()
        .expect("WebSocket request");
    if let Some(token) = bearer {
        request.headers_mut().insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {token}")).expect("authorization header"),
        );
    }
    connect_async(request)
        .await
        .expect("connect event WebSocket")
        .0
}

async fn next_json(socket: &mut ClientSocket) -> serde_json::Value {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match socket.next().await.expect("WebSocket remains open") {
                Ok(Message::Text(text)) => {
                    return serde_json::from_str(&text).expect("event frame JSON");
                }
                Ok(Message::Ping(payload)) => {
                    socket
                        .send(Message::Pong(payload))
                        .await
                        .expect("reply to ping");
                }
                Ok(Message::Close(frame)) => panic!("unexpected close frame: {frame:?}"),
                Ok(_) => {}
                Err(error) => panic!("WebSocket read failed: {error}"),
            }
        }
    })
    .await
    .expect("event frame delivery")
}

async fn expect_no_frame(socket: &mut ClientSocket) {
    let result = tokio::time::timeout(Duration::from_millis(150), socket.next()).await;
    assert!(
        result.is_err(),
        "unexpected duplicate WebSocket frame: {result:?}"
    );
}

fn http_error_status(error: WebSocketError) -> StatusCode {
    match error {
        WebSocketError::Http(response) => response.status(),
        other => panic!("expected HTTP handshake failure, got {other}"),
    }
}

#[derive(Default)]
struct TestEventStore {
    events: RwLock<Vec<StoredEvent>>,
    reads: RwLock<Vec<(u64, usize)>>,
    read_calls: AtomicUsize,
    fail_on_read: AtomicUsize,
    block_first_read: AtomicBool,
    first_snapshot_taken: Notify,
    release_first_read: Notify,
}

impl TestEventStore {
    async fn insert(&self, event: StoredEvent) {
        self.events.write().await.push(event);
    }

    async fn reads(&self) -> Vec<(u64, usize)> {
        self.reads.read().await.clone()
    }

    fn fail_on_read(&self, call: usize) {
        self.fail_on_read.store(call, Ordering::SeqCst);
    }

    fn block_first_read(&self) {
        self.block_first_read.store(true, Ordering::SeqCst);
    }
}

#[async_trait]
impl EventStore for TestEventStore {
    async fn append_durable(&self, event: StoredEvent) -> Result<StoredEvent, EventStoreError> {
        self.insert(event.clone()).await;
        Ok(event)
    }

    async fn append_diagnostic(
        &self,
        _event: StoredEvent,
        _expires_at: String,
    ) -> Result<(), EventStoreError> {
        Ok(())
    }

    async fn read_from_cursor(
        &self,
        cursor: u64,
        limit: usize,
    ) -> Result<Vec<StoredEvent>, EventStoreError> {
        self.reads.write().await.push((cursor, limit));
        let call = self.read_calls.fetch_add(1, Ordering::SeqCst) + 1;
        if self.fail_on_read.load(Ordering::SeqCst) == call {
            return Err(EventStoreError::Backend(Box::new(std::io::Error::other(
                PRIVATE_BACKEND_SENTINEL,
            ))));
        }

        let page = self
            .events
            .read()
            .await
            .iter()
            .filter(|event| event.global_sequence > cursor)
            .take(limit)
            .cloned()
            .collect();
        if call == 1 && self.block_first_read.load(Ordering::SeqCst) {
            self.first_snapshot_taken.notify_one();
            self.release_first_read.notified().await;
        }
        Ok(page)
    }

    async fn read_run_events(&self, run_id: RunId) -> Result<Vec<StoredEvent>, EventStoreError> {
        Ok(self
            .events
            .read()
            .await
            .iter()
            .filter(|event| event.run_id == run_id.to_string())
            .cloned()
            .collect())
    }

    async fn query(&self, filter: EventFilter) -> Result<Vec<StoredEvent>, EventStoreError> {
        let mut events = self.events.read().await.clone();
        events.retain(|event| {
            filter
                .run_id
                .is_none_or(|run_id| event.run_id == run_id.to_string())
                && (filter.event_types.is_empty() || filter.event_types.contains(&event.event_type))
                && filter
                    .since_global_sequence
                    .is_none_or(|sequence| event.global_sequence >= sequence)
        });
        if let Some(limit) = filter.limit {
            events.truncate(limit);
        }
        Ok(events)
    }

    async fn max_sequence(&self, run_id: RunId) -> Result<u64, EventStoreError> {
        Ok(self
            .events
            .read()
            .await
            .iter()
            .filter(|event| event.run_id == run_id.to_string())
            .map(|event| event.sequence)
            .max()
            .unwrap_or(0))
    }

    async fn has_terminal_event(&self, _run_id: RunId) -> Result<bool, EventStoreError> {
        Ok(false)
    }
}

fn stored_event(global_sequence: u64, run_id: RunId, kind: EventKind) -> (StoredEvent, RunEvent) {
    let id = EventId::new();
    let event_type = EventType::from_kind(&kind)
        .expect("fixture uses catalogued event kind")
        .as_str()
        .to_owned();
    let event = RunEvent::new_durable(
        id,
        run_id,
        global_sequence,
        kind.clone(),
        EventCorrelation {
            run_id,
            ..Default::default()
        },
    );
    let stored = StoredEvent {
        id: id.to_string(),
        event_type,
        sequence: global_sequence,
        global_sequence,
        run_id: run_id.to_string(),
        conversation_id: None,
        correlation_id: id.to_string(),
        causation_id: None,
        scope_id: "test".to_owned(),
        timestamp: event.timestamp.to_rfc3339(),
        durability: "durable".to_owned(),
        payload: serde_json::to_value(kind).expect("serialize fixture event kind"),
        trace_id: None,
        span_id: None,
        schema_version: 1,
    };
    (stored, event)
}

#[tokio::test]
async fn replays_follows_reconnects_and_filters_across_bounded_pages() {
    let target_run = RunId::new();
    let unrelated_run = RunId::new();
    let store = Arc::new(TestEventStore::default());
    for sequence in 1..=258 {
        let (stored, _) = if sequence == 1 || sequence == 258 {
            stored_event(sequence, target_run, EventKind::RunCreated)
        } else {
            stored_event(sequence, unrelated_run, EventKind::RunQueued)
        };
        store.insert(stored).await;
    }
    let bus = EventBus::new(8);
    let server = spawn_server(Config::default(), bus.clone(), Some(store.clone())).await;
    let query = format!("?after_sequence=0&run_id={target_run}&kinds=run_created");
    let mut socket = connect(&server, &query, None).await;

    let first = next_json(&mut socket).await;
    let across_page_boundary = next_json(&mut socket).await;
    assert_eq!(first["global_sequence"], 1);
    assert_eq!(across_page_boundary["global_sequence"], 258);
    assert_eq!(first["run_id"], target_run.to_string());
    assert_eq!(across_page_boundary["run_id"], target_run.to_string());
    let reads = store.reads().await;
    assert_eq!(reads[0], (0, 256));
    assert_eq!(reads[1], (256, 256));
    assert!(reads.iter().all(|(_, limit)| *limit == 256));
    socket.close(None).await.expect("close initial socket");

    let (reconnected_stored, _) = stored_event(259, target_run, EventKind::RunCreated);
    store.insert(reconnected_stored).await;
    let reconnect_query = format!("?after_sequence=258&run_id={target_run}&kinds=run_created");
    let mut resumed = connect(&server, &reconnect_query, None).await;
    assert_eq!(next_json(&mut resumed).await["global_sequence"], 259);

    let (live_stored, live) = stored_event(260, target_run, EventKind::RunCreated);
    store.insert(live_stored).await;
    bus.publish(live);
    assert_eq!(next_json(&mut resumed).await["global_sequence"], 260);
    expect_no_frame(&mut resumed).await;
}

#[tokio::test]
async fn forced_broadcast_lag_recovers_after_last_durable_checkpoint_without_duplicates() {
    let run_id = RunId::new();
    let store = Arc::new(TestEventStore::default());
    for sequence in 1..=2 {
        let (stored, _) = stored_event(sequence, run_id, EventKind::RunCreated);
        store.insert(stored).await;
    }
    store.block_first_read();
    let bus = EventBus::new(2);
    let server = spawn_server(Config::default(), bus.clone(), Some(store.clone())).await;
    let mut socket = connect(&server, "?after_sequence=0", None).await;
    store.first_snapshot_taken.notified().await;

    for sequence in 3..=6 {
        let (stored, live) = stored_event(sequence, run_id, EventKind::RunCreated);
        store.insert(stored).await;
        bus.publish(live);
    }
    store.release_first_read.notify_one();

    let mut delivered = Vec::new();
    for _ in 1..=6 {
        delivered.push(
            next_json(&mut socket).await["global_sequence"]
                .as_u64()
                .expect("durable checkpoint"),
        );
    }
    assert_eq!(delivered, (1..=6).collect::<Vec<_>>());
    expect_no_frame(&mut socket).await;
    let reads = store.reads().await;
    assert_eq!(reads[0], (0, 256));
    assert_eq!(reads[1], (2, 256));
    assert!(reads.iter().all(|(_, limit)| *limit == 256));
}

#[tokio::test]
async fn backend_failure_closes_explicitly_without_exposing_backend_detail() {
    let run_id = RunId::new();
    let store = Arc::new(TestEventStore::default());
    let (first_stored, _) = stored_event(1, run_id, EventKind::RunCreated);
    store.insert(first_stored).await;
    store.block_first_read();
    store.fail_on_read(2);
    let bus = EventBus::new(1);
    let server = spawn_server(Config::default(), bus.clone(), Some(store.clone())).await;
    let mut socket = connect(&server, "?after_sequence=0", None).await;
    store.first_snapshot_taken.notified().await;
    for sequence in 2..=4 {
        let (stored, live) = stored_event(sequence, run_id, EventKind::RunCreated);
        store.insert(stored).await;
        bus.publish(live);
    }
    store.release_first_read.notify_one();
    assert_eq!(next_json(&mut socket).await["global_sequence"], 1);

    let message = tokio::time::timeout(Duration::from_secs(5), socket.next())
        .await
        .expect("explicit close delivery")
        .expect("close frame")
        .expect("valid close frame");
    let Message::Close(Some(frame)) = message else {
        panic!("expected explicit close frame, got {message:?}");
    };
    assert_eq!(frame.code, CloseCode::Error);
    assert_eq!(frame.reason, "durable event recovery unavailable");
    assert!(!frame.reason.contains(PRIVATE_BACKEND_SENTINEL));
    assert_eq!(store.reads().await[..2], [(0, 256), (1, 256)]);
}

#[tokio::test]
async fn validates_checkpoint_requires_durable_store_and_preserves_auth() {
    let store = Arc::new(TestEventStore::default());
    let run_id = RunId::new();
    let (stored, _) = stored_event(1, run_id, EventKind::RunCreated);
    store.insert(stored).await;
    let token = "event-stream-secret";
    let mut config = Config::default();
    config.auth.enabled = true;
    config.auth.api_keys = vec![format!("{:x}", Sha256::digest(token.as_bytes()))];
    let server = spawn_server(config, EventBus::new(4), Some(store)).await;

    let unauthorized = connect_async(format!(
        "{}/api/v1alpha1/events/stream?after_sequence=0",
        server.ws_base_url
    ))
    .await
    .expect_err("missing authentication must reject upgrade");
    assert_eq!(http_error_status(unauthorized), StatusCode::UNAUTHORIZED);

    let invalid = {
        let mut request = format!(
            "{}/api/v1alpha1/events/stream?after_sequence=-1",
            server.ws_base_url
        )
        .into_client_request()
        .expect("invalid-checkpoint request");
        request.headers_mut().insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {token}")).expect("authorization header"),
        );
        connect_async(request)
            .await
            .expect_err("negative checkpoint must reject upgrade")
    };
    assert_eq!(http_error_status(invalid), StatusCode::UNPROCESSABLE_ENTITY);

    let mut authenticated = connect(&server, "?after_sequence=0", Some(token)).await;
    assert_eq!(next_json(&mut authenticated).await["global_sequence"], 1);

    let missing_store_server = spawn_server(Config::default(), EventBus::new(4), None).await;
    let unavailable = connect_async(format!(
        "{}/api/v1alpha1/events/stream?after_sequence=0",
        missing_store_server.ws_base_url
    ))
    .await
    .expect_err("missing durable store must reject upgrade");
    assert_eq!(http_error_status(unavailable), StatusCode::NOT_IMPLEMENTED);
}
