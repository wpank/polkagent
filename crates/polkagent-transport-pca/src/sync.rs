//! C3: Cross-device message synchronisation and app-layer acknowledgements.
//!
//! This module provides two cooperating mechanisms:
//!
//! ## App-Layer ACK with Sequence Numbers
//!
//! [`AppAck`] extends the transport-level delivery receipts with
//! application-layer semantics:
//!
//! - **Individual ACK** — acknowledges a single sequence number.
//! - **Cumulative ACK** — acknowledges all messages up to and including a
//!   given sequence number (like TCP).
//! - Each ACK carries the originating `device_id` so the sender can track
//!   which devices have seen a message.
//!
//! ## Cross-Device Sync
//!
//! [`DeviceSyncState`] tracks per-device high-water marks and pending ACKs
//! so that a single identity operating across multiple devices can
//! synchronise its view of a conversation.
//!
//! [`SyncClock`] aggregates the sync state for all known devices belonging
//! to an identity.
//!
//! # External Infrastructure Required
//!
//! Full cross-device sync requires a **relay** or **mailbox** service that
//! holds messages for offline devices. This module provides the local state
//! tracking; the actual relay transport is out of scope and would be
//! plugged in via a [`crate::device_channels::DeviceChannelSet`] subscription
//! combined with persistent storage.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::error::PcaError;

// ---------------------------------------------------------------------------
// App-layer ACK
// ---------------------------------------------------------------------------

/// An application-layer acknowledgement with sequence numbers (C3).
///
/// Unlike transport-level delivery receipts which only confirm that the
/// message reached the transport layer, an `AppAck` confirms that the
/// receiving **application** has processed the message.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AppAck {
    /// The sequence number being acknowledged.
    pub sequence: u64,
    /// The device that is acknowledging.
    pub device_id: String,
    /// Epoch-millisecond timestamp of the acknowledgement.
    pub timestamp_ms: u64,
    /// If `true`, this ACK covers all sequences `<= self.sequence`.
    /// If `false`, it is a selective ACK for exactly `self.sequence`.
    pub cumulative: bool,
}

/// A message envelope annotated with sync metadata for cross-device delivery.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncEnvelope {
    /// The originating device.
    pub device_id: String,
    /// Monotonic per-device sequence number.
    pub sync_sequence: u64,
    /// Optional piggy-backed ACK (acknowledges previously received messages).
    pub ack: Option<AppAck>,
    /// The opaque message payload.
    pub payload: Vec<u8>,
}

// ---------------------------------------------------------------------------
// Per-device sync state
// ---------------------------------------------------------------------------

/// Tracks the synchronisation state of a single device.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeviceSyncState {
    /// The device identifier.
    pub device_id: String,
    /// Highest sequence number received from (or sent to) this device.
    pub sequence_high_water: u64,
    /// Highest sequence number for which a cumulative ACK has been received.
    pub acked_through: u64,
    /// Selectively ACKed sequence numbers above `acked_through`.
    pub selective_acks: BTreeSet<u64>,
    /// Epoch-millisecond timestamp of the last sync event.
    pub last_sync_ms: Option<u64>,
}

impl DeviceSyncState {
    /// Create a fresh sync state for a device.
    pub fn new(device_id: impl Into<String>) -> Self {
        Self {
            device_id: device_id.into(),
            sequence_high_water: 0,
            acked_through: 0,
            selective_acks: BTreeSet::new(),
            last_sync_ms: None,
        }
    }

    /// Record that a message with the given sequence was received.
    pub fn record_received(&mut self, sequence: u64, now_ms: u64) {
        if sequence > self.sequence_high_water {
            self.sequence_high_water = sequence;
        }
        self.last_sync_ms = Some(now_ms);
    }

    /// Apply an ACK to this device's state.
    pub fn apply_ack(&mut self, ack: &AppAck) {
        if ack.cumulative {
            if ack.sequence > self.acked_through {
                self.acked_through = ack.sequence;
                // Remove any selective ACKs that are now covered.
                self.selective_acks = self.selective_acks.split_off(&(ack.sequence + 1));
            }
        } else if ack.sequence > self.acked_through {
            self.selective_acks.insert(ack.sequence);
        }
        self.last_sync_ms = Some(ack.timestamp_ms);
    }

    /// Check whether a specific sequence number has been acknowledged.
    pub fn is_acked(&self, sequence: u64) -> bool {
        sequence <= self.acked_through || self.selective_acks.contains(&sequence)
    }

    /// Return the set of sequences that are received but not yet acknowledged.
    ///
    /// This is the range `(acked_through, sequence_high_water]` minus
    /// selectively acked sequences.
    pub fn pending_ack_count(&self) -> u64 {
        if self.sequence_high_water <= self.acked_through {
            return 0;
        }
        let range = self.sequence_high_water - self.acked_through;
        let selective = self
            .selective_acks
            .range((self.acked_through + 1)..=self.sequence_high_water)
            .count() as u64;
        range - selective
    }
}

// ---------------------------------------------------------------------------
// Sync clock (multi-device)
// ---------------------------------------------------------------------------

/// Aggregates sync state across all devices belonging to an identity.
///
/// This is the local bookkeeping needed for cross-device synchronisation.
/// The actual message relay/storage is provided by external infrastructure.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SyncClock {
    /// Per-device sync state, keyed by device_id.
    devices: BTreeMap<String, DeviceSyncState>,
    /// Monotonic sequence counter for locally generated sync envelopes.
    next_sync_sequence: u64,
}

impl SyncClock {
    /// Create a new empty sync clock.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a device. Returns `true` if the device was newly added.
    pub fn register_device(&mut self, device_id: impl Into<String>) -> bool {
        let device_id = device_id.into();
        if self.devices.contains_key(&device_id) {
            return false;
        }
        self.devices
            .insert(device_id.clone(), DeviceSyncState::new(device_id));
        true
    }

    /// Remove a device. Returns `true` if it existed.
    pub fn remove_device(&mut self, device_id: &str) -> bool {
        self.devices.remove(device_id).is_some()
    }

    /// Return the number of registered devices.
    pub fn device_count(&self) -> usize {
        self.devices.len()
    }

    /// Get the sync state of a specific device.
    pub fn device_state(&self, device_id: &str) -> Option<&DeviceSyncState> {
        self.devices.get(device_id)
    }

    /// Record an incoming message from a device.
    pub fn record_received(
        &mut self,
        device_id: &str,
        sequence: u64,
        now_ms: u64,
    ) -> Result<(), PcaError> {
        let state = self
            .devices
            .get_mut(device_id)
            .ok_or_else(|| PcaError::SyncError {
                reason: format!("unknown device: {device_id}"),
            })?;
        state.record_received(sequence, now_ms);
        Ok(())
    }

    /// Apply an ACK from a device.
    pub fn apply_ack(&mut self, ack: &AppAck) -> Result<(), PcaError> {
        let state = self
            .devices
            .get_mut(&ack.device_id)
            .ok_or_else(|| PcaError::SyncError {
                reason: format!("unknown device: {}", ack.device_id),
            })?;
        state.apply_ack(ack);
        debug!(
            device = %ack.device_id,
            sequence = ack.sequence,
            cumulative = ack.cumulative,
            "applied app-layer ACK"
        );
        Ok(())
    }

    /// Create a [`SyncEnvelope`] wrapping a payload with sync metadata.
    ///
    /// Optionally piggy-backs an ACK for the given device.
    pub fn wrap_payload(
        &mut self,
        local_device_id: &str,
        payload: Vec<u8>,
        piggybacked_ack: Option<AppAck>,
    ) -> SyncEnvelope {
        let seq = self.next_sync_sequence;
        self.next_sync_sequence += 1;

        SyncEnvelope {
            device_id: local_device_id.to_string(),
            sync_sequence: seq,
            ack: piggybacked_ack,
            payload,
        }
    }

    /// Process an incoming [`SyncEnvelope`].
    ///
    /// Records the message receipt and applies the piggy-backed ACK (if any).
    /// Returns the extracted payload.
    pub fn receive_envelope(
        &mut self,
        envelope: &SyncEnvelope,
        now_ms: u64,
    ) -> Result<Vec<u8>, PcaError> {
        // Register device on-the-fly if unknown (lenient mode).
        if !self.devices.contains_key(&envelope.device_id) {
            self.register_device(envelope.device_id.clone());
        }

        self.record_received(&envelope.device_id, envelope.sync_sequence, now_ms)?;

        if let Some(ref ack) = envelope.ack {
            // The piggy-backed ACK might reference a different device.
            if self.devices.contains_key(&ack.device_id) {
                self.apply_ack(ack)?;
            }
        }

        Ok(envelope.payload.clone())
    }

    /// Return a serializable snapshot of the full sync state.
    pub fn snapshot(&self) -> SyncClockSnapshot {
        SyncClockSnapshot {
            devices: self.devices.values().cloned().collect(),
            next_sync_sequence: self.next_sync_sequence,
        }
    }

    /// Restore from a snapshot.
    pub fn restore(snapshot: &SyncClockSnapshot) -> Self {
        let mut devices = BTreeMap::new();
        for state in &snapshot.devices {
            devices.insert(state.device_id.clone(), state.clone());
        }
        Self {
            devices,
            next_sync_sequence: snapshot.next_sync_sequence,
        }
    }

    /// Build a cumulative ACK for a device up to its high-water mark.
    pub fn build_cumulative_ack(&self, device_id: &str, now_ms: u64) -> Option<AppAck> {
        let state = self.devices.get(device_id)?;
        if state.sequence_high_water == 0 {
            return None;
        }
        Some(AppAck {
            sequence: state.sequence_high_water,
            device_id: device_id.to_string(),
            timestamp_ms: now_ms,
            cumulative: true,
        })
    }
}

/// Serializable snapshot of a [`SyncClock`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncClockSnapshot {
    /// Per-device sync states.
    pub devices: Vec<DeviceSyncState>,
    /// Next sync sequence number.
    pub next_sync_sequence: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- AppAck tests --------------------------------------------------------

    #[test]
    fn app_ack_serializes() {
        let ack = AppAck {
            sequence: 42,
            device_id: "dev-1".into(),
            timestamp_ms: 1_000_000,
            cumulative: true,
        };
        let json = serde_json::to_string(&ack).expect("serialize");
        let back: AppAck = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, ack);
    }

    // -- DeviceSyncState tests -----------------------------------------------

    #[test]
    fn sync_state_tracks_received() {
        let mut state = DeviceSyncState::new("dev-1");
        state.record_received(1, 100);
        state.record_received(3, 200);
        assert_eq!(state.sequence_high_water, 3);
        assert_eq!(state.last_sync_ms, Some(200));
    }

    #[test]
    fn cumulative_ack_advances_acked_through() {
        let mut state = DeviceSyncState::new("dev-1");
        state.record_received(5, 100);
        state.apply_ack(&AppAck {
            sequence: 3,
            device_id: "dev-1".into(),
            timestamp_ms: 200,
            cumulative: true,
        });
        assert_eq!(state.acked_through, 3);
        assert!(state.is_acked(1));
        assert!(state.is_acked(3));
        assert!(!state.is_acked(4));
    }

    #[test]
    fn selective_ack_tracks_individual_sequences() {
        let mut state = DeviceSyncState::new("dev-1");
        state.record_received(5, 100);
        state.apply_ack(&AppAck {
            sequence: 5,
            device_id: "dev-1".into(),
            timestamp_ms: 200,
            cumulative: false,
        });
        assert!(!state.is_acked(4));
        assert!(state.is_acked(5));
    }

    #[test]
    fn cumulative_ack_clears_covered_selective_acks() {
        let mut state = DeviceSyncState::new("dev-1");
        state.record_received(10, 100);

        // Selective ACKs for 3 and 5.
        state.apply_ack(&AppAck {
            sequence: 3,
            device_id: "dev-1".into(),
            timestamp_ms: 100,
            cumulative: false,
        });
        state.apply_ack(&AppAck {
            sequence: 5,
            device_id: "dev-1".into(),
            timestamp_ms: 100,
            cumulative: false,
        });
        assert_eq!(state.selective_acks.len(), 2);

        // Cumulative ACK through 5 clears both.
        state.apply_ack(&AppAck {
            sequence: 5,
            device_id: "dev-1".into(),
            timestamp_ms: 200,
            cumulative: true,
        });
        assert_eq!(state.acked_through, 5);
        assert!(state.selective_acks.is_empty());
    }

    #[test]
    fn pending_ack_count_correct() {
        let mut state = DeviceSyncState::new("dev-1");
        state.record_received(5, 100);
        assert_eq!(state.pending_ack_count(), 5); // 1..=5

        state.apply_ack(&AppAck {
            sequence: 3,
            device_id: "dev-1".into(),
            timestamp_ms: 200,
            cumulative: true,
        });
        assert_eq!(state.pending_ack_count(), 2); // 4, 5

        state.apply_ack(&AppAck {
            sequence: 4,
            device_id: "dev-1".into(),
            timestamp_ms: 300,
            cumulative: false,
        });
        assert_eq!(state.pending_ack_count(), 1); // 5
    }

    #[test]
    fn pending_ack_count_zero_when_all_acked() {
        let mut state = DeviceSyncState::new("dev-1");
        state.record_received(3, 100);
        state.apply_ack(&AppAck {
            sequence: 3,
            device_id: "dev-1".into(),
            timestamp_ms: 200,
            cumulative: true,
        });
        assert_eq!(state.pending_ack_count(), 0);
    }

    // -- SyncClock tests -----------------------------------------------------

    #[test]
    fn sync_clock_register_and_remove() {
        let mut clock = SyncClock::new();
        assert!(clock.register_device("dev-1"));
        assert!(!clock.register_device("dev-1")); // duplicate
        assert_eq!(clock.device_count(), 1);

        assert!(clock.remove_device("dev-1"));
        assert!(!clock.remove_device("dev-1"));
        assert_eq!(clock.device_count(), 0);
    }

    #[test]
    fn sync_clock_wrap_and_receive() {
        let mut clock = SyncClock::new();
        clock.register_device("dev-a");
        clock.register_device("dev-b");

        let env = clock.wrap_payload("dev-a", b"hello".to_vec(), None);
        assert_eq!(env.sync_sequence, 0);
        assert_eq!(env.device_id, "dev-a");

        let payload = clock.receive_envelope(&env, 1000).expect("receive");
        assert_eq!(payload, b"hello");

        let state = clock.device_state("dev-a").expect("state");
        assert_eq!(state.sequence_high_water, 0);
    }

    #[test]
    fn sync_clock_piggyback_ack() {
        let mut clock = SyncClock::new();
        clock.register_device("dev-a");
        clock.register_device("dev-b");

        // dev-a sends with a piggyback ack for dev-b's sequence 5.
        let ack = AppAck {
            sequence: 5,
            device_id: "dev-b".into(),
            timestamp_ms: 1000,
            cumulative: true,
        };
        let env = clock.wrap_payload("dev-a", b"msg".to_vec(), Some(ack));
        clock.receive_envelope(&env, 2000).expect("receive");

        let dev_b = clock.device_state("dev-b").expect("dev-b");
        assert_eq!(dev_b.acked_through, 5);
    }

    #[test]
    fn sync_clock_unknown_device_recorded() {
        let mut clock = SyncClock::new();
        let result = clock.record_received("unknown", 1, 100);
        assert!(result.is_err());
    }

    #[test]
    fn sync_clock_auto_registers_on_receive() {
        let mut clock = SyncClock::new();
        let env = SyncEnvelope {
            device_id: "new-dev".into(),
            sync_sequence: 0,
            ack: None,
            payload: b"first".to_vec(),
        };
        // Should auto-register the device.
        let payload = clock.receive_envelope(&env, 100).expect("receive");
        assert_eq!(payload, b"first");
        assert_eq!(clock.device_count(), 1);
    }

    #[test]
    fn sync_clock_sequence_increments() {
        let mut clock = SyncClock::new();
        let e1 = clock.wrap_payload("dev-a", b"m1".to_vec(), None);
        let e2 = clock.wrap_payload("dev-a", b"m2".to_vec(), None);
        assert_eq!(e1.sync_sequence, 0);
        assert_eq!(e2.sync_sequence, 1);
    }

    #[test]
    fn sync_clock_build_cumulative_ack() {
        let mut clock = SyncClock::new();
        clock.register_device("dev-a");
        clock.record_received("dev-a", 10, 100).expect("record");

        let ack = clock.build_cumulative_ack("dev-a", 200).expect("ack");
        assert_eq!(ack.sequence, 10);
        assert!(ack.cumulative);
        assert_eq!(ack.timestamp_ms, 200);
    }

    #[test]
    fn sync_clock_no_ack_for_empty_device() {
        let mut clock = SyncClock::new();
        clock.register_device("dev-a");
        assert!(clock.build_cumulative_ack("dev-a", 100).is_none());
    }

    #[test]
    fn sync_clock_snapshot_roundtrip() {
        let mut clock = SyncClock::new();
        clock.register_device("dev-a");
        clock.register_device("dev-b");
        clock.record_received("dev-a", 5, 100).expect("record");
        let _ = clock.wrap_payload("dev-a", b"x".to_vec(), None);
        let _ = clock.wrap_payload("dev-a", b"y".to_vec(), None);

        let snap = clock.snapshot();
        let json = serde_json::to_string(&snap).expect("serialize");
        let back: SyncClockSnapshot = serde_json::from_str(&json).expect("deserialize");
        let restored = SyncClock::restore(&back);

        assert_eq!(restored.device_count(), 2);
        assert_eq!(
            restored
                .device_state("dev-a")
                .expect("a")
                .sequence_high_water,
            5
        );
    }

    #[test]
    fn sync_envelope_serializes() {
        let env = SyncEnvelope {
            device_id: "dev-1".into(),
            sync_sequence: 7,
            ack: Some(AppAck {
                sequence: 3,
                device_id: "dev-2".into(),
                timestamp_ms: 500,
                cumulative: true,
            }),
            payload: b"hello".to_vec(),
        };
        let json = serde_json::to_string(&env).expect("serialize");
        let back: SyncEnvelope = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.sync_sequence, 7);
        assert!(back.ack.is_some());
    }

    #[test]
    fn device_sync_state_serializes() {
        let mut state = DeviceSyncState::new("dev-1");
        state.record_received(10, 100);
        state.apply_ack(&AppAck {
            sequence: 5,
            device_id: "dev-1".into(),
            timestamp_ms: 200,
            cumulative: true,
        });
        state.apply_ack(&AppAck {
            sequence: 8,
            device_id: "dev-1".into(),
            timestamp_ms: 300,
            cumulative: false,
        });

        let json = serde_json::to_string(&state).expect("serialize");
        let back: DeviceSyncState = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.acked_through, 5);
        assert!(back.selective_acks.contains(&8));
        assert_eq!(back.sequence_high_water, 10);
    }
}
