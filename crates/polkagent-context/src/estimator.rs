//! Token estimation from string content.
//!
//! Provides a configurable heuristic for estimating how many tokens a piece of
//! text will consume. The default ratio is 4 characters per token, matching
//! the convention used in [`polkagent_conversation::types::Message`].

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// TokenEstimator
// ---------------------------------------------------------------------------

/// Estimates token counts from character lengths.
///
/// The default configuration uses a ratio of 4 characters per token, which is
/// a reasonable approximation for English text processed by modern sub-word
/// tokenizers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenEstimator {
    /// Number of characters that map to approximately one token.
    chars_per_token: f64,
}

impl TokenEstimator {
    /// Create a new estimator with the given characters-per-token ratio.
    ///
    /// # Panics
    ///
    /// Panics if `chars_per_token` is not positive.
    #[must_use]
    pub fn new(chars_per_token: f64) -> Self {
        assert!(
            chars_per_token > 0.0,
            "chars_per_token must be positive, got {chars_per_token}"
        );
        Self { chars_per_token }
    }

    /// Estimate the number of tokens in `text`.
    ///
    /// Returns at least 1 for any non-empty string.
    #[must_use]
    pub fn estimate(&self, text: &str) -> u32 {
        if text.is_empty() {
            return 0;
        }
        let Ok(characters) = u32::try_from(text.len()) else {
            // Conservatively saturate inputs larger than the public token
            // count can represent instead of underestimating their budget.
            return u32::MAX;
        };
        let estimated = (f64::from(characters) / self.chars_per_token).ceil();
        let tokens = if estimated >= f64::from(u32::MAX) {
            u32::MAX
        } else {
            // The estimator is positive by construction and the upper bound
            // above proves this conversion fits in u32.
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "the value is non-negative and bounded by u32::MAX before conversion"
            )]
            let bounded = estimated as u32;
            bounded
        };
        tokens.max(1)
    }

    /// Estimate the total token count for multiple text segments.
    #[must_use]
    pub fn estimate_many(&self, texts: &[&str]) -> u32 {
        texts.iter().map(|t| self.estimate(t)).sum()
    }

    /// Return the configured characters-per-token ratio.
    #[must_use]
    pub fn chars_per_token(&self) -> f64 {
        self.chars_per_token
    }
}

impl Default for TokenEstimator {
    fn default() -> Self {
        Self {
            chars_per_token: 4.0,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
// Context unit tests intentionally panic at the exact fixture or invariant
// boundary that failed so assembly regressions remain easy to diagnose.
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "context unit-test assertions intentionally panic with focused diagnostics"
)]
mod tests {
    use super::*;

    #[test]
    fn default_ratio_is_four() {
        let est = TokenEstimator::default();
        assert!((est.chars_per_token() - 4.0).abs() < f64::EPSILON);
    }

    #[test]
    fn empty_string_is_zero_tokens() {
        let est = TokenEstimator::default();
        assert_eq!(est.estimate(""), 0);
    }

    #[test]
    fn single_char_is_at_least_one() {
        let est = TokenEstimator::default();
        assert_eq!(est.estimate("a"), 1);
    }

    #[test]
    fn four_chars_is_one_token() {
        let est = TokenEstimator::default();
        assert_eq!(est.estimate("abcd"), 1);
    }

    #[test]
    fn five_chars_rounds_up_to_two() {
        let est = TokenEstimator::default();
        assert_eq!(est.estimate("abcde"), 2);
    }

    #[test]
    fn twelve_chars_is_three_tokens() {
        let est = TokenEstimator::default();
        // 12 / 4 = 3.0, exact
        assert_eq!(est.estimate("hello world!"), 3);
    }

    #[test]
    fn custom_ratio() {
        let est = TokenEstimator::new(2.0);
        // "abcd" = 4 chars, 4/2 = 2 tokens
        assert_eq!(est.estimate("abcd"), 2);
    }

    #[test]
    fn estimate_saturates_when_ratio_exceeds_token_range() {
        let est = TokenEstimator::new(f64::MIN_POSITIVE);
        assert_eq!(est.estimate("a"), u32::MAX);
    }

    #[test]
    fn estimate_many_sums_segments() {
        let est = TokenEstimator::default();
        let total = est.estimate_many(&["abcd", "abcdefgh"]); // 1 + 2 = 3
        assert_eq!(total, 3);
    }

    #[test]
    fn estimate_many_empty_list() {
        let est = TokenEstimator::default();
        assert_eq!(est.estimate_many(&[]), 0);
    }

    #[test]
    #[should_panic(expected = "chars_per_token must be positive")]
    fn zero_ratio_panics() {
        let _ = TokenEstimator::new(0.0);
    }

    #[test]
    #[should_panic(expected = "chars_per_token must be positive")]
    fn negative_ratio_panics() {
        let _ = TokenEstimator::new(-1.0);
    }

    #[test]
    fn serde_round_trip() {
        let est = TokenEstimator::new(3.5);
        let json = serde_json::to_string(&est).expect("serialize");
        let back: TokenEstimator = serde_json::from_str(&json).expect("deserialize");
        assert!((back.chars_per_token() - 3.5).abs() < f64::EPSILON);
    }
}
