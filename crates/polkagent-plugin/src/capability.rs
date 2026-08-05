//! Plugin capabilities and capability sets.
//!
//! Each plugin declares the capabilities it requires (and optionally,
//! capabilities it can use if available). The runtime grants a
//! [`CapabilitySet`] to each plugin, and the sandbox checks operations
//! against this set before dispatching.
//!
//! # Capability model
//!
//! Capabilities follow a dot-separated naming convention that maps to the
//! [`PluginCapability`] enum:
//!
//! | String | Variant |
//! |--------|---------|
//! | `fs.read` | `ReadFileSystem` |
//! | `fs.write` | `WriteFileSystem` |
//! | `network.http` | `NetworkAccess` |
//! | `chain.query` | `ChainQuery` |
//! | `chain.submit` | `ChainSubmit` |
//! | `tool.execute` | `ToolExecution` |
//! | `memory.read` | `MemoryAccess` |

use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::error::PluginError;

// ---------------------------------------------------------------------------
// PluginCapability enum
// ---------------------------------------------------------------------------

/// A single capability that can be granted to a plugin.
///
/// Capabilities gate what operations a plugin is allowed to perform.
/// The sandbox checks each operation against the plugin's granted set
/// before dispatching.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginCapability {
    /// Read files from the filesystem.
    ReadFileSystem,
    /// Write files to the filesystem.
    WriteFileSystem,
    /// Make outbound HTTP/network requests.
    NetworkAccess,
    /// Query on-chain state (read-only).
    ChainQuery,
    /// Submit extrinsics / transactions to a chain.
    ChainSubmit,
    /// Invoke registered tools.
    ToolExecution,
    /// Read from the agent memory store.
    MemoryAccess,
}

impl PluginCapability {
    /// All known capabilities.
    pub const ALL: &'static [PluginCapability] = &[
        Self::ReadFileSystem,
        Self::WriteFileSystem,
        Self::NetworkAccess,
        Self::ChainQuery,
        Self::ChainSubmit,
        Self::ToolExecution,
        Self::MemoryAccess,
    ];

    /// Return the canonical dot-separated string representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ReadFileSystem => "fs.read",
            Self::WriteFileSystem => "fs.write",
            Self::NetworkAccess => "network.http",
            Self::ChainQuery => "chain.query",
            Self::ChainSubmit => "chain.submit",
            Self::ToolExecution => "tool.execute",
            Self::MemoryAccess => "memory.read",
        }
    }
}

impl fmt::Display for PluginCapability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for PluginCapability {
    type Err = PluginError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "fs.read" => Ok(Self::ReadFileSystem),
            "fs.write" => Ok(Self::WriteFileSystem),
            "network.http" => Ok(Self::NetworkAccess),
            "chain.query" => Ok(Self::ChainQuery),
            "chain.submit" => Ok(Self::ChainSubmit),
            "tool.execute" => Ok(Self::ToolExecution),
            "memory.read" => Ok(Self::MemoryAccess),
            other => Err(PluginError::ManifestInvalid {
                plugin_name: None,
                reason: format!("unknown capability: '{other}'"),
            }),
        }
    }
}

// ---------------------------------------------------------------------------
// CapabilitySet
// ---------------------------------------------------------------------------

/// An ordered set of capabilities, supporting intersection and union
/// operations.
///
/// Internally backed by a [`BTreeSet`] for deterministic iteration order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilitySet {
    inner: BTreeSet<PluginCapability>,
}

impl CapabilitySet {
    /// Create an empty capability set.
    pub fn empty() -> Self {
        Self {
            inner: BTreeSet::new(),
        }
    }

    /// Create a capability set containing all known capabilities.
    pub fn all() -> Self {
        Self {
            inner: PluginCapability::ALL.iter().copied().collect(),
        }
    }

    /// Parse a set of capability strings (e.g. from a TOML manifest).
    ///
    /// # Errors
    ///
    /// Returns [`PluginError::ManifestInvalid`] if any string does not map
    /// to a known capability.
    pub fn parse_strings(strings: &[String]) -> Result<Self, PluginError> {
        let mut set = BTreeSet::new();
        for s in strings {
            let cap = s.parse::<PluginCapability>()?;
            set.insert(cap);
        }
        Ok(Self { inner: set })
    }

    /// Insert a capability into the set.
    pub fn insert(&mut self, cap: PluginCapability) {
        self.inner.insert(cap);
    }

    /// Check whether the set contains a specific capability.
    pub fn contains(&self, cap: PluginCapability) -> bool {
        self.inner.contains(&cap)
    }

    /// Return the number of capabilities in the set.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Check whether the set is empty.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Return the intersection of two capability sets.
    ///
    /// The result contains only capabilities present in both sets.
    #[must_use]
    pub fn intersection(&self, other: &Self) -> Self {
        Self {
            inner: self.inner.intersection(&other.inner).copied().collect(),
        }
    }

    /// Return the union of two capability sets.
    ///
    /// The result contains all capabilities present in either set.
    #[must_use]
    pub fn union(&self, other: &Self) -> Self {
        Self {
            inner: self.inner.union(&other.inner).copied().collect(),
        }
    }

    /// Check whether this set is a subset of another.
    pub fn is_subset_of(&self, other: &Self) -> bool {
        self.inner.is_subset(&other.inner)
    }

    /// Check whether this set is a superset of another.
    pub fn is_superset_of(&self, other: &Self) -> bool {
        self.inner.is_superset(&other.inner)
    }

    /// Return an iterator over the capabilities in the set.
    pub fn iter(&self) -> impl Iterator<Item = PluginCapability> + '_ {
        self.inner.iter().copied()
    }

    /// Return the capabilities as a sorted vector of strings.
    pub fn to_strings(&self) -> Vec<String> {
        self.inner.iter().map(ToString::to_string).collect()
    }

    /// Return capabilities in `self` that are missing from `other`.
    #[must_use]
    pub fn difference(&self, other: &Self) -> Self {
        Self {
            inner: self.inner.difference(&other.inner).copied().collect(),
        }
    }
}

impl FromIterator<PluginCapability> for CapabilitySet {
    fn from_iter<I: IntoIterator<Item = PluginCapability>>(iter: I) -> Self {
        Self {
            inner: iter.into_iter().collect(),
        }
    }
}

impl Default for CapabilitySet {
    fn default() -> Self {
        Self::empty()
    }
}

impl fmt::Display for CapabilitySet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let strings: Vec<&str> = self.inner.iter().map(PluginCapability::as_str).collect();
        write!(f, "[{}]", strings.join(", "))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "capability parser tests intentionally panic when static fixtures fail to parse"
)]
mod tests {
    use super::*;

    #[test]
    fn capability_roundtrip_str() {
        for cap in PluginCapability::ALL {
            let s = cap.as_str();
            let parsed: PluginCapability = s.parse().expect("should parse");
            assert_eq!(*cap, parsed);
        }
    }

    #[test]
    fn capability_parse_unknown() {
        let result = "unknown.cap".parse::<PluginCapability>();
        assert!(result.is_err());
    }

    #[test]
    fn capability_display() {
        assert_eq!(PluginCapability::ChainQuery.to_string(), "chain.query");
        assert_eq!(PluginCapability::ReadFileSystem.to_string(), "fs.read");
    }

    #[test]
    fn capability_set_empty() {
        let set = CapabilitySet::empty();
        assert!(set.is_empty());
        assert_eq!(set.len(), 0);
        assert!(!set.contains(PluginCapability::ChainQuery));
    }

    #[test]
    fn capability_set_all() {
        let set = CapabilitySet::all();
        assert_eq!(set.len(), PluginCapability::ALL.len());
        for cap in PluginCapability::ALL {
            assert!(set.contains(*cap));
        }
    }

    #[test]
    fn capability_set_insert_and_contains() {
        let mut set = CapabilitySet::empty();
        set.insert(PluginCapability::ChainQuery);
        set.insert(PluginCapability::MemoryAccess);
        assert!(set.contains(PluginCapability::ChainQuery));
        assert!(set.contains(PluginCapability::MemoryAccess));
        assert!(!set.contains(PluginCapability::NetworkAccess));
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn capability_set_intersection() {
        let a = CapabilitySet::from_iter([
            PluginCapability::ChainQuery,
            PluginCapability::MemoryAccess,
            PluginCapability::NetworkAccess,
        ]);
        let b = CapabilitySet::from_iter([
            PluginCapability::ChainQuery,
            PluginCapability::ToolExecution,
            PluginCapability::MemoryAccess,
        ]);
        let intersection = a.intersection(&b);
        assert_eq!(intersection.len(), 2);
        assert!(intersection.contains(PluginCapability::ChainQuery));
        assert!(intersection.contains(PluginCapability::MemoryAccess));
        assert!(!intersection.contains(PluginCapability::NetworkAccess));
        assert!(!intersection.contains(PluginCapability::ToolExecution));
    }

    #[test]
    fn capability_set_union() {
        let a = CapabilitySet::from_iter([PluginCapability::ChainQuery]);
        let b = CapabilitySet::from_iter([PluginCapability::MemoryAccess]);
        let union = a.union(&b);
        assert_eq!(union.len(), 2);
        assert!(union.contains(PluginCapability::ChainQuery));
        assert!(union.contains(PluginCapability::MemoryAccess));
    }

    #[test]
    fn capability_set_subset_superset() {
        let small = CapabilitySet::from_iter([PluginCapability::ChainQuery]);
        let big = CapabilitySet::from_iter([
            PluginCapability::ChainQuery,
            PluginCapability::MemoryAccess,
        ]);
        assert!(small.is_subset_of(&big));
        assert!(!big.is_subset_of(&small));
        assert!(big.is_superset_of(&small));
        assert!(!small.is_superset_of(&big));
    }

    #[test]
    fn capability_set_difference() {
        let a = CapabilitySet::from_iter([
            PluginCapability::ChainQuery,
            PluginCapability::MemoryAccess,
            PluginCapability::NetworkAccess,
        ]);
        let b = CapabilitySet::from_iter([PluginCapability::ChainQuery]);
        let diff = a.difference(&b);
        assert_eq!(diff.len(), 2);
        assert!(diff.contains(PluginCapability::MemoryAccess));
        assert!(diff.contains(PluginCapability::NetworkAccess));
        assert!(!diff.contains(PluginCapability::ChainQuery));
    }

    #[test]
    fn capability_set_parse_strings() {
        let strings = vec![
            "chain.query".to_string(),
            "memory.read".to_string(),
            "network.http".to_string(),
        ];
        let set = CapabilitySet::parse_strings(&strings).expect("should parse");
        assert_eq!(set.len(), 3);
        assert!(set.contains(PluginCapability::ChainQuery));
        assert!(set.contains(PluginCapability::MemoryAccess));
        assert!(set.contains(PluginCapability::NetworkAccess));
    }

    #[test]
    fn capability_set_parse_strings_unknown() {
        let strings = vec!["chain.query".to_string(), "bogus.cap".to_string()];
        let result = CapabilitySet::parse_strings(&strings);
        assert!(result.is_err());
    }

    #[test]
    fn capability_set_to_strings() {
        let set = CapabilitySet::from_iter([
            PluginCapability::ChainQuery,
            PluginCapability::MemoryAccess,
        ]);
        let strings = set.to_strings();
        assert!(strings.contains(&"chain.query".to_string()));
        assert!(strings.contains(&"memory.read".to_string()));
    }

    #[test]
    fn capability_set_display() {
        let set = CapabilitySet::from_iter([PluginCapability::ChainQuery]);
        let display = set.to_string();
        assert!(display.contains("chain.query"));
        assert!(display.starts_with('['));
        assert!(display.ends_with(']'));
    }

    #[test]
    fn intersection_with_empty_is_empty() {
        let full = CapabilitySet::all();
        let empty = CapabilitySet::empty();
        let result = full.intersection(&empty);
        assert!(result.is_empty());
    }

    #[test]
    fn union_with_empty_is_identity() {
        let a = CapabilitySet::from_iter([PluginCapability::ChainSubmit]);
        let empty = CapabilitySet::empty();
        let result = a.union(&empty);
        assert_eq!(result, a);
    }
}
