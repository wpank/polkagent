//! Per-device channel subscription tracking.
//!
//! [`DeviceChannelSet`] maintains a mapping of device identifiers to their
//! active channel subscriptions. On restart, the set can be restored from
//! persistent state so that channel routing resumes without requiring
//! re-subscription from every device.

use std::collections::{HashMap, HashSet};

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

/// A unique device identifier (e.g. a hardware fingerprint or session token).
pub type DeviceId = String;

/// A channel identifier that a device can subscribe to.
pub type ChannelId = String;

/// Serializable snapshot of device-to-channel subscriptions.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeviceChannelSnapshot {
    /// Map from device ID to its set of subscribed channel IDs.
    pub subscriptions: HashMap<DeviceId, Vec<ChannelId>>,
}

/// Thread-safe set that tracks which channels each device is subscribed to.
///
/// Designed to survive restarts: call [`snapshot`] to persist the current
/// state and [`restore`] to reload it.
pub struct DeviceChannelSet {
    inner: RwLock<HashMap<DeviceId, HashSet<ChannelId>>>,
}

impl DeviceChannelSet {
    /// Create an empty device channel set.
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(HashMap::new()),
        }
    }

    /// Subscribe a device to a channel.
    ///
    /// Returns `true` if the subscription was new.
    pub fn subscribe(&self, device_id: &str, channel_id: &str) -> bool {
        let mut map = self.inner.write();
        map.entry(device_id.to_owned())
            .or_default()
            .insert(channel_id.to_owned())
    }

    /// Unsubscribe a device from a channel.
    ///
    /// Returns `true` if the subscription existed and was removed.
    pub fn unsubscribe(&self, device_id: &str, channel_id: &str) -> bool {
        let mut map = self.inner.write();
        if let Some(channels) = map.get_mut(device_id) {
            let removed = channels.remove(channel_id);
            if channels.is_empty() {
                map.remove(device_id);
            }
            removed
        } else {
            false
        }
    }

    /// Remove all subscriptions for a device.
    ///
    /// Returns the number of channels the device was subscribed to.
    pub fn remove_device(&self, device_id: &str) -> usize {
        let mut map = self.inner.write();
        map.remove(device_id).map_or(0, |channels| channels.len())
    }

    /// Check whether a device is subscribed to a specific channel.
    pub fn is_subscribed(&self, device_id: &str, channel_id: &str) -> bool {
        let map = self.inner.read();
        map.get(device_id)
            .map_or(false, |channels| channels.contains(channel_id))
    }

    /// Return all channel IDs a device is subscribed to.
    pub fn channels_for_device(&self, device_id: &str) -> Vec<ChannelId> {
        let map = self.inner.read();
        map.get(device_id)
            .map(|channels| {
                let mut v: Vec<_> = channels.iter().cloned().collect();
                v.sort();
                v
            })
            .unwrap_or_default()
    }

    /// Return all device IDs subscribed to a specific channel.
    pub fn devices_for_channel(&self, channel_id: &str) -> Vec<DeviceId> {
        let map = self.inner.read();
        let mut result: Vec<_> = map
            .iter()
            .filter(|(_, channels)| channels.contains(channel_id))
            .map(|(device_id, _)| device_id.clone())
            .collect();
        result.sort();
        result
    }

    /// Return the total number of tracked devices.
    pub fn device_count(&self) -> usize {
        self.inner.read().len()
    }

    /// Return the total number of subscriptions across all devices.
    pub fn subscription_count(&self) -> usize {
        self.inner.read().values().map(HashSet::len).sum()
    }

    /// Take a serializable snapshot of the current subscriptions.
    pub fn snapshot(&self) -> DeviceChannelSnapshot {
        let map = self.inner.read();
        let subscriptions = map
            .iter()
            .map(|(device_id, channels)| {
                let mut ch: Vec<_> = channels.iter().cloned().collect();
                ch.sort();
                (device_id.clone(), ch)
            })
            .collect();
        DeviceChannelSnapshot { subscriptions }
    }

    /// Restore subscriptions from a snapshot (additive — does not clear
    /// existing entries).
    pub fn restore(&self, snapshot: &DeviceChannelSnapshot) {
        let mut map = self.inner.write();
        for (device_id, channels) in &snapshot.subscriptions {
            let entry = map.entry(device_id.clone()).or_default();
            for ch in channels {
                entry.insert(ch.clone());
            }
        }
    }

    /// Clear all subscriptions.
    pub fn clear(&self) {
        self.inner.write().clear();
    }
}

impl Default for DeviceChannelSet {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subscribe_and_check() {
        let set = DeviceChannelSet::new();
        assert!(set.subscribe("dev-1", "ch-a"));
        assert!(set.is_subscribed("dev-1", "ch-a"));
        assert!(!set.is_subscribed("dev-1", "ch-b"));
    }

    #[test]
    fn duplicate_subscribe_returns_false() {
        let set = DeviceChannelSet::new();
        assert!(set.subscribe("dev-1", "ch-a"));
        assert!(!set.subscribe("dev-1", "ch-a"));
    }

    #[test]
    fn unsubscribe_removes_channel() {
        let set = DeviceChannelSet::new();
        set.subscribe("dev-1", "ch-a");
        set.subscribe("dev-1", "ch-b");

        assert!(set.unsubscribe("dev-1", "ch-a"));
        assert!(!set.is_subscribed("dev-1", "ch-a"));
        assert!(set.is_subscribed("dev-1", "ch-b"));
    }

    #[test]
    fn unsubscribe_nonexistent_returns_false() {
        let set = DeviceChannelSet::new();
        assert!(!set.unsubscribe("dev-1", "ch-a"));
    }

    #[test]
    fn unsubscribe_last_channel_removes_device() {
        let set = DeviceChannelSet::new();
        set.subscribe("dev-1", "ch-a");
        set.unsubscribe("dev-1", "ch-a");
        assert_eq!(set.device_count(), 0);
    }

    #[test]
    fn remove_device_clears_all() {
        let set = DeviceChannelSet::new();
        set.subscribe("dev-1", "ch-a");
        set.subscribe("dev-1", "ch-b");
        set.subscribe("dev-1", "ch-c");

        let count = set.remove_device("dev-1");
        assert_eq!(count, 3);
        assert_eq!(set.device_count(), 0);
    }

    #[test]
    fn remove_unknown_device_returns_zero() {
        let set = DeviceChannelSet::new();
        assert_eq!(set.remove_device("unknown"), 0);
    }

    #[test]
    fn channels_for_device() {
        let set = DeviceChannelSet::new();
        set.subscribe("dev-1", "ch-b");
        set.subscribe("dev-1", "ch-a");
        set.subscribe("dev-1", "ch-c");

        let channels = set.channels_for_device("dev-1");
        assert_eq!(channels, vec!["ch-a", "ch-b", "ch-c"]);
    }

    #[test]
    fn channels_for_unknown_device_is_empty() {
        let set = DeviceChannelSet::new();
        assert!(set.channels_for_device("unknown").is_empty());
    }

    #[test]
    fn devices_for_channel() {
        let set = DeviceChannelSet::new();
        set.subscribe("dev-b", "ch-1");
        set.subscribe("dev-a", "ch-1");
        set.subscribe("dev-c", "ch-2");

        let devices = set.devices_for_channel("ch-1");
        assert_eq!(devices, vec!["dev-a", "dev-b"]);
    }

    #[test]
    fn device_and_subscription_counts() {
        let set = DeviceChannelSet::new();
        assert_eq!(set.device_count(), 0);
        assert_eq!(set.subscription_count(), 0);

        set.subscribe("dev-1", "ch-a");
        set.subscribe("dev-1", "ch-b");
        set.subscribe("dev-2", "ch-a");

        assert_eq!(set.device_count(), 2);
        assert_eq!(set.subscription_count(), 3);
    }

    #[test]
    fn snapshot_and_restore() {
        let set = DeviceChannelSet::new();
        set.subscribe("dev-1", "ch-a");
        set.subscribe("dev-1", "ch-b");
        set.subscribe("dev-2", "ch-c");

        let snap = set.snapshot();

        let restored = DeviceChannelSet::new();
        restored.restore(&snap);

        assert!(restored.is_subscribed("dev-1", "ch-a"));
        assert!(restored.is_subscribed("dev-1", "ch-b"));
        assert!(restored.is_subscribed("dev-2", "ch-c"));
        assert_eq!(restored.device_count(), 2);
        assert_eq!(restored.subscription_count(), 3);
    }

    #[test]
    fn restore_is_additive() {
        let set = DeviceChannelSet::new();
        set.subscribe("dev-1", "ch-existing");

        let snap = DeviceChannelSnapshot {
            subscriptions: HashMap::from([
                ("dev-1".into(), vec!["ch-new".into()]),
                ("dev-2".into(), vec!["ch-x".into()]),
            ]),
        };
        set.restore(&snap);

        assert!(set.is_subscribed("dev-1", "ch-existing"));
        assert!(set.is_subscribed("dev-1", "ch-new"));
        assert!(set.is_subscribed("dev-2", "ch-x"));
    }

    #[test]
    fn snapshot_serializes_to_json() {
        let set = DeviceChannelSet::new();
        set.subscribe("dev-1", "ch-a");

        let snap = set.snapshot();
        let json = serde_json::to_string(&snap).expect("serialize");
        let back: DeviceChannelSnapshot = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, snap);
    }

    #[test]
    fn clear_removes_all() {
        let set = DeviceChannelSet::new();
        set.subscribe("dev-1", "ch-a");
        set.subscribe("dev-2", "ch-b");
        set.clear();
        assert_eq!(set.device_count(), 0);
        assert_eq!(set.subscription_count(), 0);
    }

    #[test]
    fn default_creates_empty() {
        let set = DeviceChannelSet::default();
        assert_eq!(set.device_count(), 0);
    }

    #[test]
    fn concurrent_access() {
        use std::sync::Arc;
        use std::thread;

        let set = Arc::new(DeviceChannelSet::new());
        let mut handles = Vec::new();

        for i in 0..10 {
            let s = Arc::clone(&set);
            handles.push(thread::spawn(move || {
                let dev = format!("dev-{i}");
                for j in 0..5 {
                    let ch = format!("ch-{j}");
                    s.subscribe(&dev, &ch);
                }
            }));
        }

        for h in handles {
            h.join().expect("join");
        }

        assert_eq!(set.device_count(), 10);
        assert_eq!(set.subscription_count(), 50);
    }
}
