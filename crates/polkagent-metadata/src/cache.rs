//! In-memory LRU metadata cache.
//!
//! [`MetadataCache`] stores [`MetadataSnapshot`]s keyed by `(ChainId, MetadataHash)`,
//! with a configurable maximum number of entries. When the cache is full the
//! least-recently-used entry is evicted to make room.
//!
//! The cache is thread-safe and can be shared across async tasks.

use std::collections::VecDeque;
use std::sync::{Arc, RwLock};

use crate::types::{ChainId, MetadataHash, MetadataSnapshot};

/// Default maximum number of entries in the cache.
const DEFAULT_MAX_ENTRIES: usize = 16;

/// Thread-safe, LRU metadata cache.
///
/// Stores metadata snapshots keyed by `(ChainId, MetadataHash)`.
/// When the number of entries exceeds `max_entries`, the
/// least-recently-used entry is evicted.
#[derive(Debug, Clone)]
pub struct MetadataCache {
    inner: Arc<RwLock<CacheInner>>,
}

#[derive(Debug)]
struct CacheInner {
    /// Ordered list of entries; most-recently-used is at the back.
    entries: VecDeque<CacheEntry>,
    /// Maximum number of entries before eviction.
    max_entries: usize,
}

#[derive(Debug, Clone)]
struct CacheEntry {
    chain_id: ChainId,
    hash: MetadataHash,
    snapshot: MetadataSnapshot,
}

impl MetadataCache {
    /// Create a new cache with the default capacity (16 entries).
    #[must_use]
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_MAX_ENTRIES)
    }

    /// Create a new cache with a specific maximum number of entries.
    ///
    /// # Panics
    ///
    /// Panics if `max_entries` is 0.
    #[must_use]
    pub fn with_capacity(max_entries: usize) -> Self {
        assert!(max_entries > 0, "max_entries must be > 0");
        Self {
            inner: Arc::new(RwLock::new(CacheInner {
                entries: VecDeque::with_capacity(max_entries),
                max_entries,
            })),
        }
    }

    /// Insert a snapshot into the cache.
    ///
    /// If a snapshot with the same `(chain_id, hash)` already exists it is
    /// replaced and moved to the most-recently-used position. If the cache
    /// is full the least-recently-used entry is evicted first.
    pub fn insert(&self, snapshot: MetadataSnapshot) {
        let mut inner = self
            .inner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let chain_id = snapshot.chain_id.clone();
        let hash = snapshot.hash.clone();

        // Remove existing entry with the same key (if any).
        inner
            .entries
            .retain(|e| !(e.chain_id == chain_id && e.hash == hash));

        // Evict LRU if at capacity.
        while inner.entries.len() >= inner.max_entries {
            inner.entries.pop_front();
        }

        inner.entries.push_back(CacheEntry {
            chain_id,
            hash,
            snapshot,
        });
    }

    /// Look up a specific snapshot by chain ID and hash.
    ///
    /// Marks the entry as most-recently-used on hit.
    #[must_use]
    pub fn get(&self, chain_id: &ChainId, hash: &MetadataHash) -> Option<MetadataSnapshot> {
        let mut inner = self
            .inner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let pos = inner
            .entries
            .iter()
            .position(|e| &e.chain_id == chain_id && &e.hash == hash)?;

        // Move to back (MRU).
        let entry = inner.entries.remove(pos)?;
        let snapshot = entry.snapshot.clone();
        inner.entries.push_back(entry);

        Some(snapshot)
    }

    /// Get the most recently inserted/accessed snapshot for a chain.
    #[must_use]
    pub fn get_latest(&self, chain_id: &ChainId) -> Option<MetadataSnapshot> {
        let inner = self
            .inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inner
            .entries
            .iter()
            .rev()
            .find(|e| &e.chain_id == chain_id)
            .map(|e| e.snapshot.clone())
    }

    /// Remove a specific entry from the cache.
    ///
    /// Returns `true` if the entry was found and removed.
    pub fn evict(&self, chain_id: &ChainId, hash: &MetadataHash) -> bool {
        let mut inner = self
            .inner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let before = inner.entries.len();
        inner
            .entries
            .retain(|e| !(&e.chain_id == chain_id && &e.hash == hash));
        inner.entries.len() < before
    }

    /// Return the current number of entries in the cache.
    #[must_use]
    pub fn len(&self) -> usize {
        let inner = self
            .inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inner.entries.len()
    }

    /// Return `true` if the cache is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Return the maximum number of entries the cache can hold.
    #[must_use]
    pub fn capacity(&self) -> usize {
        let inner = self
            .inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inner.max_entries
    }
}

impl Default for MetadataCache {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::MetadataVersion;
    use polkagent_core::now;

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
    fn insert_and_get() {
        let cache = MetadataCache::new();
        let snap = make_snapshot("polkadot", b"metadata_v1");
        let hash = snap.hash.clone();
        let chain_id = snap.chain_id.clone();

        cache.insert(snap);
        let got = cache.get(&chain_id, &hash);
        assert!(got.is_some());
        assert_eq!(got.as_ref().map(|s| &s.hash), Some(&hash));
    }

    #[test]
    fn get_returns_none_for_missing() {
        let cache = MetadataCache::new();
        let result = cache.get(
            &ChainId::new("polkadot"),
            &MetadataHash::from_hex("nonexistent"),
        );
        assert!(result.is_none());
    }

    #[test]
    fn get_latest_returns_most_recent() {
        let cache = MetadataCache::new();
        let chain = ChainId::new("polkadot");

        let snap1 = make_snapshot("polkadot", b"old_metadata");
        let snap2 = make_snapshot("polkadot", b"new_metadata");
        let expected_hash = snap2.hash.clone();

        cache.insert(snap1);
        cache.insert(snap2);

        let latest = cache.get_latest(&chain);
        assert!(latest.is_some());
        assert_eq!(latest.as_ref().map(|s| &s.hash), Some(&expected_hash));
    }

    #[test]
    fn get_latest_returns_none_for_missing_chain() {
        let cache = MetadataCache::new();
        let result = cache.get_latest(&ChainId::new("nonexistent"));
        assert!(result.is_none());
    }

    #[test]
    fn evict_removes_entry() {
        let cache = MetadataCache::new();
        let snap = make_snapshot("polkadot", b"to_evict");
        let hash = snap.hash.clone();
        let chain_id = snap.chain_id.clone();

        cache.insert(snap);
        assert_eq!(cache.len(), 1);

        let removed = cache.evict(&chain_id, &hash);
        assert!(removed);
        assert_eq!(cache.len(), 0);
        assert!(cache.get(&chain_id, &hash).is_none());
    }

    #[test]
    fn evict_returns_false_for_missing() {
        let cache = MetadataCache::new();
        let removed = cache.evict(
            &ChainId::new("polkadot"),
            &MetadataHash::from_hex("nonexistent"),
        );
        assert!(!removed);
    }

    #[test]
    fn lru_eviction_when_full() {
        let cache = MetadataCache::with_capacity(3);

        let snap1 = make_snapshot("polkadot", b"snap1");
        let snap2 = make_snapshot("kusama", b"snap2");
        let snap3 = make_snapshot("westend", b"snap3");

        let hash1 = snap1.hash.clone();
        let chain1 = snap1.chain_id.clone();

        cache.insert(snap1);
        cache.insert(snap2);
        cache.insert(snap3);
        assert_eq!(cache.len(), 3);

        // Insert a 4th entry; snap1 (LRU) should be evicted.
        let snap4 = make_snapshot("rococo", b"snap4");
        cache.insert(snap4);
        assert_eq!(cache.len(), 3);

        // snap1 should be gone.
        assert!(cache.get(&chain1, &hash1).is_none());
    }

    #[test]
    fn lru_access_promotes_entry() {
        let cache = MetadataCache::with_capacity(3);

        let snap1 = make_snapshot("polkadot", b"snap1");
        let snap2 = make_snapshot("kusama", b"snap2");
        let snap3 = make_snapshot("westend", b"snap3");

        let hash1 = snap1.hash.clone();
        let chain1 = snap1.chain_id.clone();

        let hash2 = snap2.hash.clone();
        let chain2 = snap2.chain_id.clone();

        cache.insert(snap1);
        cache.insert(snap2);
        cache.insert(snap3);

        // Access snap1 to promote it to MRU.
        let _ = cache.get(&chain1, &hash1);

        // Insert snap4; snap2 should be evicted (it is now the LRU).
        let snap4 = make_snapshot("rococo", b"snap4");
        cache.insert(snap4);

        // snap1 should still exist (promoted by get).
        assert!(cache.get(&chain1, &hash1).is_some());
        // snap2 should be evicted.
        assert!(cache.get(&chain2, &hash2).is_none());
    }

    #[test]
    fn duplicate_insert_replaces() {
        let cache = MetadataCache::new();
        let snap = make_snapshot("polkadot", b"data");
        let hash = snap.hash.clone();
        let chain_id = snap.chain_id.clone();

        cache.insert(snap.clone());
        cache.insert(snap);
        assert_eq!(cache.len(), 1);
        assert!(cache.get(&chain_id, &hash).is_some());
    }

    #[test]
    fn default_capacity() {
        let cache = MetadataCache::new();
        assert_eq!(cache.capacity(), 16);
    }

    #[test]
    fn is_empty_and_len() {
        let cache = MetadataCache::new();
        assert!(cache.is_empty());
        assert_eq!(cache.len(), 0);

        cache.insert(make_snapshot("polkadot", b"x"));
        assert!(!cache.is_empty());
        assert_eq!(cache.len(), 1);
    }

    #[test]
    #[should_panic(expected = "max_entries must be > 0")]
    fn zero_capacity_panics() {
        let _ = MetadataCache::with_capacity(0);
    }

    #[test]
    fn thread_safety() {
        use std::sync::Arc;
        use std::thread;

        let cache = Arc::new(MetadataCache::with_capacity(100));
        let mut handles = vec![];

        for i in 0..10 {
            let cache = Arc::clone(&cache);
            handles.push(thread::spawn(move || {
                let data = format!("thread_{i}");
                let snap = make_snapshot("polkadot", data.as_bytes());
                cache.insert(snap);
            }));
        }

        for h in handles {
            h.join().expect("thread panicked");
        }

        assert_eq!(cache.len(), 10);
    }
}
