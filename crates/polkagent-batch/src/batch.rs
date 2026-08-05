//! Batch type — a collection of items to be processed together.
//!
//! A [`Batch`] groups multiple [`BatchItem`] values
//! under a single identity, configuration, and status.

use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::config::BatchConfig;
use crate::item::{BatchItem, ItemStatus};

/// Unique identifier for a batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BatchId(Uuid);

impl BatchId {
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

impl Default for BatchId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for BatchId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for BatchId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(s).map(Self)
    }
}

impl From<Uuid> for BatchId {
    fn from(uuid: Uuid) -> Self {
        Self(uuid)
    }
}

impl From<BatchId> for Uuid {
    fn from(id: BatchId) -> Self {
        id.0
    }
}

/// Overall status of a batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BatchStatus {
    /// The batch is being assembled and is not yet being processed.
    Collecting,
    /// The batch is currently being processed.
    Processing,
    /// All items have been processed (possibly with some failures).
    Completed,
    /// The batch was aborted (e.g. due to a `FailFast` policy).
    Aborted,
}

impl fmt::Display for BatchStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Collecting => write!(f, "collecting"),
            Self::Processing => write!(f, "processing"),
            Self::Completed => write!(f, "completed"),
            Self::Aborted => write!(f, "aborted"),
        }
    }
}

/// A batch of items to be processed together.
///
/// `T` is the payload type carried by each item in the batch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Batch<T> {
    /// Unique identifier for this batch.
    pub id: BatchId,
    /// The items in this batch.
    pub items: Vec<BatchItem<T>>,
    /// Current overall status of the batch.
    pub status: BatchStatus,
    /// When this batch was created.
    pub created_at: DateTime<Utc>,
    /// Processing configuration.
    pub config: BatchConfig,
}

impl<T> Batch<T> {
    /// Create a new empty batch with the given configuration.
    #[must_use]
    pub fn new(config: BatchConfig) -> Self {
        Self {
            id: BatchId::new(),
            items: Vec::new(),
            status: BatchStatus::Collecting,
            created_at: Utc::now(),
            config,
        }
    }

    /// Create a new batch with an explicit ID and configuration.
    #[must_use]
    pub fn with_id(id: BatchId, config: BatchConfig) -> Self {
        Self {
            id,
            items: Vec::new(),
            status: BatchStatus::Collecting,
            created_at: Utc::now(),
            config,
        }
    }

    /// Add an item to the batch. Returns `false` if the batch is at `max_size`
    /// and cannot accept more items (when `max_size` > 0).
    pub fn add(&mut self, item: BatchItem<T>) -> bool {
        if self.config.max_size > 0 && self.items.len() >= self.config.max_size {
            return false;
        }
        self.items.push(item);
        true
    }

    /// Add a payload directly, wrapping it in a `BatchItem`.
    pub fn push(&mut self, payload: T) -> bool {
        self.add(BatchItem::new(payload))
    }

    /// Returns the number of items in the batch.
    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Returns `true` if the batch contains no items.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Returns `true` if the batch has reached its configured `max_size`.
    #[must_use]
    pub fn is_full(&self) -> bool {
        self.config.max_size > 0 && self.items.len() >= self.config.max_size
    }

    /// Count items by status.
    #[must_use]
    pub fn count_by_status(&self, status: &ItemStatus) -> usize {
        self.items.iter().filter(|i| &i.status == status).count()
    }

    /// Count completed items.
    #[must_use]
    pub fn completed_count(&self) -> usize {
        self.count_by_status(&ItemStatus::Completed)
    }

    /// Count failed items.
    #[must_use]
    pub fn failed_count(&self) -> usize {
        self.count_by_status(&ItemStatus::Failed)
    }
}
