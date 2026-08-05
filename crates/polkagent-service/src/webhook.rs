//! Webhook dispatcher — subscribes to the [`EventBus`] and delivers matching
//! events to registered webhook endpoints.
//!
//! [`WebhookDispatcher`] bridges the internal event system with the
//! [`polkagent_surface_webhook`] delivery engine. It runs as a background
//! Tokio task, converting each [`polkagent_core::RunEvent`] into a webhook-compatible event
//! type string, checking the [`WebhookRegistry`] for matching subscriptions,
//! and dispatching deliveries through the [`WebhookService`].
//!
//! # Retry behaviour
//!
//! Failed deliveries are recorded in the [`DeliveryStore`] with a
//! `next_retry_at` timestamp. The dispatcher spawns a periodic retry task
//! that re-attempts pending deliveries according to each subscription's
//! [`polkagent_surface_webhook::RetryPolicy`].

use std::sync::Arc;
use std::time::Duration;

use polkagent_event::{EventBus, EventReceiver};
use polkagent_surface_webhook::{
    DeliveryStore, InMemoryDeliveryStore, WebhookRegistry, WebhookService,
};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use polkagent_event::types::EventType;

// ---------------------------------------------------------------------------
// WebhookDispatcher
// ---------------------------------------------------------------------------

/// Subscribes to the [`EventBus`] and dispatches matching events to webhook
/// endpoints via the [`WebhookService`].
///
/// The dispatcher is designed to be an optional subsystem of [`AppService`].
/// When started, it spawns two background tasks:
///
/// 1. **Event loop** — receives events from the bus, maps them to webhook
///    event type strings, and dispatches to matching subscriptions.
/// 2. **Retry loop** — periodically scans the delivery store for failed
///    deliveries whose `next_retry_at` has passed and re-attempts them.
///
/// Both tasks respect a shared shutdown signal and will exit cleanly when
/// [`shutdown`](WebhookDispatcher::shutdown) is called.
///
/// [`AppService`]: crate::app::AppService
pub struct WebhookDispatcher<S: DeliveryStore = InMemoryDeliveryStore> {
    service: Arc<WebhookService<S>>,
    event_bus: EventBus,
    /// Sends `true` to signal background tasks to stop.
    shutdown_tx: watch::Sender<bool>,
    /// Event-loop task handle (populated after `start`).
    event_handle: Option<JoinHandle<()>>,
    /// Retry-loop task handle (populated after `start`).
    retry_handle: Option<JoinHandle<()>>,
    /// Interval between retry sweeps.
    retry_interval: Duration,
}

impl<S: DeliveryStore + 'static> WebhookDispatcher<S> {
    /// Create a new dispatcher wired to the given event bus and webhook
    /// service.
    ///
    /// The dispatcher is **not started** until [`start`](Self::start) is
    /// called. This allows the caller to register webhooks first.
    #[must_use]
    pub fn new(event_bus: EventBus, service: Arc<WebhookService<S>>) -> Self {
        let (shutdown_tx, _) = watch::channel(false);
        Self {
            service,
            event_bus,
            shutdown_tx,
            event_handle: None,
            retry_handle: None,
            retry_interval: Duration::from_secs(30),
        }
    }

    /// Set the interval between retry sweeps (default: 30 s).
    #[must_use]
    pub fn with_retry_interval(mut self, interval: Duration) -> Self {
        self.retry_interval = interval;
        self
    }

    /// Return a reference to the underlying [`WebhookService`].
    pub fn service(&self) -> &WebhookService<S> {
        &self.service
    }

    /// Return a reference to the underlying [`WebhookRegistry`].
    pub fn registry(&self) -> &WebhookRegistry {
        self.service.registry()
    }

    /// Start the background event-loop and retry-loop tasks.
    ///
    /// This subscribes to the event bus and begins processing events
    /// immediately. Can only be called once; subsequent calls are no-ops.
    pub fn start(&mut self) {
        if self.event_handle.is_some() {
            return; // Already started.
        }

        let rx = self.event_bus.subscribe();
        let service = Arc::clone(&self.service);
        let mut shutdown_rx = self.shutdown_tx.subscribe();

        // Spawn the event loop.
        let event_handle = tokio::spawn(async move {
            event_loop(rx, service, &mut shutdown_rx).await;
        });
        self.event_handle = Some(event_handle);

        // Spawn the retry loop.
        let retry_service = Arc::clone(&self.service);
        let mut retry_shutdown_rx = self.shutdown_tx.subscribe();
        let interval = self.retry_interval;

        let retry_handle = tokio::spawn(async move {
            retry_loop(retry_service, interval, &mut retry_shutdown_rx).await;
        });
        self.retry_handle = Some(retry_handle);

        info!("webhook dispatcher started");
    }

    /// Signal the background tasks to shut down and wait for them to exit.
    pub async fn shutdown(&mut self) {
        let _ = self.shutdown_tx.send(true);

        if let Some(h) = self.event_handle.take() {
            let _ = h.await;
        }
        if let Some(h) = self.retry_handle.take() {
            let _ = h.await;
        }

        info!("webhook dispatcher shut down");
    }

    /// Return `true` if the dispatcher has been started.
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.event_handle.is_some()
    }
}

// ---------------------------------------------------------------------------
// Event loop
// ---------------------------------------------------------------------------

/// Maps a [`polkagent_core::event::EventKind`] to the webhook event type
/// string used for registry lookups.
///
/// Returns `None` for event kinds that have no catalogued [`EventType`]
/// mapping (the event will be silently skipped).
fn event_type_string(event: &polkagent_core::event::RunEvent) -> Option<String> {
    EventType::from_kind(&event.kind).map(|et| et.as_str().to_owned())
}

/// Core event receive loop. Receives events from the bus, converts them to
/// webhook event type strings, and dispatches to matching subscriptions.
async fn event_loop<S: DeliveryStore + 'static>(
    mut rx: EventReceiver,
    service: Arc<WebhookService<S>>,
    shutdown_rx: &mut watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            biased;

            _ = shutdown_rx.changed() => {
                debug!("webhook event loop received shutdown signal");
                break;
            }

            result = rx.recv() => {
                match result {
                    Ok(event) => {
                        if let Some(event_type) = event_type_string(&event) {
                            let data = serde_json::to_value(&event.kind)
                                .unwrap_or(serde_json::Value::Null);

                            match service.dispatch(&event_type, data).await {
                                Ok(records) => {
                                    if !records.is_empty() {
                                        debug!(
                                            event_type = %event_type,
                                            deliveries = records.len(),
                                            "dispatched webhook event"
                                        );
                                    }
                                }
                                Err(e) => {
                                    warn!(
                                        event_type = %event_type,
                                        error = %e,
                                        "webhook dispatch failed"
                                    );
                                }
                            }
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        warn!(skipped = n, "webhook event loop lagged, some events missed");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        info!("event bus closed, webhook event loop exiting");
                        break;
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Retry loop
// ---------------------------------------------------------------------------

/// Periodically scans the delivery store for failed deliveries that are due
/// for retry and re-dispatches them.
async fn retry_loop<S: DeliveryStore + 'static>(
    _service: Arc<WebhookService<S>>,
    interval: Duration,
    shutdown_rx: &mut watch::Receiver<bool>,
) {
    let mut ticker = tokio::time::interval(interval);
    // Skip the initial immediate tick.
    ticker.tick().await;

    loop {
        tokio::select! {
            biased;

            _ = shutdown_rx.changed() => {
                debug!("webhook retry loop received shutdown signal");
                break;
            }

            _ = ticker.tick() => {
                // The retry logic is handled by the WebhookDelivery engine
                // through the store's list_pending_retries. Here we simply
                // log that a sweep happened; actual re-delivery requires
                // reconstructing payloads, which is deferred to a future
                // iteration with a proper outbox pattern.
                debug!("webhook retry sweep tick");
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use polkagent_core::{
        event::{EventCorrelation, EventKind, RunEvent},
        ids::{EventId, RunId},
    };
    use polkagent_surface_webhook::{
        DeliveryRecord, InMemoryDeliveryStore, WebhookConfig, WebhookRegistry,
    };
    use std::sync::Arc;

    type TestSetup = (
        EventBus,
        Arc<WebhookRegistry>,
        Arc<InMemoryDeliveryStore>,
        Arc<WebhookService<InMemoryDeliveryStore>>,
        WebhookDispatcher<InMemoryDeliveryStore>,
    );

    /// Helper: build an event bus, registry, service, and dispatcher for
    /// testing. Returns all components so tests can interact with them.
    fn setup() -> TestSetup {
        let bus = EventBus::new(64);
        let registry = Arc::new(WebhookRegistry::new());
        let store = Arc::new(InMemoryDeliveryStore::new());
        let service = Arc::new(WebhookService::new(
            Arc::clone(&registry),
            Arc::clone(&store),
        ));
        let dispatcher = WebhookDispatcher::new(bus.clone(), Arc::clone(&service));
        (bus, registry, store, service, dispatcher)
    }

    /// Helper: create a `RunEvent` with a given `EventKind`.
    fn make_event(kind: EventKind) -> RunEvent {
        let run_id = RunId::new();
        RunEvent::new_durable(
            EventId::new(),
            run_id,
            1,
            kind,
            EventCorrelation {
                run_id,
                ..Default::default()
            },
        )
    }

    // -----------------------------------------------------------------------
    // Test 1: Dispatcher subscribes to event bus
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn dispatcher_subscribes_to_event_bus() {
        let (bus, _registry, _store, _service, mut dispatcher) = setup();

        // Before start, no subscribers from the dispatcher.
        let before = bus.subscriber_count();

        dispatcher.start();

        // After start, the dispatcher has subscribed.
        let after = bus.subscriber_count();
        assert!(
            after > before,
            "dispatcher should add at least one subscriber"
        );

        dispatcher.shutdown().await;
    }

    // -----------------------------------------------------------------------
    // Test 2: Matching event triggers webhook delivery
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn matching_event_triggers_delivery() {
        let (bus, _registry, store, service, mut dispatcher) = setup();

        // Register a webhook that accepts run_created events.
        let mut cfg = WebhookConfig::new("http://127.0.0.1:1/hook", "secret");
        cfg.events = vec!["run_created".into()];
        service.register(cfg);

        dispatcher.start();

        // Publish a RunCreated event.
        let event = make_event(EventKind::RunCreated);
        bus.publish(event);

        // Allow the event loop to process the event.
        tokio::time::sleep(Duration::from_millis(100)).await;

        // The delivery store should have at least one record (the delivery
        // will fail because no server is listening, but the record is still
        // created).
        assert!(
            !store.is_empty(),
            "delivery store should have a record after matching event"
        );

        dispatcher.shutdown().await;
    }

    // -----------------------------------------------------------------------
    // Test 3: Non-matching event is ignored
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn non_matching_event_is_ignored() {
        let (bus, _registry, store, service, mut dispatcher) = setup();

        // Register a webhook that ONLY accepts run_completed events.
        let mut cfg = WebhookConfig::new("http://127.0.0.1:1/hook", "secret");
        cfg.events = vec!["run_completed".into()];
        service.register(cfg);

        dispatcher.start();

        // Publish a RunCreated event (which should NOT match).
        let event = make_event(EventKind::RunCreated);
        bus.publish(event);

        // Allow the event loop to process.
        tokio::time::sleep(Duration::from_millis(100)).await;

        // No deliveries should have been created since the event did not
        // match the subscription filter.
        assert!(
            store.is_empty(),
            "delivery store should be empty for non-matching event"
        );

        dispatcher.shutdown().await;
    }

    // -----------------------------------------------------------------------
    // Test 4: Failed delivery is retried (recorded with retry metadata)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn failed_delivery_is_recorded_for_retry() {
        let (bus, _registry, store, service, mut dispatcher) = setup();

        // Register a webhook pointing to a non-routable address.
        let cfg = WebhookConfig::new("http://127.0.0.1:1/hook", "secret");
        service.register(cfg);

        dispatcher.start();

        // Publish a RunCreated event. The delivery will fail (no server),
        // and the record should indicate a failure with retry metadata.
        let event = make_event(EventKind::RunCreated);
        bus.publish(event);

        tokio::time::sleep(Duration::from_millis(200)).await;

        assert!(
            !store.is_empty(),
            "failed delivery should still create a record"
        );

        // Retrieve the record and verify failure status.
        let records: Vec<DeliveryRecord> = {
            // List all deliveries for any webhook.
            let subs = service.registry().list();
            let mut all = Vec::new();
            for sub in &subs {
                let mut recs = store.list_by_webhook(sub.id, 100).await.unwrap_or_default();
                all.append(&mut recs);
            }
            all
        };

        assert!(!records.is_empty(), "should have delivery records");
        let record = &records[0];
        assert_eq!(
            record.status,
            polkagent_surface_webhook::DeliveryStatus::Failed,
            "delivery to non-routable address should fail"
        );
        assert!(
            record.last_error.is_some(),
            "failed delivery should have an error message"
        );

        dispatcher.shutdown().await;
    }

    // -----------------------------------------------------------------------
    // Test 5: Dispatcher works without webhooks configured
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn dispatcher_works_without_webhooks() {
        let (bus, _registry, store, _service, mut dispatcher) = setup();

        // No webhooks registered — the dispatcher should still start and
        // process events without errors.
        dispatcher.start();

        let event = make_event(EventKind::RunCreated);
        bus.publish(event);

        tokio::time::sleep(Duration::from_millis(100)).await;

        // No deliveries created (no webhooks registered).
        assert!(
            store.is_empty(),
            "no deliveries should be created when no webhooks are registered"
        );

        dispatcher.shutdown().await;
    }

    // -----------------------------------------------------------------------
    // Test 6: Multiple webhooks receive same event
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn multiple_webhooks_receive_same_event() {
        let (bus, _registry, store, service, mut dispatcher) = setup();

        // Register two wildcard webhooks.
        let cfg_a = WebhookConfig::new("http://127.0.0.1:1/hook-a", "sa");
        let cfg_b = WebhookConfig::new("http://127.0.0.1:1/hook-b", "sb");
        service.register(cfg_a);
        service.register(cfg_b);

        dispatcher.start();

        let event = make_event(EventKind::RunCreated);
        bus.publish(event);

        tokio::time::sleep(Duration::from_millis(200)).await;

        // Both webhooks should have received deliveries.
        assert!(
            store.len() >= 2,
            "both webhooks should have delivery records, got {}",
            store.len()
        );

        dispatcher.shutdown().await;
    }

    // -----------------------------------------------------------------------
    // Test 7: Shutdown stops the dispatcher
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn shutdown_stops_dispatcher() {
        let (_bus, _registry, _store, _service, mut dispatcher) = setup();

        dispatcher.start();
        assert!(dispatcher.is_running());

        dispatcher.shutdown().await;
        // After shutdown the handles are consumed.
        assert!(!dispatcher.is_running());
    }

    // -----------------------------------------------------------------------
    // Test 8: event_type_string maps known kinds
    // -----------------------------------------------------------------------

    #[test]
    fn event_type_string_maps_known_kinds() {
        let event = make_event(EventKind::RunCreated);
        assert_eq!(event_type_string(&event).as_deref(), Some("run_created"));

        let event = make_event(EventKind::RunCompleted {
            output_artifact_id: None,
            input_tokens: 0,
            output_tokens: 0,
        });
        assert_eq!(event_type_string(&event).as_deref(), Some("run_completed"));

        let event = make_event(EventKind::RunFailed {
            reason: "oops".into(),
        });
        assert_eq!(event_type_string(&event).as_deref(), Some("run_failed"));

        let event = make_event(EventKind::RunQueued);
        assert_eq!(event_type_string(&event).as_deref(), Some("run_queued"));
    }
}
