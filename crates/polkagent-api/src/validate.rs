//! Input validation helpers for API request parameters.
//!
//! All public functions in this module perform cheap, allocation-light
//! validation of caller-supplied values and return a typed result or a
//! sanitised version of the input.
//!
//! # Usage
//!
//! Route handlers call these helpers before interacting with the domain
//! layer:
//!
//! ```rust,ignore
//! use crate::validate::{validate_id, validate_page_size};
//!
//! let id = validate_id(&raw_id)?;
//! let limit = validate_page_size(query.limit);
//! ```

use uuid::Uuid;

use crate::error::ApiError;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum length accepted by [`sanitize_string`].
const MAX_STRING_BYTES: usize = 10 * 1024; // 10 KB

/// Default page size returned by [`validate_page_size`] when none is supplied.
const DEFAULT_PAGE_SIZE: usize = 20;

/// Smallest accepted page size.
const MIN_PAGE_SIZE: usize = 1;

/// Largest accepted page size.
const MAX_PAGE_SIZE: usize = 100;

// ---------------------------------------------------------------------------
// validate_id
// ---------------------------------------------------------------------------

/// Parse and validate a UUID string.
///
/// Accepts any UUID format supported by [`Uuid::parse_str`] (hyphenated,
/// simple, braced, etc.).  Returns [`ApiError::ValidationError`] for
/// malformed input.
///
/// # Errors
///
/// Returns `ApiError::ValidationError` when `id` is not a valid UUID.
pub fn validate_id(id: &str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(id).map_err(|_| ApiError::ValidationError(format!("invalid UUID: '{id}'")))
}

// ---------------------------------------------------------------------------
// validate_hex
// ---------------------------------------------------------------------------

/// Decode and validate a hex-encoded byte string.
///
/// Accepts strings with an optional `0x` / `0X` prefix.  The decoded bytes
/// are returned as a `Vec<u8>`.  Returns [`ApiError::ValidationError`] for
/// strings that contain non-hex characters or have an odd length.
///
/// # Errors
///
/// Returns `ApiError::ValidationError` when `hex` is not a valid hex string.
pub fn validate_hex(hex: &str) -> Result<Vec<u8>, ApiError> {
    let stripped = hex
        .strip_prefix("0x")
        .or_else(|| hex.strip_prefix("0X"))
        .unwrap_or(hex);

    if stripped.is_empty() {
        return Err(ApiError::ValidationError(
            "hex string must not be empty".to_owned(),
        ));
    }

    if !stripped.len().is_multiple_of(2) {
        return Err(ApiError::ValidationError(
            "hex string has odd length".to_owned(),
        ));
    }

    (0..stripped.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&stripped[i..i + 2], 16).map_err(|_| {
                ApiError::ValidationError(format!(
                    "invalid hex character at offset {i}: '{}'",
                    &stripped[i..i + 2]
                ))
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// validate_page_size
// ---------------------------------------------------------------------------

/// Clamp an optional page-size parameter to a safe range.
///
/// - `None` → `DEFAULT_PAGE_SIZE` (20)
/// - Below `MIN_PAGE_SIZE` → `MIN_PAGE_SIZE` (1)
/// - Above `MAX_PAGE_SIZE` → `MAX_PAGE_SIZE` (100)
///
/// Never fails; always returns a usable value.
pub fn validate_page_size(limit: Option<usize>) -> usize {
    limit
        .unwrap_or(DEFAULT_PAGE_SIZE)
        .clamp(MIN_PAGE_SIZE, MAX_PAGE_SIZE)
}

// ---------------------------------------------------------------------------
// sanitize_string
// ---------------------------------------------------------------------------

/// Remove ASCII / Unicode control characters and truncate to 10 KB.
///
/// Control characters (Unicode category `Cc`, i.e. code points `U+0000`–
/// `U+001F` and `U+007F`–`U+009F`) are stripped.  Tab (`U+0009`), line feed
/// (`U+000A`), and carriage return (`U+000D`) are preserved because they are
/// commonly legitimate in prose fields.
///
/// After stripping, the string is truncated to at most `MAX_STRING_BYTES`
/// UTF-8 bytes.  Truncation respects character boundaries.
pub fn sanitize_string(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .filter(|c| {
            // Keep printable chars and common whitespace (tab, LF, CR).
            !c.is_control() || matches!(c, '\t' | '\n' | '\r')
        })
        .collect();

    // Truncate at a UTF-8 character boundary at or before MAX_STRING_BYTES.
    if cleaned.len() <= MAX_STRING_BYTES {
        cleaned
    } else {
        let mut end = MAX_STRING_BYTES;
        while !cleaned.is_char_boundary(end) {
            end -= 1;
        }
        cleaned[..end].to_owned()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    reason = "validation tests intentionally fail fast when expected success or error values are absent"
)]
mod tests {
    use super::*;

    // --- validate_id ---

    #[test]
    fn valid_uuid_passes() {
        let id = "550e8400-e29b-41d4-a716-446655440000";
        let result = validate_id(id);
        assert!(
            result.is_ok(),
            "expected valid UUID to pass, got {result:?}"
        );
        assert_eq!(result.unwrap().to_string(), id);
    }

    #[test]
    fn invalid_uuid_rejected() {
        let result = validate_id("not-a-uuid");
        assert!(result.is_err(), "expected invalid UUID to be rejected");
        match result.unwrap_err() {
            ApiError::ValidationError(msg) => assert!(msg.contains("invalid UUID")),
            other => panic!("wrong error variant: {other:?}"),
        }
    }

    #[test]
    fn uuid_without_hyphens_passes() {
        let id = "550e8400e29b41d4a716446655440000";
        assert!(validate_id(id).is_ok());
    }

    #[test]
    fn empty_string_is_invalid_uuid() {
        assert!(validate_id("").is_err());
    }

    // --- validate_hex ---

    #[test]
    fn valid_hex_passes() {
        let result = validate_hex("deadbeef");
        assert!(result.is_ok(), "expected valid hex to pass");
        assert_eq!(result.unwrap(), vec![0xde, 0xad, 0xbe, 0xef]);
    }

    #[test]
    fn valid_hex_with_0x_prefix_passes() {
        let result = validate_hex("0xdeadbeef");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), vec![0xde, 0xad, 0xbe, 0xef]);
    }

    #[test]
    fn invalid_hex_rejected() {
        let result = validate_hex("zzzz");
        assert!(result.is_err(), "expected invalid hex to be rejected");
        match result.unwrap_err() {
            ApiError::ValidationError(msg) => assert!(msg.contains("invalid hex character")),
            other => panic!("wrong error variant: {other:?}"),
        }
    }

    #[test]
    fn odd_length_hex_rejected() {
        let result = validate_hex("abc");
        assert!(result.is_err());
        match result.unwrap_err() {
            ApiError::ValidationError(msg) => assert!(msg.contains("odd length")),
            other => panic!("wrong error variant: {other:?}"),
        }
    }

    #[test]
    fn empty_hex_rejected() {
        assert!(validate_hex("").is_err());
        assert!(validate_hex("0x").is_err());
    }

    #[test]
    fn hex_64_byte_private_key_passes() {
        let key = "a".repeat(128); // 64 bytes as hex
        let result = validate_hex(&key);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().len(), 64);
    }

    // --- validate_page_size ---

    #[test]
    fn page_size_none_returns_default() {
        assert_eq!(validate_page_size(None), DEFAULT_PAGE_SIZE);
    }

    #[test]
    fn page_size_zero_clamped_to_one() {
        assert_eq!(validate_page_size(Some(0)), MIN_PAGE_SIZE);
    }

    #[test]
    fn page_size_within_range_unchanged() {
        assert_eq!(validate_page_size(Some(50)), 50);
    }

    #[test]
    fn page_size_above_max_clamped() {
        assert_eq!(validate_page_size(Some(9999)), MAX_PAGE_SIZE);
    }

    #[test]
    fn page_size_exactly_max_passes() {
        assert_eq!(validate_page_size(Some(100)), 100);
    }

    // --- sanitize_string ---

    #[test]
    fn sanitize_preserves_normal_text() {
        let s = "Hello, World!";
        assert_eq!(sanitize_string(s), s);
    }

    #[test]
    fn sanitize_strips_null_bytes() {
        let s = "hello\x00world";
        assert_eq!(sanitize_string(s), "helloworld");
    }

    #[test]
    fn sanitize_strips_control_chars() {
        // BEL (0x07), ESC (0x1B), DEL (0x7F)
        let s = "a\x07b\x1Bc\x7Fd";
        assert_eq!(sanitize_string(s), "abcd");
    }

    #[test]
    fn sanitize_preserves_tab_lf_cr() {
        let s = "line1\nline2\r\n\ttabbed";
        assert_eq!(sanitize_string(s), s);
    }

    #[test]
    fn sanitize_truncates_long_string() {
        // Build a string longer than 10 KB.
        let long = "a".repeat(MAX_STRING_BYTES + 100);
        let result = sanitize_string(&long);
        assert!(result.len() <= MAX_STRING_BYTES);
    }

    #[test]
    fn sanitize_empty_string_stays_empty() {
        assert_eq!(sanitize_string(""), "");
    }
}
