//! `polkagent-eval` — Evaluation framework for measuring Polkagent agent performance.
//!
//! This crate provides a complete evaluation harness for benchmarking and
//! regression-testing AI agents in the Polkagent platform.
//!
//! # Module overview
//!
//! | Module | Contents |
//! |--------|----------|
//! | [`types`] | Core eval types: [`EvalSuite`](types::EvalSuite), [`EvalCase`](types::EvalCase), [`EvalCategory`](types::EvalCategory), [`EvalInput`](types::EvalInput), [`Expected`](types::Expected) |
//! | [`runner`] | [`EvalRunner`](runner::EvalRunner): executes suites against a [`ModelExecutor`](polkagent_executor_trait::ModelExecutor) |
//! | [`scorer`] | [`score_case`](scorer::score_case), [`Score`](scorer::Score), [`CheckResult`](scorer::CheckResult) |
//! | [`report`] | [`EvalReport`](report::EvalReport), [`CaseResult`](report::CaseResult), report rendering |
//! | [`corpus`] | Suite loading from disk; [`builtin_safety_suite`](corpus::builtin_safety_suite) |
//! | [`regression`] | [`RegressionDetector`](regression::RegressionDetector), [`compare_reports`](regression::compare_reports) |
//! | [`corpus_manifest`] | [`CorpusManifest`](corpus_manifest::CorpusManifest), [`compute_corpus_digest`](corpus_manifest::compute_corpus_digest), [`verify_corpus_integrity`](corpus_manifest::verify_corpus_integrity), [`load_corpus_from_toml`](corpus_manifest::load_corpus_from_toml) |
//! | [`judge`] | [`ModelAsJudgeScorer`](judge::ModelAsJudgeScorer), [`JudgeConfig`](judge::JudgeConfig), [`JudgeCriterion`](judge::JudgeCriterion), [`JudgeScore`](judge::JudgeScore) |
//! | [`promotion`] | [`PromotionCandidate`](promotion::PromotionCandidate) |
//! | `variant_runner` | **EXPERIMENTAL** (`evolutionary` feature) — multi-variant evaluation comparison |
//! | `thompson` | **EXPERIMENTAL** (`evolutionary` feature) — Thompson sampling selector |

#![forbid(unsafe_code)]
#![warn(
    missing_docs,
    clippy::pedantic,
    clippy::unwrap_used,
    clippy::expect_used
)]
#![allow(
    clippy::module_name_repetitions,
    clippy::must_use_candidate,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc
)]

pub mod corpus;
pub mod corpus_manifest;
pub mod judge;
pub mod promotion;
pub mod regression;
pub mod report;
pub mod runner;
pub mod scorer;
pub mod types;

#[cfg(feature = "evolutionary")]
pub mod thompson;
#[cfg(feature = "evolutionary")]
pub mod variant_runner;

/// Convert a `u64` to `f64` without an unchecked precision-losing cast.
///
/// Each 32-bit half is exactly representable as `f64`; combining the halves
/// retains the conversion's expected final IEEE-754 rounding for large values.
pub(crate) fn u64_to_f64(value: u64) -> f64 {
    let bytes = value.to_be_bytes();
    let high = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    let low = u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
    f64::from(high) * 4_294_967_296.0 + f64::from(low)
}

/// Convert a platform-sized count to `f64` without an unchecked precision-
/// losing cast. Large values are composed from exactly representable 32-bit
/// halves; the final floating-point rounding is appropriate for ratios.
pub(crate) fn usize_to_f64(value: usize) -> f64 {
    let value = u64::try_from(value).unwrap_or(u64::MAX);
    u64_to_f64(value)
}

#[cfg(test)]
mod conversion_tests {
    use super::u64_to_f64;

    #[test]
    fn u64_conversion_preserves_exact_integer_range() {
        assert_eq!(u64_to_f64(0).to_bits(), 0.0_f64.to_bits());
        assert_eq!(
            u64_to_f64(u64::from(u32::MAX)).to_bits(),
            4_294_967_295.0_f64.to_bits()
        );
        assert_eq!(
            u64_to_f64(1_u64 << 32).to_bits(),
            4_294_967_296.0_f64.to_bits()
        );
        assert_eq!(
            u64_to_f64((1_u64 << 53) - 1).to_bits(),
            9_007_199_254_740_991.0_f64.to_bits()
        );
    }

    #[test]
    fn u64_conversion_uses_expected_ieee754_rounding() {
        assert_eq!(
            u64_to_f64((1_u64 << 53) + 1).to_bits(),
            9_007_199_254_740_992.0_f64.to_bits()
        );
        assert_eq!(
            u64_to_f64(u64::MAX).to_bits(),
            18_446_744_073_709_551_616.0_f64.to_bits()
        );
    }
}
