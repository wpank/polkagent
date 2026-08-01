//! Error mapping between SQLite storage errors and [`polkagent_feed::FeedError`].
//!
//! This module provides conversion helpers so that `rusqlite`, `serde_json`,
//! `uuid`, and timestamp-parsing errors are transparently mapped to the
//! [`FeedError`] variants expected by the [`FeedStore`] trait.

use polkagent_feed::FeedError;

/// Map a [`rusqlite::Error`] to [`FeedError`].
///
/// `QueryReturnedNoRows` is mapped to [`FeedError::NotFound`]; all other
/// variants become [`FeedError::ProcessingError`].
pub(crate) fn map_sqlite(e: rusqlite::Error) -> FeedError {
    match e {
        rusqlite::Error::QueryReturnedNoRows => {
            FeedError::NotFound("record not found".to_string())
        }
        other => FeedError::ProcessingError(format!("sqlite error: {other}")),
    }
}

/// Map a [`serde_json::Error`] to [`FeedError::ProcessingError`].
pub(crate) fn map_json(e: serde_json::Error) -> FeedError {
    FeedError::ProcessingError(format!("json error: {e}"))
}

/// Map a [`uuid::Error`] to [`FeedError::ProcessingError`].
pub(crate) fn map_uuid(e: uuid::Error) -> FeedError {
    FeedError::ProcessingError(format!("invalid id: {e}"))
}

/// Parse an ISO-8601 / RFC 3339 timestamp string into a `DateTime<Utc>`.
pub(crate) fn parse_ts(s: &str) -> polkagent_feed::Result<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .map_err(|e| FeedError::ProcessingError(format!("invalid timestamp '{s}': {e}")))
}
