use std::collections::HashMap;
use std::fmt;
use std::hash::Hash;
use std::time::Duration;

use chrono::{DateTime, Utc};

/// A callback invoked when an entry is evicted from the cache.
pub type EvictionCallback<K, V> = Box<dyn Fn(&K, &V) + Send + Sync>;

/// An entry stored inside the LRU cache.
struct Entry<K, V> {
    key: K,
    value: V,
    inserted_at: DateTime<Utc>,
    expires_at: Option<DateTime<Utc>>,
    access_count: u64,
    /// Doubly-linked-list pointers (indexes into `entries`).
    prev: Option<usize>,
    next: Option<usize>,
}

/// A generic LRU cache with per-entry TTL and eviction callbacks.
///
/// This is a building-block used internally by [`crate::memory::InMemoryCache`]. It is **not**
/// thread-safe on its own -- the caller is expected to wrap it in a lock.
pub struct LruCache<K, V> {
    /// Maximum number of entries before eviction kicks in.
    max_entries: usize,
    /// Default TTL applied when no per-entry TTL is specified.
    default_ttl: Option<Duration>,
    /// Slab storage for entries.
    entries: Vec<Option<Entry<K, V>>>,
    /// Fast key -> slab-index lookup.
    index: HashMap<K, usize>,
    /// Free list of slab indexes.
    free: Vec<usize>,
    /// Head of the LRU list (most recently used).
    head: Option<usize>,
    /// Tail of the LRU list (least recently used).
    tail: Option<usize>,
    /// Optional callback fired on eviction.
    on_evict: Option<EvictionCallback<K, V>>,
}

impl<K, V> LruCache<K, V>
where
    K: Eq + Hash + Clone + fmt::Debug,
    V: Clone,
{
    /// Create a new LRU cache with the given maximum capacity.
    pub fn new(max_entries: usize) -> Self {
        Self {
            max_entries,
            default_ttl: None,
            entries: Vec::new(),
            index: HashMap::new(),
            free: Vec::new(),
            head: None,
            tail: None,
            on_evict: None,
        }
    }

    /// Set the default TTL for entries that don't specify one.
    #[must_use]
    pub fn with_default_ttl(mut self, ttl: Duration) -> Self {
        self.default_ttl = Some(ttl);
        self
    }

    /// Register an eviction callback.
    #[must_use]
    pub fn with_eviction_callback(mut self, cb: EvictionCallback<K, V>) -> Self {
        self.on_evict = Some(cb);
        self
    }

    /// Insert or update a value.  Returns the old value if the key was
    /// already present.
    pub fn insert(&mut self, key: K, value: V, ttl: Option<Duration>) -> Option<V> {
        let now = Utc::now();
        let effective_ttl = ttl.or(self.default_ttl);
        let expires_at = effective_ttl.map(|d| {
            let millis = i64::try_from(d.as_millis()).unwrap_or(i64::MAX);
            now + chrono::Duration::milliseconds(millis)
        });

        // If the key already exists, update in place and promote.
        if let Some(&idx) = self.index.get(&key) {
            let entry = self.entries[idx]
                .as_mut()
                .unwrap_or_else(|| unreachable!("indexed entry must exist"));
            let old = entry.value.clone();
            entry.value = value;
            entry.expires_at = expires_at;
            entry.inserted_at = now;
            entry.access_count = 0;
            self.promote(idx);
            return Some(old);
        }

        // Evict expired entries first, then LRU if still at capacity.
        self.evict_expired();
        while self.index.len() >= self.max_entries {
            self.evict_lru();
        }

        let entry = Entry {
            key: key.clone(),
            value,
            inserted_at: now,
            expires_at,
            access_count: 0,
            prev: None,
            next: None,
        };

        let idx = if let Some(free_idx) = self.free.pop() {
            self.entries[free_idx] = Some(entry);
            free_idx
        } else {
            self.entries.push(Some(entry));
            self.entries.len() - 1
        };

        self.index.insert(key, idx);
        self.push_front(idx);
        None
    }

    /// Look up a value by key, returning `None` if absent or expired.
    /// Promotes the entry to most-recently-used on hit.
    pub fn get(&mut self, key: &K) -> Option<&V> {
        let idx = *self.index.get(key)?;
        // Check expiry.
        let now = Utc::now();
        let expired = self.entries[idx]
            .as_ref()
            .and_then(|e| e.expires_at)
            .is_some_and(|exp| exp <= now);

        if expired {
            self.remove_entry(idx);
            return None;
        }

        self.promote(idx);
        let entry = self.entries[idx]
            .as_mut()
            .unwrap_or_else(|| unreachable!("entry must exist"));
        entry.access_count += 1;
        Some(&entry.value)
    }

    /// Remove a key from the cache.  Returns the value if it was present.
    pub fn remove(&mut self, key: &K) -> Option<V> {
        let idx = *self.index.get(key)?;
        let entry = self.remove_entry(idx)?;
        Some(entry.value)
    }

    /// Clear the entire cache.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.index.clear();
        self.free.clear();
        self.head = None;
        self.tail = None;
    }

    /// Returns the number of live (non-expired) entries.
    pub fn len(&self) -> usize {
        self.index.len()
    }

    /// Returns `true` if the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    /// Check whether a key exists (and is not expired).
    pub fn contains_key(&mut self, key: &K) -> bool {
        self.get(key).is_some()
    }

    // -- Internal helpers ----------------------------------------------------

    /// Remove expired entries (lazy sweep).
    fn evict_expired(&mut self) {
        let now = Utc::now();
        let expired_idxs: Vec<usize> = self
            .index
            .values()
            .copied()
            .filter(|&idx| {
                self.entries[idx]
                    .as_ref()
                    .and_then(|e| e.expires_at)
                    .is_some_and(|exp| exp <= now)
            })
            .collect();
        for idx in expired_idxs {
            self.remove_entry(idx);
        }
    }

    /// Evict the least-recently-used entry (the tail of the list).
    fn evict_lru(&mut self) {
        if let Some(tail_idx) = self.tail {
            if let Some(entry) = self.remove_entry(tail_idx) {
                if let Some(ref cb) = self.on_evict {
                    cb(&entry.key, &entry.value);
                }
            }
        }
    }

    /// Remove an entry by slab index and unlink it from the list.
    fn remove_entry(&mut self, idx: usize) -> Option<Entry<K, V>> {
        let entry = self.entries[idx].take()?;
        self.index.remove(&entry.key);
        self.unlink(idx);
        self.free.push(idx);
        Some(entry)
    }

    /// Move an entry to the head of the list (most recently used).
    fn promote(&mut self, idx: usize) {
        if self.head == Some(idx) {
            return; // already at front
        }
        self.unlink(idx);
        self.push_front(idx);
    }

    /// Insert an index at the head of the doubly-linked list.
    fn push_front(&mut self, idx: usize) {
        let entry = self.entries[idx]
            .as_mut()
            .unwrap_or_else(|| unreachable!("entry must exist"));
        entry.prev = None;
        entry.next = self.head;

        if let Some(old_head) = self.head {
            if let Some(e) = self.entries[old_head].as_mut() {
                e.prev = Some(idx);
            }
        }
        self.head = Some(idx);
        if self.tail.is_none() {
            self.tail = Some(idx);
        }
    }

    /// Remove an index from the doubly-linked list without freeing the slot.
    fn unlink(&mut self, idx: usize) {
        let (prev, next) = {
            let Some(entry) = self.entries[idx].as_ref() else {
                return;
            };
            (entry.prev, entry.next)
        };

        if let Some(p) = prev {
            if let Some(e) = self.entries[p].as_mut() {
                e.next = next;
            }
        } else {
            self.head = next;
        }

        if let Some(n) = next {
            if let Some(e) = self.entries[n].as_mut() {
                e.prev = prev;
            }
        } else {
            self.tail = prev;
        }

        if let Some(e) = self.entries[idx].as_mut() {
            e.prev = None;
            e.next = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn insert_and_get() {
        let mut cache = LruCache::new(10);
        cache.insert("a", 1, None);
        assert_eq!(cache.get(&"a"), Some(&1));
    }

    #[test]
    fn lru_eviction_removes_oldest() {
        let mut cache = LruCache::new(2);
        cache.insert("a", 1, None);
        cache.insert("b", 2, None);
        cache.insert("c", 3, None); // should evict "a"
        assert!(cache.get(&"a").is_none());
        assert_eq!(cache.get(&"b"), Some(&2));
        assert_eq!(cache.get(&"c"), Some(&3));
    }

    #[test]
    fn access_promotes_entry() {
        let mut cache = LruCache::new(2);
        cache.insert("a", 1, None);
        cache.insert("b", 2, None);
        // Access "a" so "b" becomes LRU.
        let _ = cache.get(&"a");
        cache.insert("c", 3, None); // should evict "b"
        assert_eq!(cache.get(&"a"), Some(&1));
        assert!(cache.get(&"b").is_none());
        assert_eq!(cache.get(&"c"), Some(&3));
    }

    #[test]
    fn ttl_expiry() {
        let mut cache = LruCache::new(10);
        // Insert with a zero-duration TTL (expires immediately).
        cache.insert("x", 42, Some(Duration::from_millis(0)));
        // A tiny sleep isn't needed because chrono resolution is sub-ms.
        std::thread::sleep(Duration::from_millis(5));
        assert!(cache.get(&"x").is_none());
    }

    #[test]
    fn remove_returns_value() {
        let mut cache = LruCache::new(10);
        cache.insert("a", 1, None);
        assert_eq!(cache.remove(&"a"), Some(1));
        assert!(cache.get(&"a").is_none());
    }

    #[test]
    fn clear_empties_cache() {
        let mut cache = LruCache::new(10);
        cache.insert("a", 1, None);
        cache.insert("b", 2, None);
        cache.clear();
        assert!(cache.is_empty());
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn eviction_callback_fires() {
        let evicted = Arc::new(Mutex::new(Vec::new()));
        let evicted_clone = Arc::clone(&evicted);
        let mut cache =
            LruCache::new(2).with_eviction_callback(Box::new(move |k: &&str, v: &i32| {
                evicted_clone
                    .lock()
                    .expect("lock poisoned")
                    .push(((*k).to_string(), *v));
            }));
        cache.insert("a", 1, None);
        cache.insert("b", 2, None);
        cache.insert("c", 3, None); // evicts "a"
        let evicted = evicted.lock().expect("lock poisoned");
        assert_eq!(evicted.len(), 1);
        assert_eq!(evicted[0], ("a".to_string(), 1));
    }

    #[test]
    fn update_existing_key() {
        let mut cache = LruCache::new(10);
        cache.insert("a", 1, None);
        let old = cache.insert("a", 2, None);
        assert_eq!(old, Some(1));
        assert_eq!(cache.get(&"a"), Some(&2));
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn default_ttl_applies_when_none_specified() {
        let mut cache = LruCache::new(10).with_default_ttl(Duration::from_millis(0));
        cache.insert("a", 1, None);
        std::thread::sleep(Duration::from_millis(5));
        assert!(cache.get(&"a").is_none());
    }
}
