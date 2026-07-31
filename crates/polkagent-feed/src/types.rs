//! Core types for the `polkagent-feed` crate.
//!
//! This module defines the primary domain types used throughout the feed
//! processing system: feeds, cursors, feed items, and supporting enumerations.

use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// FeedId
// ---------------------------------------------------------------------------

/// Unique identifier for a [`Feed`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FeedId(Uuid);

impl FeedId {
    /// Generate a new time-ordered (v7) identifier.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    /// Wrap an existing [`Uuid`].
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

impl Default for FeedId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for FeedId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for FeedId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(s).map(Self)
    }
}

impl From<Uuid> for FeedId {
    fn from(uuid: Uuid) -> Self {
        Self(uuid)
    }
}

impl From<FeedId> for Uuid {
    fn from(id: FeedId) -> Self {
        id.0
    }
}

// ---------------------------------------------------------------------------
// EventFilter
// ---------------------------------------------------------------------------

/// Filter criteria for selecting events from the event bus.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventFilter {
    /// Only forward events whose kind is in this list. An empty list means all
    /// event kinds are accepted.
    pub event_kinds: Vec<String>,

    /// Only forward events produced by agents in this list. An empty list means
    /// events from all agents are accepted.
    pub agent_ids: Vec<String>,
}

impl EventFilter {
    /// Create a new filter that accepts all events.
    #[must_use]
    pub fn allow_all() -> Self {
        Self {
            event_kinds: Vec::new(),
            agent_ids: Vec::new(),
        }
    }

    /// Return `true` if the given event kind and agent id pass this filter.
    ///
    /// An empty `event_kinds` or `agent_ids` list is interpreted as "accept
    /// all values for that dimension".
    #[must_use]
    pub fn matches(&self, event_kind: &str, agent_id: &str) -> bool {
        let kind_ok = self.event_kinds.is_empty()
            || self.event_kinds.iter().any(|k| k == event_kind);
        let agent_ok = self.agent_ids.is_empty()
            || self.agent_ids.iter().any(|a| a == agent_id);
        kind_ok && agent_ok
    }
}

impl Default for EventFilter {
    fn default() -> Self {
        Self::allow_all()
    }
}

// ---------------------------------------------------------------------------
// FeedSource
// ---------------------------------------------------------------------------

/// Defines where a [`Feed`] obtains its items.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FeedSource {
    /// Fires on a cron schedule. The `cron` field uses standard 5-field cron
    /// syntax (e.g. `"0 * * * *"` for hourly).
    Schedule {
        /// Cron expression describing the schedule.
        cron: String,
    },

    /// Receives items pushed by an external webhook call to the given URL path.
    Webhook {
        /// The URL path suffix at which this webhook is mounted.
        path: String,

        /// Optional HMAC-SHA256 secret hash used to verify webhook authenticity.
        secret_hash: Option<String>,
    },

    /// Subscribes to the internal event bus and filters incoming events.
    EventBus {
        /// Filter applied before enqueuing an event as a feed item.
        filter: EventFilter,
    },

    /// Periodically polls on-chain state using a structured query.
    ChainState {
        /// The chain-state query to execute on each poll.
        query: String,

        /// How often to poll, in seconds.
        interval_secs: u64,
    },
}

// ---------------------------------------------------------------------------
// FeedStatus
// ---------------------------------------------------------------------------

/// Operational status of a [`Feed`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum FeedStatus {
    /// The feed is running normally.
    Active,

    /// The feed has been administratively paused; no items will be processed.
    Paused,

    /// The feed has encountered an unrecoverable error.
    Error(String),
}

// ---------------------------------------------------------------------------
// Cursor
// ---------------------------------------------------------------------------

/// Durable bookmark indicating the last-processed position in a feed's item
/// stream.
///
/// Cursors are advanced atomically after successful processing to guarantee
/// at-least-once delivery: if the process crashes between processing an item
/// and advancing the cursor, the item will be reprocessed on restart.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cursor {
    /// An opaque, source-specific string encoding the current read position.
    /// Examples: an event sequence number, a block height, or a timestamp.
    pub position: String,

    /// When the cursor was last successfully advanced.
    pub last_processed_at: DateTime<Utc>,

    /// Total number of feed items that have been successfully processed under
    /// this cursor.
    pub items_processed: u64,
}

impl Cursor {
    /// Create a fresh cursor at the given initial position.
    #[must_use]
    pub fn new(position: impl Into<String>) -> Self {
        Self {
            position: position.into(),
            last_processed_at: Utc::now(),
            items_processed: 0,
        }
    }

    /// Advance the cursor to a new position and increment the processed count.
    #[must_use]
    pub fn advance(mut self, new_position: impl Into<String>) -> Self {
        self.position = new_position.into();
        self.last_processed_at = Utc::now();
        self.items_processed += 1;
        self
    }
}

// ---------------------------------------------------------------------------
// Feed
// ---------------------------------------------------------------------------

/// A named, durable subscription that ingests items from a [`FeedSource`] and
/// routes them through [`crate::trigger::Trigger`]s.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Feed {
    /// Unique identifier for this feed.
    pub id: FeedId,

    /// Human-readable name for this feed.
    pub name: String,

    /// The source from which this feed ingests items.
    pub source: FeedSource,

    /// The current read cursor for this feed.
    pub cursor: Cursor,

    /// Whether this feed is actively processing items.
    pub status: FeedStatus,

    /// The agent that owns and manages this feed.
    pub agent_id: polkagent_core::AgentId,

    /// When this feed was first created.
    pub created_at: DateTime<Utc>,
}

impl Feed {
    /// Create a new active feed.
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        source: FeedSource,
        agent_id: polkagent_core::AgentId,
    ) -> Self {
        Self {
            id: FeedId::new(),
            name: name.into(),
            source,
            cursor: Cursor::new(""),
            status: FeedStatus::Active,
            agent_id,
            created_at: Utc::now(),
        }
    }
}

// ---------------------------------------------------------------------------
// FeedItem
// ---------------------------------------------------------------------------

/// A single item ingested from a feed's source and pending trigger evaluation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedItem {
    /// Unique identifier for this item (stable across redeliveries).
    pub id: Uuid,

    /// The feed that produced this item.
    pub feed_id: FeedId,

    /// The raw payload received from the feed source.
    pub payload: serde_json::Value,

    /// When this item was received / enqueued.
    pub received_at: DateTime<Utc>,

    /// Whether this item has already been fully processed. Items that are
    /// processed but whose cursor has not yet been advanced will have
    /// `processed == false`.
    pub processed: bool,
}

impl FeedItem {
    /// Create a new unprocessed feed item.
    #[must_use]
    pub fn new(feed_id: FeedId, payload: serde_json::Value) -> Self {
        Self {
            id: Uuid::now_v7(),
            feed_id,
            payload,
            received_at: Utc::now(),
            processed: false,
        }
    }
}
