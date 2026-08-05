//! Metadata drift watcher — periodically checks for runtime metadata drift
//! and emits `MetadataDriftDetected` events via the event bus.
//!
//! [`MetadataDriftWatcher`] runs as a background Tokio task, using the
//! [`MetadataService`] to detect drift on configured chains. When drift is
//! detected, it publishes a [`RunEvent`] with [`EventKind::MetadataDriftDetected`]
//! to the [`EventBus`].
//!
//! The watcher is detect-and-propose only — it never auto-merges metadata.

use std::time::Duration;

use polkagent_core::event::{Durability, EventCorrelation, EventKind, RunEvent};
use polkagent_core::ids::{EventId, RunId};
use polkagent_event::EventBus;
use polkagent_metadata::{ChainId, MetadataDrift, MetadataService};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

// ---------------------------------------------------------------------------
// MetadataDriftWatcher
// ---------------------------------------------------------------------------

/// Background watcher that periodically checks for metadata drift on
/// configured chains and publishes alerts via the event bus.
///
/// This is an optional subsystem of [`AppService`](crate::app::AppService).
/// When started, it spawns a single background task that polls the
/// [`MetadataService`] for drift on all configured chain IDs at a
/// configurable interval.
///
/// The watcher is detect-and-propose only: it emits
/// [`EventKind::MetadataDriftDetected`] events but never auto-merges or
/// auto-pins metadata.
pub struct MetadataDriftWatcher {
    metadata_service: MetadataService,
    event_bus: EventBus,
    chain_ids: Vec<ChainId>,
    poll_interval: Duration,
    /// Sends `true` to signal the background task to stop.
    shutdown_tx: watch::Sender<bool>,
    /// Task handle (populated after `start`).
    handle: Option<JoinHandle<()>>,
}

impl MetadataDriftWatcher {
    /// Create a new watcher.
    ///
    /// The watcher is **not started** until [`start`](Self::start) is called.
    #[must_use]
    pub fn new(
        metadata_service: MetadataService,
        event_bus: EventBus,
        chain_ids: Vec<ChainId>,
        poll_interval: Duration,
    ) -> Self {
        let (shutdown_tx, _) = watch::channel(false);
        Self {
            metadata_service,
            event_bus,
            chain_ids,
            poll_interval,
            shutdown_tx,
            handle: None,
        }
    }

    /// Start the background drift-check task.
    ///
    /// Can only be called once; subsequent calls are no-ops.
    pub fn start(&mut self) {
        if self.handle.is_some() {
            return;
        }

        let metadata_service = self.metadata_service.clone();
        let event_bus = self.event_bus.clone();
        let chain_ids = self.chain_ids.clone();
        let poll_interval = self.poll_interval;
        let mut shutdown_rx = self.shutdown_tx.subscribe();

        let handle = tokio::spawn(async move {
            drift_check_loop(
                metadata_service,
                event_bus,
                chain_ids,
                poll_interval,
                &mut shutdown_rx,
            )
            .await;
        });
        self.handle = Some(handle);

        info!(
            chains = self.chain_ids.len(),
            interval_secs = self.poll_interval.as_secs(),
            "metadata drift watcher started"
        );
    }

    /// Signal the background task to shut down and wait for it to exit.
    pub async fn shutdown(&mut self) {
        let _ = self.shutdown_tx.send(true);
        if let Some(h) = self.handle.take() {
            let _ = h.await;
        }
        info!("metadata drift watcher shut down");
    }

    /// Return `true` if the watcher has been started.
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.handle.is_some()
    }

    /// Return a reference to the underlying [`MetadataService`].
    pub fn metadata_service(&self) -> &MetadataService {
        &self.metadata_service
    }

    /// Return the list of monitored chain IDs.
    pub fn chain_ids(&self) -> &[ChainId] {
        &self.chain_ids
    }
}

// ---------------------------------------------------------------------------
// Background loop
// ---------------------------------------------------------------------------

/// Core polling loop: checks each chain for drift at the specified interval.
async fn drift_check_loop(
    metadata_service: MetadataService,
    event_bus: EventBus,
    chain_ids: Vec<ChainId>,
    poll_interval: Duration,
    shutdown_rx: &mut watch::Receiver<bool>,
) {
    let mut ticker = tokio::time::interval(poll_interval);
    // Skip the initial immediate tick so we don't check immediately on startup.
    ticker.tick().await;

    loop {
        tokio::select! {
            biased;

            _ = shutdown_rx.changed() => {
                debug!("metadata drift watcher received shutdown signal");
                break;
            }

            _ = ticker.tick() => {
                for chain_id in &chain_ids {
                    if let Some(drift) = metadata_service.check_drift(chain_id) {
                        warn!(
                            chain = %drift.chain_id,
                            pinned = %drift.pinned_hash,
                            current = %drift.current_hash,
                            "metadata drift detected by watcher"
                        );
                        publish_drift_event(&event_bus, &drift);
                    } else {
                        debug!(chain = %chain_id, "no metadata drift");
                    }
                }
            }
        }
    }
}

/// Publish a `MetadataDriftDetected` event to the event bus.
fn publish_drift_event(event_bus: &EventBus, drift: &MetadataDrift) {
    let run_id = RunId::new();
    let event = RunEvent {
        id: EventId::new(),
        run_id,
        sequence: 0,
        kind: EventKind::MetadataDriftDetected {
            chain_id: drift.chain_id.to_string(),
            pinned_hash: drift.pinned_hash.to_string(),
            current_hash: drift.current_hash.to_string(),
        },
        durability: Durability::Durable,
        correlation: EventCorrelation {
            run_id,
            ..Default::default()
        },
        causation_id: None,
        timestamp: chrono::Utc::now(),
    };
    event_bus.publish(event);
}

// ---------------------------------------------------------------------------
// On-demand check (for `polkagent doctor`)
// ---------------------------------------------------------------------------

/// Check all configured chains for metadata drift on demand.
///
/// Returns a list of `(chain_id, MetadataDrift)` tuples for chains where
/// drift was detected. Used by `polkagent doctor` and similar diagnostic
/// commands.
pub fn check_drift_all(
    metadata_service: &MetadataService,
    chain_ids: &[ChainId],
) -> Vec<MetadataDrift> {
    let mut drifts = Vec::new();
    for chain_id in chain_ids {
        if let Some(drift) = metadata_service.check_drift(chain_id) {
            drifts.push(drift);
        }
    }
    drifts
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use polkagent_core::now;
    use polkagent_metadata::{MetadataService, MetadataSnapshot, MetadataVersion};

    fn make_snapshot(chain: &str, data: &[u8]) -> MetadataSnapshot {
        MetadataSnapshot::new(
            ChainId::new(chain),
            MetadataVersion::V14,
            data.to_vec(),
            now(),
            1,
        )
    }

    #[test]
    fn check_drift_all_returns_empty_when_no_drift() {
        let svc = MetadataService::new();
        let chain = ChainId::new("polkadot");

        svc.register_snapshot(make_snapshot("polkadot", b"v1"));
        svc.pin_current(&chain, "v1").expect("pin");

        let drifts = check_drift_all(&svc, &[chain]);
        assert!(drifts.is_empty());
    }

    #[test]
    fn check_drift_all_detects_drift() {
        let svc = MetadataService::new();
        let chain = ChainId::new("polkadot");

        svc.register_snapshot(make_snapshot("polkadot", b"v1"));
        svc.pin_current(&chain, "v1").expect("pin");
        svc.register_snapshot(make_snapshot("polkadot", b"v2"));

        let drifts = check_drift_all(&svc, &[chain]);
        assert_eq!(drifts.len(), 1);
        assert_eq!(drifts[0].chain_id, ChainId::new("polkadot"));
    }

    #[test]
    fn check_drift_all_multiple_chains() {
        let svc = MetadataService::new();
        let polkadot = ChainId::new("polkadot");
        let kusama = ChainId::new("kusama");

        // Polkadot: pinned v1, current v2 = drift
        svc.register_snapshot(make_snapshot("polkadot", b"v1"));
        svc.pin_current(&polkadot, "v1").expect("pin");
        svc.register_snapshot(make_snapshot("polkadot", b"v2"));

        // Kusama: no pins = no drift
        svc.register_snapshot(make_snapshot("kusama", b"ksm"));

        let drifts = check_drift_all(&svc, &[polkadot, kusama]);
        assert_eq!(drifts.len(), 1);
    }

    #[test]
    fn publish_drift_event_does_not_panic() {
        let bus = EventBus::new(16);
        let drift = MetadataDrift {
            chain_id: ChainId::new("polkadot"),
            pinned_hash: polkagent_metadata::MetadataHash::from_bytes(b"old"),
            current_hash: polkagent_metadata::MetadataHash::from_bytes(b"new"),
            detected_at: now(),
            affected_pallets: vec!["Balances".into()],
        };
        publish_drift_event(&bus, &drift);
    }

    #[tokio::test]
    async fn watcher_starts_and_shuts_down() {
        let svc = MetadataService::new();
        let bus = EventBus::new(16);
        let chains = vec![ChainId::new("polkadot")];

        let mut watcher = MetadataDriftWatcher::new(svc, bus, chains, Duration::from_millis(50));

        assert!(!watcher.is_running());
        watcher.start();
        assert!(watcher.is_running());

        watcher.shutdown().await;
        assert!(!watcher.is_running());
    }

    #[tokio::test]
    async fn watcher_publishes_drift_event() {
        let svc = MetadataService::new();
        let chain = ChainId::new("polkadot");

        // Set up drift: pin v1, register v2
        svc.register_snapshot(make_snapshot("polkadot", b"v1"));
        svc.pin_current(&chain, "v1").expect("pin");
        svc.register_snapshot(make_snapshot("polkadot", b"v2"));

        let bus = EventBus::new(64);
        let mut rx = bus.subscribe();

        let mut watcher =
            MetadataDriftWatcher::new(svc, bus, vec![chain], Duration::from_millis(50));
        watcher.start();

        // Wait for at least one poll cycle.
        tokio::time::sleep(Duration::from_millis(120)).await;

        // Should have received a drift event.
        let event = rx.try_recv();
        assert!(event.is_ok(), "should have received a drift event");
        let event = event.expect("event");
        match &event.kind {
            EventKind::MetadataDriftDetected { chain_id, .. } => {
                assert_eq!(chain_id, "polkadot");
            }
            other => panic!("unexpected event kind: {other:?}"),
        }

        watcher.shutdown().await;
    }
}
