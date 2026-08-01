//! Error mapping between `rusqlite` errors and [`GroupError`].
//!
//! This module provides helpers that translate SQLite-level failures into the
//! domain error types expected by the [`GroupStore`] trait.

use polkagent_group::GroupError;

/// Map a [`rusqlite::Error`] to a [`GroupError::Internal`].
pub(crate) fn map_err(e: rusqlite::Error) -> GroupError {
    GroupError::Internal(format!("sqlite error: {e}"))
}

/// Returns `true` if the error is a UNIQUE/PK constraint violation.
pub(crate) fn is_constraint_violation(e: &rusqlite::Error) -> bool {
    matches!(
        e,
        rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error {
                code: rusqlite::ffi::ErrorCode::ConstraintViolation,
                ..
            },
            _,
        )
    )
}
