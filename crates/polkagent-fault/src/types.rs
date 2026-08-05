//! Fault type definitions for the fault injection framework.
//!
//! This module defines the core types used to describe faults, corruption
//! patterns, and scheduling policies for fault injection points.

use std::sync::atomic::{AtomicUsize, Ordering};

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Corruption enum
// ---------------------------------------------------------------------------

/// Describes how data should be corrupted when a [`Fault::CorruptData`] fires.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Corruption {
    /// Flip a single bit at the given byte index.
    FlipBit(usize),
    /// Truncate the data to the given byte length.
    Truncate(usize),
    /// Prepend `n` garbage bytes (all `0xFF`) to the data.
    PrependGarbage(usize),
    /// Swap the first and second halves of the data (if long enough).
    SwapFields,
}

impl Corruption {
    /// Apply this corruption to a mutable byte buffer.
    ///
    /// If the corruption cannot be applied (e.g. `FlipBit` on an empty buffer),
    /// this is a no-op.
    pub fn apply(&self, data: &mut Vec<u8>) {
        match self {
            Self::FlipBit(byte_index) => {
                if let Some(b) = data.get_mut(*byte_index) {
                    *b ^= 0xFF;
                }
            }
            Self::Truncate(len) => {
                data.truncate(*len);
            }
            Self::PrependGarbage(n) => {
                let garbage = vec![0xFF_u8; *n];
                let original = std::mem::take(data);
                *data = garbage;
                data.extend_from_slice(&original);
            }
            Self::SwapFields => {
                if data.len() >= 2 {
                    let mid = data.len() / 2;
                    let (left, right) = data.split_at(mid);
                    let mut swapped = right.to_vec();
                    swapped.extend_from_slice(left);
                    *data = swapped;
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Fault enum
// ---------------------------------------------------------------------------

/// A fault that can be injected at a named injection point.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Fault {
    /// Immediately panic — simulates a process crash.
    ///
    /// Tests should use `std::panic::catch_unwind` to observe this fault.
    Crash,

    /// Sleep for `ms` milliseconds, then return a timeout error.
    Timeout {
        /// Duration in milliseconds to sleep before reporting timeout.
        ms: u64,
    },

    /// Return an error with the given message.
    Error {
        /// Human-readable error message returned to the caller.
        message: String,
    },

    /// Corrupt data passing through this point.
    CorruptData {
        /// How to corrupt the data.
        corruption: Corruption,
    },

    /// Sleep for `ms` milliseconds (slow I/O simulation), then continue
    /// normally.
    SlowDown {
        /// Duration in milliseconds to sleep.
        ms: u64,
    },

    /// Simulate a partial write: truncate output to half its length.
    PartialWrite,
}

// ---------------------------------------------------------------------------
// FaultSchedule enum
// ---------------------------------------------------------------------------

/// Controls when a [`FaultPoint`] fires relative to how many times it has
/// been evaluated.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FaultSchedule {
    /// Fire on every evaluation.
    Always,

    /// Fire only after `n` evaluations have occurred (i.e. evaluations
    /// `n+1`, `n+2`, ... all fire).
    AfterN(usize),

    /// Fire with the given probability on each evaluation.
    ///
    /// The value must be in `[0.0, 1.0]`. Values outside this range are
    /// clamped.
    Probability(f64),

    /// Fire exactly once (on the first evaluation), then never again.
    Once,

    /// Fire according to a boolean pattern that repeats.
    ///
    /// The `i`-th evaluation fires if `pattern[i % pattern.len()]` is `true`.
    /// An empty pattern never fires.
    Pattern(Vec<bool>),
}

// ---------------------------------------------------------------------------
// FaultPoint struct
// ---------------------------------------------------------------------------

/// A named fault point: combines a [`Fault`] with a [`FaultSchedule`] and
/// an atomic evaluation counter.
pub struct FaultPoint {
    /// The fault to inject when this point fires.
    pub fault: Fault,
    /// Controls when the fault fires.
    pub schedule: FaultSchedule,
    /// Number of times this point has been evaluated so far.
    pub counter: AtomicUsize,
}

impl std::fmt::Debug for FaultPoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FaultPoint")
            .field("fault", &self.fault)
            .field("schedule", &self.schedule)
            .field("counter", &self.counter.load(Ordering::Relaxed))
            .finish()
    }
}

impl FaultPoint {
    /// Construct a new [`FaultPoint`] with counter initialised to zero.
    pub fn new(fault: Fault, schedule: FaultSchedule) -> Self {
        Self {
            fault,
            schedule,
            counter: AtomicUsize::new(0),
        }
    }

    /// Evaluate whether the fault should fire on this call.
    ///
    /// Atomically increments the internal counter and then applies the
    /// schedule policy. Returns `true` if the fault fires.
    pub fn should_fire(&self) -> bool {
        // Fetch the current count and increment for the next call.
        let call_index = self.counter.fetch_add(1, Ordering::SeqCst);

        match &self.schedule {
            FaultSchedule::Always => true,

            FaultSchedule::AfterN(n) => call_index >= *n,

            FaultSchedule::Probability(p) => {
                let clamped = p.clamp(0.0, 1.0);
                if clamped.is_nan() || clamped <= 0.0 {
                    return false;
                }
                if clamped >= 1.0 {
                    return true;
                }
                // Use a simple LCG mixing the counter to get a deterministic
                // pseudo-random value without pulling in an extra dependency.
                let mixed = lcg_next(call_index as u64);
                let threshold = probability_threshold(clamped);
                mixed < threshold
            }

            FaultSchedule::Once => call_index == 0,

            FaultSchedule::Pattern(pattern) => {
                if pattern.is_empty() {
                    return false;
                }
                pattern[call_index % pattern.len()]
            }
        }
    }

    /// Reset the counter to zero.
    pub fn reset(&self) {
        self.counter.store(0, Ordering::SeqCst);
    }
}

/// Simple 64-bit LCG for pseudo-random probability decisions.
///
/// Constants from Knuth's MMIX.
fn lcg_next(seed: u64) -> u64 {
    seed.wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407)
}

/// Convert a probability to the historical `p * 2^64` threshold directly
/// from its IEEE-754 representation, avoiding lossy numeric casts.
fn probability_threshold(probability: f64) -> u64 {
    if probability.is_nan() || probability <= 0.0 {
        return 0;
    }
    if probability >= 1.0 {
        return u64::MAX;
    }

    let bits = probability.to_bits();
    let exponent_bits = u16::try_from((bits >> 52) & 0x7ff).unwrap_or_default();
    if exponent_bits == 0 {
        return 0;
    }

    let significand = (bits & ((1_u64 << 52) - 1)) | (1_u64 << 52);
    let scale_shift = i32::from(exponent_bits) - 1023 + 12;
    if scale_shift >= 0 {
        significand
            .checked_shl(u32::try_from(scale_shift).unwrap_or(u32::MAX))
            .unwrap_or(u64::MAX)
    } else {
        significand
            .checked_shr(scale_shift.unsigned_abs())
            .unwrap_or(0)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probability_threshold_handles_boundaries_and_exact_fractions() {
        assert_eq!(probability_threshold(f64::NAN), 0);
        assert_eq!(probability_threshold(0.0), 0);
        assert_eq!(probability_threshold(0.25), 1_u64 << 62);
        assert_eq!(probability_threshold(0.5), 1_u64 << 63);
        assert_eq!(probability_threshold(1.0), u64::MAX);
    }

    // -----------------------------------------------------------------------
    // Corruption::apply tests
    // -----------------------------------------------------------------------

    #[test]
    fn corruption_flip_bit_flips_byte() {
        let mut data = vec![0x00, 0x01, 0x02];
        Corruption::FlipBit(1).apply(&mut data);
        assert_eq!(data[1], 0xFF ^ 0x01);
    }

    #[test]
    fn corruption_flip_bit_out_of_bounds_is_noop() {
        let mut data = vec![0x01];
        Corruption::FlipBit(10).apply(&mut data);
        assert_eq!(data, vec![0x01]);
    }

    #[test]
    fn corruption_truncate_shortens_data() {
        let mut data = vec![1, 2, 3, 4, 5];
        Corruption::Truncate(3).apply(&mut data);
        assert_eq!(data, vec![1, 2, 3]);
    }

    #[test]
    fn corruption_truncate_to_zero() {
        let mut data = vec![1, 2, 3];
        Corruption::Truncate(0).apply(&mut data);
        assert!(data.is_empty());
    }

    #[test]
    fn corruption_prepend_garbage_adds_bytes() {
        let mut data = vec![0xAA];
        Corruption::PrependGarbage(3).apply(&mut data);
        assert_eq!(data, vec![0xFF, 0xFF, 0xFF, 0xAA]);
    }

    #[test]
    fn corruption_swap_fields_on_even_length() {
        let mut data = vec![1, 2, 3, 4];
        Corruption::SwapFields.apply(&mut data);
        assert_eq!(data, vec![3, 4, 1, 2]);
    }

    #[test]
    fn corruption_swap_fields_on_single_byte_is_noop() {
        let mut data = vec![42];
        Corruption::SwapFields.apply(&mut data);
        assert_eq!(data, vec![42]);
    }

    // -----------------------------------------------------------------------
    // FaultSchedule tests (via FaultPoint::should_fire)
    // -----------------------------------------------------------------------

    #[test]
    fn schedule_always_fires_every_time() {
        let fp = FaultPoint::new(Fault::Crash, FaultSchedule::Always);
        for _ in 0..10 {
            assert!(fp.should_fire());
        }
    }

    #[test]
    fn schedule_after_n_fires_only_after_n_calls() {
        let fp = FaultPoint::new(Fault::Crash, FaultSchedule::AfterN(3));
        // First three calls (indices 0, 1, 2) should NOT fire.
        assert!(!fp.should_fire()); // index 0
        assert!(!fp.should_fire()); // index 1
        assert!(!fp.should_fire()); // index 2
                                    // Fourth call (index 3) onwards SHOULD fire.
        assert!(fp.should_fire()); // index 3
        assert!(fp.should_fire()); // index 4
    }

    #[test]
    fn schedule_once_fires_exactly_once() {
        let fp = FaultPoint::new(Fault::Crash, FaultSchedule::Once);
        assert!(fp.should_fire()); // first call fires
        assert!(!fp.should_fire()); // second call does not
        assert!(!fp.should_fire()); // third call does not
    }

    #[test]
    fn schedule_pattern_fires_according_to_pattern() {
        let fp = FaultPoint::new(
            Fault::Crash,
            FaultSchedule::Pattern(vec![true, false, true, false]),
        );
        assert!(fp.should_fire()); // index 0 → true
        assert!(!fp.should_fire()); // index 1 → false
        assert!(fp.should_fire()); // index 2 → true
        assert!(!fp.should_fire()); // index 3 → false
                                    // Pattern repeats.
        assert!(fp.should_fire()); // index 4 → true
        assert!(!fp.should_fire()); // index 5 → false
    }

    #[test]
    fn schedule_pattern_empty_never_fires() {
        let fp = FaultPoint::new(Fault::Crash, FaultSchedule::Pattern(vec![]));
        for _ in 0..5 {
            assert!(!fp.should_fire());
        }
    }

    #[test]
    fn schedule_probability_fires_roughly_at_expected_rate() {
        let fp = FaultPoint::new(Fault::Crash, FaultSchedule::Probability(0.5));
        let trials: usize = 10_000;
        let fired: usize = (0..trials).filter(|_| fp.should_fire()).count();
        // Allow ±15% tolerance around 50%.
        let lower = trials * 35 / 100;
        let upper = trials * 65 / 100;
        assert!(
            fired >= lower && fired <= upper,
            "expected roughly 50% fire rate, got {fired}/{trials}"
        );
    }

    #[test]
    fn schedule_probability_zero_never_fires() {
        let fp = FaultPoint::new(Fault::Crash, FaultSchedule::Probability(0.0));
        for _ in 0..100 {
            assert!(!fp.should_fire());
        }
    }

    #[test]
    fn schedule_probability_one_always_fires() {
        let fp = FaultPoint::new(Fault::Crash, FaultSchedule::Probability(1.0));
        for _ in 0..100 {
            assert!(fp.should_fire());
        }
    }

    #[test]
    fn counter_reset_restarts_schedule() {
        let fp = FaultPoint::new(Fault::Crash, FaultSchedule::Once);
        assert!(fp.should_fire()); // fires first time
        assert!(!fp.should_fire()); // silenced
        fp.reset();
        assert!(fp.should_fire()); // fires again after reset
    }
}
