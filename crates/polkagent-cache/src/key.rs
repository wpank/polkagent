use std::fmt;

use serde::{Deserialize, Serialize};

/// The byte-length threshold above which a raw key is hashed with BLAKE3
/// to keep internal storage compact.
const HASH_THRESHOLD: usize = 128;

/// A cache key with namespace support.
///
/// Keys are composed of a `namespace` and a `name`, which together form a
/// unique identifier.  When the combined string representation exceeds
/// `HASH_THRESHOLD` bytes the key is transparently hashed with BLAKE3 so
/// that internal data-structure comparisons stay cheap.
#[derive(Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct CacheKey {
    namespace: String,
    name: String,
    /// Pre-computed canonical string (possibly BLAKE3-hashed).
    canonical: String,
}

impl CacheKey {
    /// Create a new `CacheKey` in the given `namespace`.
    ///
    /// ```
    /// use polkagent_cache::CacheKey;
    /// let key = CacheKey::new("metadata", "polkadot");
    /// assert_eq!(key.namespace(), "metadata");
    /// assert_eq!(key.name(), "polkadot");
    /// ```
    pub fn new(namespace: impl Into<String>, name: impl Into<String>) -> Self {
        let namespace = namespace.into();
        let name = name.into();
        let raw = format!("{namespace}:{name}");
        let canonical = if raw.len() > HASH_THRESHOLD {
            let hash = blake3::hash(raw.as_bytes());
            format!("{namespace}:{hash}")
        } else {
            raw
        };
        Self {
            namespace,
            name,
            canonical,
        }
    }

    /// Returns the namespace portion of the key.
    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    /// Returns the name portion of the key.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the canonical string representation used for storage lookups.
    ///
    /// For short keys this is `"namespace:name"`.  For long keys this is
    /// `"namespace:<blake3-hash>"`.
    pub fn canonical(&self) -> &str {
        &self.canonical
    }

    /// Returns `true` if the raw key was hashed with BLAKE3.
    pub fn is_hashed(&self) -> bool {
        let raw = format!("{}:{}", self.namespace, self.name);
        raw.len() > HASH_THRESHOLD
    }
}

impl fmt::Debug for CacheKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CacheKey({}:{})", self.namespace, self.name)
    }
}

impl fmt::Display for CacheKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.canonical)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_key_is_not_hashed() {
        let key = CacheKey::new("ns", "short");
        assert!(!key.is_hashed());
        assert_eq!(key.canonical(), "ns:short");
    }

    #[test]
    fn long_key_is_hashed_with_blake3() {
        let long_name = "a".repeat(200);
        let key = CacheKey::new("ns", &long_name);
        assert!(key.is_hashed());
        // canonical should start with namespace
        assert!(key.canonical().starts_with("ns:"));
        // and should NOT contain the raw long name
        assert!(!key.canonical().contains(&long_name));
    }

    #[test]
    fn same_inputs_produce_equal_keys() {
        let a = CacheKey::new("metadata", "polkadot");
        let b = CacheKey::new("metadata", "polkadot");
        assert_eq!(a, b);
    }

    #[test]
    fn different_namespaces_differ() {
        let a = CacheKey::new("metadata", "polkadot");
        let b = CacheKey::new("runtime", "polkadot");
        assert_ne!(a, b);
    }

    #[test]
    fn display_shows_canonical() {
        let key = CacheKey::new("metadata", "polkadot");
        assert_eq!(format!("{key}"), "metadata:polkadot");
    }

    #[test]
    fn blake3_hash_is_deterministic() {
        let long_name = "b".repeat(200);
        let k1 = CacheKey::new("ns", &long_name);
        let k2 = CacheKey::new("ns", &long_name);
        assert_eq!(k1.canonical(), k2.canonical());
    }
}
