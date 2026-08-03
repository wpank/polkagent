//! Crash-safe persistence for PCA transport state.
//!
//! [`PersistentState`] captures everything needed to restore a PCA transport
//! after an unexpected shutdown:
//!
//! - **Session material** — encrypted session keys, nonce counters, peer
//!   addresses, and session metadata.
//! - **Dedup markers** — delivery IDs that have already been processed.
//! - **Pending turns** — accepted but not-yet-completed processing work.
//! - **Outbound lanes** — messages queued for sending but not yet delivered.
//! - **Device channel subscriptions** — per-device channel routing state.
//!
//! The [`save_state`] and [`load_state`] functions serialize to / deserialize
//! from JSON, which can be written to a file or any other durable store.

use serde::{Deserialize, Serialize};

use crate::dedup::PendingTurn;
use crate::device_channels::DeviceChannelSnapshot;

/// Serializable session material for crash recovery.
///
/// Contains the minimum state needed to resume an encrypted session without
/// re-performing the handshake. Note that the actual shared secret bytes are
/// included — callers must ensure the serialized form is stored securely.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionPersistence {
    /// The unique session ID.
    pub session_id: String,
    /// The local SS58 address.
    pub local_address: String,
    /// The remote peer's SS58 address.
    pub peer_address: String,
    /// The shared secret bytes (32 bytes).
    pub shared_secret: Vec<u8>,
    /// Current outgoing (encrypt) nonce counter value.
    pub encrypt_nonce_counter: u64,
    /// Current incoming (decrypt) nonce counter value.
    pub decrypt_nonce_counter: u64,
    /// Number of key rotations performed.
    pub rotation_count: u32,
    /// Whether the session was active at save time.
    pub was_active: bool,
}

/// A queued outbound message that has not yet been delivered.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OutboundLaneEntry {
    /// The delivery ID assigned to this outbound message.
    pub delivery_id: String,
    /// Serialized message payload.
    pub payload: Vec<u8>,
    /// Monotonic sequence number within the outbound lane.
    pub sequence: u64,
}

/// The complete persistent state of a PCA transport instance.
///
/// Serialize this with [`save_state`] and restore it with [`load_state`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PersistentState {
    /// Persisted session material for each active session.
    pub sessions: Vec<SessionPersistence>,
    /// Set of delivery IDs that have already been processed.
    pub dedup_markers: Vec<String>,
    /// Turns that were accepted but not yet completed.
    pub pending_turns: Vec<PendingTurn>,
    /// Outbound messages that have not yet been delivered.
    pub outbound_lanes: Vec<OutboundLaneEntry>,
    /// Per-device channel subscriptions.
    pub device_channels: DeviceChannelSnapshot,
    /// Schema version for forward compatibility.
    pub version: u32,
}

/// Current schema version.
const STATE_VERSION: u32 = 1;

impl PersistentState {
    /// Create a new empty persistent state.
    pub fn empty() -> Self {
        Self {
            sessions: Vec::new(),
            dedup_markers: Vec::new(),
            pending_turns: Vec::new(),
            outbound_lanes: Vec::new(),
            device_channels: DeviceChannelSnapshot::default(),
            version: STATE_VERSION,
        }
    }

    /// Return whether this state contains any data worth persisting.
    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
            && self.dedup_markers.is_empty()
            && self.pending_turns.is_empty()
            && self.outbound_lanes.is_empty()
            && self.device_channels.subscriptions.is_empty()
    }

    /// Return the number of sessions stored.
    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }

    /// Return the number of pending turns stored.
    pub fn pending_turn_count(&self) -> usize {
        self.pending_turns.len()
    }

    /// Return the number of outbound lane entries stored.
    pub fn outbound_count(&self) -> usize {
        self.outbound_lanes.len()
    }
}

impl Default for PersistentState {
    fn default() -> Self {
        Self::empty()
    }
}

/// Serialize a [`PersistentState`] to a JSON byte vector.
pub fn save_state(state: &PersistentState) -> Result<Vec<u8>, crate::error::PcaError> {
    serde_json::to_vec_pretty(state).map_err(|e| crate::error::PcaError::EnvelopeError {
        reason: format!("failed to serialize persistent state: {e}"),
    })
}

/// Deserialize a [`PersistentState`] from a JSON byte slice.
pub fn load_state(data: &[u8]) -> Result<PersistentState, crate::error::PcaError> {
    serde_json::from_slice(data).map_err(|e| crate::error::PcaError::EnvelopeError {
        reason: format!("failed to deserialize persistent state: {e}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn sample_session() -> SessionPersistence {
        SessionPersistence {
            session_id: "sess-1".into(),
            local_address: "5Alice...".into(),
            peer_address: "5Bob...".into(),
            shared_secret: vec![42; 32],
            encrypt_nonce_counter: 10,
            decrypt_nonce_counter: 5,
            rotation_count: 1,
            was_active: true,
        }
    }

    fn sample_state() -> PersistentState {
        PersistentState {
            sessions: vec![sample_session()],
            dedup_markers: vec!["d-1".into(), "d-2".into()],
            pending_turns: vec![PendingTurn {
                delivery_id: "d-3".into(),
                payload: b"turn-data".to_vec(),
            }],
            outbound_lanes: vec![OutboundLaneEntry {
                delivery_id: "out-1".into(),
                payload: b"outbound-msg".to_vec(),
                sequence: 0,
            }],
            device_channels: DeviceChannelSnapshot {
                subscriptions: HashMap::from([(
                    "dev-1".into(),
                    vec!["ch-a".into(), "ch-b".into()],
                )]),
            },
            version: STATE_VERSION,
        }
    }

    #[test]
    fn empty_state_is_empty() {
        let state = PersistentState::empty();
        assert!(state.is_empty());
        assert_eq!(state.session_count(), 0);
        assert_eq!(state.pending_turn_count(), 0);
        assert_eq!(state.outbound_count(), 0);
        assert_eq!(state.version, STATE_VERSION);
    }

    #[test]
    fn default_is_empty() {
        let state = PersistentState::default();
        assert!(state.is_empty());
    }

    #[test]
    fn populated_state_is_not_empty() {
        let state = sample_state();
        assert!(!state.is_empty());
        assert_eq!(state.session_count(), 1);
        assert_eq!(state.pending_turn_count(), 1);
        assert_eq!(state.outbound_count(), 1);
    }

    #[test]
    fn save_and_load_roundtrip() {
        let state = sample_state();
        let bytes = save_state(&state).expect("save");
        let loaded = load_state(&bytes).expect("load");
        assert_eq!(loaded, state);
    }

    #[test]
    fn save_produces_valid_json() {
        let state = sample_state();
        let bytes = save_state(&state).expect("save");
        let json_str = std::str::from_utf8(&bytes).expect("utf8");
        assert!(json_str.contains("sess-1"));
        assert!(json_str.contains("5Alice..."));
        assert!(json_str.contains("d-1"));
    }

    #[test]
    fn load_rejects_invalid_json() {
        let result = load_state(b"not json");
        assert!(result.is_err());
    }

    #[test]
    fn load_rejects_empty_input() {
        let result = load_state(b"");
        assert!(result.is_err());
    }

    #[test]
    fn session_persistence_fields() {
        let session = sample_session();
        assert_eq!(session.session_id, "sess-1");
        assert_eq!(session.local_address, "5Alice...");
        assert_eq!(session.peer_address, "5Bob...");
        assert_eq!(session.shared_secret.len(), 32);
        assert_eq!(session.encrypt_nonce_counter, 10);
        assert_eq!(session.decrypt_nonce_counter, 5);
        assert_eq!(session.rotation_count, 1);
        assert!(session.was_active);
    }

    #[test]
    fn outbound_lane_entry_serialization() {
        let entry = OutboundLaneEntry {
            delivery_id: "out-42".into(),
            payload: b"payload".to_vec(),
            sequence: 7,
        };
        let json = serde_json::to_string(&entry).expect("serialize");
        let back: OutboundLaneEntry = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, entry);
    }

    #[test]
    fn multiple_sessions_roundtrip() {
        let state = PersistentState {
            sessions: vec![
                SessionPersistence {
                    session_id: "s-1".into(),
                    local_address: "5A...".into(),
                    peer_address: "5B...".into(),
                    shared_secret: vec![1; 32],
                    encrypt_nonce_counter: 0,
                    decrypt_nonce_counter: 0,
                    rotation_count: 0,
                    was_active: true,
                },
                SessionPersistence {
                    session_id: "s-2".into(),
                    local_address: "5A...".into(),
                    peer_address: "5C...".into(),
                    shared_secret: vec![2; 32],
                    encrypt_nonce_counter: 100,
                    decrypt_nonce_counter: 50,
                    rotation_count: 3,
                    was_active: false,
                },
            ],
            ..PersistentState::empty()
        };

        let bytes = save_state(&state).expect("save");
        let loaded = load_state(&bytes).expect("load");
        assert_eq!(loaded.sessions.len(), 2);
        assert_eq!(loaded.sessions[0].session_id, "s-1");
        assert_eq!(loaded.sessions[1].session_id, "s-2");
        assert_eq!(loaded.sessions[1].rotation_count, 3);
        assert!(!loaded.sessions[1].was_active);
    }

    #[test]
    fn empty_state_roundtrips() {
        let state = PersistentState::empty();
        let bytes = save_state(&state).expect("save");
        let loaded = load_state(&bytes).expect("load");
        assert!(loaded.is_empty());
        assert_eq!(loaded.version, STATE_VERSION);
    }

    #[test]
    fn dedup_markers_preserved() {
        let state = PersistentState {
            dedup_markers: (0..100).map(|i| format!("d-{i}")).collect(),
            ..PersistentState::empty()
        };
        let bytes = save_state(&state).expect("save");
        let loaded = load_state(&bytes).expect("load");
        assert_eq!(loaded.dedup_markers.len(), 100);
        assert!(loaded.dedup_markers.contains(&"d-0".to_string()));
        assert!(loaded.dedup_markers.contains(&"d-99".to_string()));
    }

    #[test]
    fn pending_turns_preserved() {
        let state = PersistentState {
            pending_turns: vec![
                PendingTurn {
                    delivery_id: "d-a".into(),
                    payload: b"p1".to_vec(),
                },
                PendingTurn {
                    delivery_id: "d-b".into(),
                    payload: b"p2".to_vec(),
                },
            ],
            ..PersistentState::empty()
        };
        let bytes = save_state(&state).expect("save");
        let loaded = load_state(&bytes).expect("load");
        assert_eq!(loaded.pending_turns.len(), 2);
        assert_eq!(loaded.pending_turns[0].delivery_id, "d-a");
        assert_eq!(loaded.pending_turns[1].payload, b"p2");
    }

    #[test]
    fn device_channels_preserved() {
        let state = PersistentState {
            device_channels: DeviceChannelSnapshot {
                subscriptions: HashMap::from([
                    ("dev-1".into(), vec!["ch-a".into(), "ch-b".into()]),
                    ("dev-2".into(), vec!["ch-c".into()]),
                ]),
            },
            ..PersistentState::empty()
        };
        let bytes = save_state(&state).expect("save");
        let loaded = load_state(&bytes).expect("load");
        assert_eq!(loaded.device_channels.subscriptions.len(), 2);
        assert_eq!(
            loaded.device_channels.subscriptions["dev-1"],
            vec!["ch-a", "ch-b"]
        );
    }

    #[test]
    fn state_is_not_empty_with_only_dedup() {
        let state = PersistentState {
            dedup_markers: vec!["d-1".into()],
            ..PersistentState::empty()
        };
        assert!(!state.is_empty());
    }

    #[test]
    fn state_is_not_empty_with_only_outbound() {
        let state = PersistentState {
            outbound_lanes: vec![OutboundLaneEntry {
                delivery_id: "o-1".into(),
                payload: vec![],
                sequence: 0,
            }],
            ..PersistentState::empty()
        };
        assert!(!state.is_empty());
    }

    #[test]
    fn version_is_preserved() {
        let mut state = PersistentState::empty();
        state.version = 42;
        let bytes = save_state(&state).expect("save");
        let loaded = load_state(&bytes).expect("load");
        assert_eq!(loaded.version, 42);
    }
}
