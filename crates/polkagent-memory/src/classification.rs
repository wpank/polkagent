//! Classification-aware memory flow control.
//!
//! Memories carry a [`Classification`] label that controls who may read them.
//! A [`ClassificationFilter`] is applied at query time to restrict results to
//! entries whose classification level does not exceed the caller's clearance.
//!
//! # Ordering (least → most sensitive)
//!
//! ```text
//! Public < Internal < Confidential < Restricted
//! ```
//!
//! A filter with `max_classification = Internal` will return `Public` and
//! `Internal` entries but will silently exclude `Confidential` and `Restricted`
//! entries (no error is returned).

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Classification
// ---------------------------------------------------------------------------

/// Security / sensitivity classification for a memory entry.
///
/// Variants are ordered from least to most sensitive so that numeric
/// comparison (`<=`) implements "may read" logic.
#[derive(
    Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Classification {
    /// Freely shareable; no restrictions.
    Public,
    /// Default level; for internal agent use only.
    #[default]
    Internal,
    /// Sensitive; restricted to privileged agent contexts.
    Confidential,
    /// Highly sensitive; only accessible under explicit authorisation.
    Restricted,
}

impl std::fmt::Display for Classification {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Public => write!(f, "public"),
            Self::Internal => write!(f, "internal"),
            Self::Confidential => write!(f, "confidential"),
            Self::Restricted => write!(f, "restricted"),
        }
    }
}

impl std::str::FromStr for Classification {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "public" => Ok(Self::Public),
            "internal" => Ok(Self::Internal),
            "confidential" => Ok(Self::Confidential),
            "restricted" => Ok(Self::Restricted),
            other => Err(format!("unknown classification: {other}")),
        }
    }
}

// ---------------------------------------------------------------------------
// ClassificationFilter
// ---------------------------------------------------------------------------

/// Filters a collection of memory entries to those at or below a maximum
/// classification level.
///
/// # Example
///
/// ```rust
/// use polkagent_memory::classification::{Classification, ClassificationFilter};
///
/// let filter = ClassificationFilter::new(Classification::Internal);
/// assert!(filter.permits(Classification::Public));
/// assert!(filter.permits(Classification::Internal));
/// assert!(!filter.permits(Classification::Confidential));
/// assert!(!filter.permits(Classification::Restricted));
/// ```
#[derive(Debug, Clone)]
pub struct ClassificationFilter {
    /// The maximum (inclusive) classification level that will be returned.
    pub max_classification: Classification,
}

impl ClassificationFilter {
    /// Create a new filter that permits entries up to `max_classification`.
    #[must_use]
    pub fn new(max_classification: Classification) -> Self {
        Self { max_classification }
    }

    /// Return `true` when a memory with the given `classification` is visible
    /// under this filter's clearance.
    #[must_use]
    pub fn permits(&self, classification: Classification) -> bool {
        classification <= self.max_classification
    }

    /// Apply this filter to a slice of `(classification, item)` pairs and
    /// return only the items whose classification is permitted.
    #[must_use]
    pub fn apply<T: Clone>(&self, entries: &[(Classification, T)]) -> Vec<T> {
        entries
            .iter()
            .filter(|(c, _)| self.permits(*c))
            .map(|(_, item)| item.clone())
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- Classification ordering --

    #[test]
    fn classification_ordering() {
        assert!(Classification::Public < Classification::Internal);
        assert!(Classification::Internal < Classification::Confidential);
        assert!(Classification::Confidential < Classification::Restricted);
    }

    #[test]
    fn classification_default_is_internal() {
        assert_eq!(Classification::default(), Classification::Internal);
    }

    #[test]
    fn classification_display() {
        assert_eq!(Classification::Public.to_string(), "public");
        assert_eq!(Classification::Internal.to_string(), "internal");
        assert_eq!(Classification::Confidential.to_string(), "confidential");
        assert_eq!(Classification::Restricted.to_string(), "restricted");
    }

    #[test]
    fn classification_from_str() {
        use std::str::FromStr;
        assert_eq!(
            Classification::from_str("public").unwrap(),
            Classification::Public
        );
        assert_eq!(
            Classification::from_str("internal").unwrap(),
            Classification::Internal
        );
        assert_eq!(
            Classification::from_str("confidential").unwrap(),
            Classification::Confidential
        );
        assert_eq!(
            Classification::from_str("restricted").unwrap(),
            Classification::Restricted
        );
        assert!(Classification::from_str("unknown").is_err());
    }

    #[test]
    fn classification_serde_round_trip() {
        let c = Classification::Confidential;
        let json = serde_json::to_string(&c).unwrap();
        assert_eq!(json, r#""confidential""#);
        let back: Classification = serde_json::from_str(&json).unwrap();
        assert_eq!(back, c);
    }

    // -- ClassificationFilter --

    #[test]
    fn filter_public_visible_to_all() {
        let filter = ClassificationFilter::new(Classification::Public);
        assert!(filter.permits(Classification::Public));
        assert!(!filter.permits(Classification::Internal));
        assert!(!filter.permits(Classification::Confidential));
        assert!(!filter.permits(Classification::Restricted));
    }

    #[test]
    fn filter_internal_hides_confidential_and_restricted() {
        let filter = ClassificationFilter::new(Classification::Internal);
        assert!(filter.permits(Classification::Public));
        assert!(filter.permits(Classification::Internal));
        assert!(!filter.permits(Classification::Confidential));
        assert!(!filter.permits(Classification::Restricted));
    }

    #[test]
    fn filter_restricted_visible_to_all_levels() {
        let filter = ClassificationFilter::new(Classification::Restricted);
        assert!(filter.permits(Classification::Public));
        assert!(filter.permits(Classification::Internal));
        assert!(filter.permits(Classification::Confidential));
        assert!(filter.permits(Classification::Restricted));
    }

    #[test]
    fn filter_respects_ordering() {
        // A Confidential filter allows up to Confidential but not Restricted.
        let filter = ClassificationFilter::new(Classification::Confidential);
        assert!(filter.permits(Classification::Public));
        assert!(filter.permits(Classification::Internal));
        assert!(filter.permits(Classification::Confidential));
        assert!(!filter.permits(Classification::Restricted));
    }

    #[test]
    fn filter_apply_removes_overly_sensitive_entries() {
        let filter = ClassificationFilter::new(Classification::Internal);
        let items = vec![
            (Classification::Public, "pub"),
            (Classification::Internal, "int"),
            (Classification::Confidential, "conf"),
            (Classification::Restricted, "restr"),
        ];
        let visible = filter.apply(&items);
        assert_eq!(visible, vec!["pub", "int"]);
    }
}
