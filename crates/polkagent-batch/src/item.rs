//! Batch item types.
//!
//! A [`BatchItem`] represents a single unit of work within a [`Batch`](crate::batch::Batch).
//! Each item carries a generic payload `T` and tracks its own processing status.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Unique identifier for a batch item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ItemId(Uuid);

impl ItemId {
    /// Generate a new time-ordered (v7) identifier.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    /// Wrap an existing [`Uuid`] value.
    #[must_use]
    pub fn from_uuid(uuid: Uuid) -> Self {
        Self(uuid)
    }

    /// Return the inner [`Uuid`].
    #[must_use]
    pub fn as_uuid(&self) -> Uuid {
        self.0
    }
}

impl Default for ItemId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for ItemId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for ItemId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(s).map(Self)
    }
}

impl From<Uuid> for ItemId {
    fn from(uuid: Uuid) -> Self {
        Self(uuid)
    }
}

impl From<ItemId> for Uuid {
    fn from(id: ItemId) -> Self {
        id.0
    }
}

/// Processing status of a single batch item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemStatus {
    /// The item is waiting to be processed.
    Pending,
    /// The item is currently being processed.
    Processing,
    /// The item was processed successfully.
    Completed,
    /// The item failed to process.
    Failed,
    /// The item was skipped (e.g., due to error policy).
    Skipped,
}

impl fmt::Display for ItemStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pending => write!(f, "pending"),
            Self::Processing => write!(f, "processing"),
            Self::Completed => write!(f, "completed"),
            Self::Failed => write!(f, "failed"),
            Self::Skipped => write!(f, "skipped"),
        }
    }
}

/// A single item within a batch.
///
/// Carries a generic payload `T` alongside processing metadata. The `result`
/// and `error` fields are populated after the item has been processed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchItem<T> {
    /// Unique identifier for this item.
    pub id: ItemId,
    /// The payload to be processed.
    pub payload: T,
    /// Current processing status.
    pub status: ItemStatus,
    /// The result produced by the handler, if successful.
    pub result: Option<serde_json::Value>,
    /// An error message, if the handler failed.
    pub error: Option<String>,
}

impl<T> BatchItem<T> {
    /// Create a new pending item with the given payload.
    #[must_use]
    pub fn new(payload: T) -> Self {
        Self {
            id: ItemId::new(),
            payload,
            status: ItemStatus::Pending,
            result: None,
            error: None,
        }
    }

    /// Create a new pending item with an explicit ID.
    #[must_use]
    pub fn with_id(id: ItemId, payload: T) -> Self {
        Self {
            id,
            payload,
            status: ItemStatus::Pending,
            result: None,
            error: None,
        }
    }

    /// Returns `true` if the item has been processed (completed or failed).
    #[must_use]
    pub fn is_done(&self) -> bool {
        matches!(
            self.status,
            ItemStatus::Completed | ItemStatus::Failed | ItemStatus::Skipped
        )
    }

    /// Returns `true` if the item completed successfully.
    #[must_use]
    pub fn is_success(&self) -> bool {
        self.status == ItemStatus::Completed
    }
}
