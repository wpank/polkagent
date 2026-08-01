use std::sync::atomic::{AtomicU64, Ordering};

/// Thread-safe cache statistics backed by atomic counters.
///
/// All mutating methods use `Relaxed` ordering because precise cross-thread
/// consistency is not critical for metrics -- we only need eventual
/// convergence and no torn reads/writes (guaranteed by aligned atomics).
#[derive(Debug, Default)]
pub struct CacheStats {
    hits: AtomicU64,
    misses: AtomicU64,
    evictions: AtomicU64,
    insertions: AtomicU64,
    removals: AtomicU64,
}

impl CacheStats {
    /// Create a fresh zeroed stats instance.
    pub fn new() -> Self {
        Self::default()
    }

    // -- Increment helpers ---------------------------------------------------

    /// Record a cache hit.
    pub fn record_hit(&self) {
        self.hits.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a cache miss.
    pub fn record_miss(&self) {
        self.misses.fetch_add(1, Ordering::Relaxed);
    }

    /// Record an eviction.
    pub fn record_eviction(&self) {
        self.evictions.fetch_add(1, Ordering::Relaxed);
    }

    /// Record an insertion.
    pub fn record_insertion(&self) {
        self.insertions.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a removal.
    pub fn record_removal(&self) {
        self.removals.fetch_add(1, Ordering::Relaxed);
    }

    // -- Read helpers --------------------------------------------------------

    /// Total cache hits.
    pub fn hits(&self) -> u64 {
        self.hits.load(Ordering::Relaxed)
    }

    /// Total cache misses.
    pub fn misses(&self) -> u64 {
        self.misses.load(Ordering::Relaxed)
    }

    /// Total evictions.
    pub fn evictions(&self) -> u64 {
        self.evictions.load(Ordering::Relaxed)
    }

    /// Total insertions.
    pub fn insertions(&self) -> u64 {
        self.insertions.load(Ordering::Relaxed)
    }

    /// Total removals.
    pub fn removals(&self) -> u64 {
        self.removals.load(Ordering::Relaxed)
    }

    /// Compute the hit rate as a value between 0.0 and 1.0.
    ///
    /// Returns 0.0 when no lookups have been performed.
    pub fn hit_rate(&self) -> f64 {
        let h = self.hits() as f64;
        let m = self.misses() as f64;
        let total = h + m;
        if total == 0.0 {
            0.0
        } else {
            h / total
        }
    }

    /// Reset all counters to zero.
    pub fn reset(&self) {
        self.hits.store(0, Ordering::Relaxed);
        self.misses.store(0, Ordering::Relaxed);
        self.evictions.store(0, Ordering::Relaxed);
        self.insertions.store(0, Ordering::Relaxed);
        self.removals.store(0, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_stats_are_zero() {
        let stats = CacheStats::new();
        assert_eq!(stats.hits(), 0);
        assert_eq!(stats.misses(), 0);
        assert_eq!(stats.evictions(), 0);
        assert_eq!(stats.insertions(), 0);
        assert_eq!(stats.removals(), 0);
        assert!((stats.hit_rate() - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn hit_rate_calculation() {
        let stats = CacheStats::new();
        stats.record_hit();
        stats.record_hit();
        stats.record_hit();
        stats.record_miss();
        // 3 hits / 4 total = 0.75
        assert!((stats.hit_rate() - 0.75).abs() < f64::EPSILON);
    }

    #[test]
    fn reset_clears_counters() {
        let stats = CacheStats::new();
        stats.record_hit();
        stats.record_miss();
        stats.record_eviction();
        stats.reset();
        assert_eq!(stats.hits(), 0);
        assert_eq!(stats.misses(), 0);
        assert_eq!(stats.evictions(), 0);
    }

    #[test]
    fn counters_increment_independently() {
        let stats = CacheStats::new();
        stats.record_insertion();
        stats.record_insertion();
        stats.record_removal();
        assert_eq!(stats.insertions(), 2);
        assert_eq!(stats.removals(), 1);
        assert_eq!(stats.hits(), 0);
    }
}
